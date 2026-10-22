use crate::core;
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const ENDPOINT: &str = "https://services.firenet.iidx.pro/api/iidx/plugin/music-reg";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSettings {
    #[serde(default)] pub auto_sync: bool,
    #[serde(default)] pub access_code: String,
    #[serde(default = "default_true")] pub omni_overlay: bool,
}
fn default_true() -> bool { true }
impl Default for SyncSettings {
    fn default() -> Self { Self { auto_sync: false, access_code: String::new(), omni_overlay: true } }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub occurred_at: String,
    pub request: Value,
    pub recipient_key: String,
    #[serde(default)] pub upload_allowed: bool,
    #[serde(default)] pub blocked_reason: Option<String>,
    #[serde(default)] pub attempts: u32,
    #[serde(default)] pub next_attempt_ms: u64,
    #[serde(default)] pub uploaded_at: Option<String>,
    #[serde(default)] pub upload_count: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Submission<'a> {
    schema_version: u32,
    access_code: &'a str,
    event_id: &'a str,
    occurred_at: &'a str,
    request: &'a Value,
}

pub fn settings_path(dir: &Path) -> PathBuf { dir.join("AnyToFire_sync.json") }
pub fn queue_path(dir: &Path) -> PathBuf { dir.join("AnyToFire_recent") }
pub fn recipient_key(code: &str) -> String { format!("{:x}", Sha256::digest(code.as_bytes())) }
pub fn official_chart(request: &Value, official: &core::Catalog, played: &core::Catalog) -> bool {
    let Some(module) = request["children"].as_array().and_then(|x| x.first()) else { return false; };
    let expected = format!("IIDX{}music", official.version);
    if module["name"] != expected || module["attributes"]["method"] != "reg" { return false; }
    let attrs = &module["attributes"];
    let Some(id) = attrs["mid"].as_str().and_then(|x| x.parse::<u32>().ok()) else { return false; };
    let Some(chart) = attrs["clid"].as_str().and_then(|x| x.parse::<usize>().ok()) else { return false; };
    if chart >= 10 { return false; }
    let (Some(ac), Some(game)) = (official.songs.get(&id), played.songs.get(&id)) else { return false; };
    ac.title == game.title && ac.levels[chart] != 0 && game.levels[chart] == ac.levels[chart]
}
pub fn load_settings(dir: &Path) -> io::Result<SyncSettings> {
    let path = settings_path(dir);
    if !path.exists() { return Ok(SyncSettings::default()); }
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}
pub fn save_settings(dir: &Path, settings: &SyncSettings) -> io::Result<()> {
    core::atomic_write(&settings_path(dir), &serde_json::to_vec_pretty(settings)?)
}
pub fn new_event(request: Value, code: &str, upload_allowed: bool) -> PlayEvent {
    PlayEvent { schema_version: 1, event_id: uuid::Uuid::new_v4().to_string(),
        occurred_at: Local::now().to_rfc3339(), request, recipient_key: recipient_key(code), upload_allowed,
        blocked_reason: None, attempts: 0, next_attempt_ms: 0, uploaded_at: None, upload_count: 0 }
}
pub fn persist(dir: &Path, event: &PlayEvent) -> io::Result<PathBuf> {
    let queue = queue_path(dir);
    fs::create_dir_all(&queue)?;
    let path = queue.join(format!("{}.json", event.event_id));
    if path.exists() { return Ok(path); }
    core::atomic_write(&path, &serde_json::to_vec(event)?)?;
    Ok(path)
}
pub fn list(dir: &Path) -> io::Result<Vec<(PathBuf, PlayEvent)>> {
    let queue = queue_path(dir);
    if !queue.exists() { return Ok(Vec::new()); }
    let mut result = Vec::new();
    for entry in fs::read_dir(queue)? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "json") {
            if let Ok(event) = serde_json::from_slice::<PlayEvent>(&fs::read(&path)?) { result.push((path, event)); }
        }
    }
    result.sort_by(|a, b| a.1.occurred_at.cmp(&b.1.occurred_at));
    Ok(result)
}
fn retry_after(response: &ureq::Response) -> Option<Duration> {
    let value=response.header("Retry-After")?;
    if let Ok(seconds)=value.parse::<u64>() { return Some(Duration::from_secs(seconds)); }
    let date=chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let remaining=date.timestamp().saturating_sub(chrono::Utc::now().timestamp());
    Some(Duration::from_secs(remaining as u64))
}
pub struct UploadError { pub message: String, pub retry: bool, pub retry_after: Option<Duration> }
fn permanent(message: impl Into<String>) -> UploadError { UploadError { message: message.into(), retry: false, retry_after: None } }
fn temporary(message: impl Into<String>, retry_after: Option<Duration>) -> UploadError { UploadError { message: message.into(), retry: true, retry_after } }
fn error_token(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.is_empty() && text.len() <= 96
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_-.[]".contains(&byte)))
}
fn api_error_message(status: u16, reply: Option<&Value>) -> String {
    let mut message = format!("HTTP {status}");
    let Some(reply) = reply else { return message; };
    let code = reply.get("code").and_then(error_token);
    if let Some(code) = code { message.push_str(&format!(" {code}")); }
    if let Some(violations) = reply.get("violations").and_then(Value::as_array) {
        let fields: Vec<String> = violations.iter().take(3).filter_map(|item| {
            let field = item.get("field").and_then(error_token)?;
            let code = item.get("code").and_then(error_token)?;
            Some(format!("{field}:{code}"))
        }).collect();
        if !fields.is_empty() { message.push_str(&format!(" [{}]", fields.join(", "))); }
    }
    if let Some(id) = reply.get("requestId").and_then(error_token) { message.push_str(&format!(" requestId={id}")); }
    if status == 404 && code == Some("ACCESS_CODE_INVALID") {
        message.push_str("；火网第三方访问码无效或已撤销，请在设置中更新");
    } else if status == 410 && code == Some("ACCESS_CODE_EXPIRED") {
        message.push_str("；火网第三方访问码已过期，请在设置中更新");
    } else if status == 400 && reply.get("violations").and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.get("code").and_then(Value::as_str) == Some("CONFLICTING_PLAY_LOG"))) {
        message.push_str("；原始游玩日志与顶层成绩字段不一致，需火网服务端兼容");
    }
    message
}
pub fn submit(event: &PlayEvent, code: &str) -> Result<u16, UploadError> {
    if code.is_empty() || recipient_key(code) != event.recipient_key { return Err(permanent("接收账号与事件绑定不一致")); }
    let body = serde_json::to_vec(&Submission { schema_version: event.schema_version,
        access_code: code, event_id: &event.event_id, occurred_at: &event.occurred_at,
        request: &event.request }).map_err(|e| permanent(e.to_string()))?;
    if body.len() > 1_048_576 { return Err(permanent("事件超过接口的 1 MiB 上限")); }
    let tls = ureq::native_tls::TlsConnector::new()
        .map_err(|e| temporary(format!("TLS 初始化失败: {e}"), None))?;
    let agent = ureq::AgentBuilder::new().tls_connector(Arc::new(tls)).build();
    let result = agent.post(ENDPOINT).set("Content-Type", "application/json; charset=utf-8")
        .set("Accept", "application/json").timeout(Duration::from_secs(12)).send_bytes(&body);
    match result {
        Ok(response) => {
            let status = response.status();
            if !matches!(status, 200 | 201 | 202) { return Err(temporary(format!("HTTP {status}"),None)); }
            let reply: Value = response.into_json().map_err(|e| temporary(format!("响应 JSON 无效: {e}"),None))?;
            if reply["eventId"] != event.event_id { return Err(temporary("响应 eventId 不匹配",None)); }
            let expected = match status { 200 => "DUPLICATE", 201 => "INSERTED", 202 => "PENDING", _ => unreachable!() };
            if reply["status"] != expected { return Err(temporary("响应状态不匹配",None)); }
            Ok(status)
        }
        Err(ureq::Error::Status(status, response)) => {
            let wait = retry_after(&response);
            let reply: Option<Value> = response.into_json().ok();
            let message = api_error_message(status, reply.as_ref());
            if status == 429 || status >= 500 { Err(temporary(message, wait)) }
            else { Err(permanent(message)) }
        }
        Err(error) => Err(temporary(format!("网络错误: {error}"),None)),
    }
}

fn mark_uploaded(event: &mut PlayEvent) {
    event.upload_count = if event.uploaded_at.is_some() {
        event.upload_count.max(1).saturating_add(1)
    } else {
        event.upload_count.saturating_add(1)
    };
    event.uploaded_at = Some(Local::now().to_rfc3339());
}

pub fn mark_native_uploaded(dir: &Path, event_id: &str) -> io::Result<bool> {
    let path = queue_path(dir).join(format!("{event_id}.json"));
    let mut event: PlayEvent = serde_json::from_slice(&fs::read(&path)?)?;
    if event.uploaded_at.is_some() { return Ok(false); }
    mark_uploaded(&mut event);
    event.blocked_reason = None;
    event.next_attempt_ms = 0;
    core::atomic_write(&path, &serde_json::to_vec(&event)?)?;
    Ok(true)
}

pub fn flush_once(dir: &Path, settings: &SyncSettings, on_result: impl Fn(&str)) -> io::Result<()> {
    if !settings.auto_sync || settings.access_code.is_empty() { return Ok(()); }
    let key = recipient_key(&settings.access_code);
    let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    for (path, mut event) in list(dir)? {
        if event.recipient_key != key || !event.upload_allowed || event.uploaded_at.is_some() || event.blocked_reason.is_some() || event.next_attempt_ms > now { continue; }
        match submit(&event, &settings.access_code) {
            Ok(status) => {
                mark_uploaded(&mut event);
                core::atomic_write(&path, &serde_json::to_vec(&event)?)?;
                on_result(&format!("单局 {} 已上传 ({status})", event.event_id));
            }
            Err(error) => {
                event.attempts=event.attempts.saturating_add(1);
                if error.retry {
                    let exponential=1u64.checked_shl(event.attempts.saturating_sub(1).min(6)).unwrap_or(60).min(60);
                    let jitter=(u64::from_le_bytes(Sha256::digest(format!("{}:{}",event.event_id,event.attempts).as_bytes())[..8].try_into().unwrap()) % 1000) as u64;
                    let delay=error.retry_after.unwrap_or(Duration::from_secs(exponential));
                    event.next_attempt_ms=now.saturating_add(delay.as_millis() as u64).saturating_add(jitter);
                    on_result(&format!("单局 {} 待重试: {}",event.event_id,error.message));
                } else {
                    event.blocked_reason=Some(error.message.clone());
                    on_result(&format!("单局 {} 已暂停: {}",event.event_id,error.message));
                }
                core::atomic_write(&path,&serde_json::to_vec(&event)?)?;
                break;
            }
        }
    }
    Ok(())
}

pub fn manual_upload(dir: &Path, ids: &[String], confirm_duplicate: bool, settings: &SyncSettings) -> Result<usize, String> {
    if settings.access_code.is_empty() { return Err("请先在设置中填写火网第三方访问码".into()); }
    if ids.is_empty() { return Err("请先选择最近游玩分数".into()); }
    let key = recipient_key(&settings.access_code);
    let events = list(dir).map_err(|e| e.to_string())?;
    let mut chosen = Vec::new();
    for id in ids {
        let (path, event) = events.iter().find(|(_, event)| &event.event_id == id).ok_or("选中的单局已不存在，请刷新列表")?;
        if !event.upload_allowed { return Err(format!("单局 {id} 不符合官方 AC 曲库，不能上传")); }
        if event.recipient_key != key && event.recipient_key != recipient_key("") {
            return Err(format!("单局 {id} 已绑定其他访问码，不能上传到当前账号"));
        }
        if event.uploaded_at.is_some() && !confirm_duplicate { return Err("含有已上传的成绩，请确认再次上传".into()); }
        chosen.push((path.clone(), event.clone()));
    }
    let mut count = 0;
    for (path, mut event) in chosen {
        event.recipient_key = key.clone();
        // Persist account binding before network I/O so retries cannot switch recipients.
        core::atomic_write(&path, &serde_json::to_vec(&event).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if let Err(error) = submit(&event, &settings.access_code) {
            event.attempts = event.attempts.saturating_add(1);
            if !error.retry { event.blocked_reason = Some(error.message.clone()); }
            core::atomic_write(&path, &serde_json::to_vec(&event).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            return Err(format!("已上传 {count} 项；单局 {} 失败：{}", event.event_id, error.message));
        }
        mark_uploaded(&mut event);
        event.blocked_reason = None;
        event.next_attempt_ms = 0;
        core::atomic_write(&path, &serde_json::to_vec(&event).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        count += 1;
    }
    Ok(count)
}

pub fn recent_rows(dir: &Path) -> io::Result<Vec<Value>> {
    let mut rows = Vec::new();
    let events = list(dir)?;
    if events.is_empty() { return Ok(rows); }
    let catalog = core::read_catalog(&dir.join("data/info/0/music_data.bin")).ok();
    let omni = [
        "data_mods/ominimix/info/1/music_omni.bin",
        "data_mods/omnimix/info/1/music_omni.bin",
        "data/info/1/music_omni.bin",
        "data/info/0/music_omni.bin",
        "ominimix/info/1/music_omni.bin",
        "omnimix/info/1/music_omni.bin",
        "music_omni.bin",
    ].iter().find_map(|relative| core::read_catalog(&dir.join(relative)).ok());
    for (_, event) in events {
        let music = event.request["children"].as_array().and_then(|v| v.first());
        let attrs = music.and_then(|v| v.get("attributes"));
        let music_id = attrs.and_then(|v| v["mid"].as_str()).unwrap_or("");
        let id = music_id.parse::<u32>().ok();
        let title = id.and_then(|id| catalog.as_ref().and_then(|catalog| catalog.songs.get(&id))
            .or_else(|| omni.as_ref().and_then(|catalog| catalog.songs.get(&id))))
            .map(|song| song.title.trim()).filter(|title| !title.is_empty())
            .map(str::to_owned).unwrap_or_else(|| format!("未知曲目 (ID: {})", if music_id.is_empty() { "?" } else { music_id }));
        let blocked = if event.blocked_reason.as_deref() == Some("HTTP 400")
            && attrs.is_some_and(|attrs| attrs["mnum"] == "-1")
            && music.and_then(|module| module["children"].as_array()).is_some_and(|children| {
                children.iter().find(|child| child["name"] == "music_play_log")
                    .is_some_and(|log| log["children"].as_array().is_some_and(|nodes| nodes.iter()
                        .any(|node| node["name"] == "is_sudden_death" && node["value"] == true)))
            }) {
            Some("HTTP 400：中途死亡原始包顶层 mnum=-1，游玩日志含实际失误数；火网服务端校验拒绝".to_owned())
        } else { event.blocked_reason.clone() };
        rows.push(serde_json::json!({"eventId":event.event_id,"occurredAt":event.occurred_at,
            "musicId":music_id,"title":title,
            "exScore":attrs.and_then(|v| {
                let pg = v["pgnum"].as_str()?.parse::<u32>().ok()?;
                let great = v["gnum"].as_str()?.parse::<u32>().ok()?;
                pg.checked_mul(2)?.checked_add(great)
            }),
            "module":music.and_then(|v| v["name"].as_str()).unwrap_or(""),
            "uploadAllowed":event.upload_allowed,"uploadedAt":event.uploaded_at,
            "uploadCount":if event.uploaded_at.is_some() { event.upload_count.max(1) } else { 0 },
            "blocked":blocked}));
    }
    Ok(rows)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test]
    fn api_errors_keep_validation_codes_without_echoing_server_details() {
        let reply = serde_json::json!({"code":"VALIDATION_FAILED","detail":"secret must not appear",
            "violations":[{"field":"request","code":"CONFLICTING_PLAY_LOG","params":{"raw":"secret"}}],
            "requestId":"00000000-0000-0000-0000-000000000001"});
        let message = api_error_message(400, Some(&reply));
        assert!(message.contains("request:CONFLICTING_PLAY_LOG"));
        assert!(message.contains("requestId="));
        assert!(!message.contains("secret"));
        let invalid = serde_json::json!({"code":"ACCESS_CODE_INVALID"});
        assert!(api_error_message(404, Some(&invalid)).contains("访问码无效"));
        assert_eq!(api_error_message(404, None), "HTTP 404");
    }
    #[test]
    fn old_sudden_death_400_shows_actionable_reason() {
        let dir = std::env::temp_dir().join(format!("anytofire-death-test-{}", uuid::Uuid::new_v4()));
        let request = serde_json::json!({"children":[{"name":"IIDX34music","attributes":{"mid":"42","mnum":"-1"},
            "children":[{"name":"music_play_log","children":[{"name":"is_sudden_death","value":true}]}]}]});
        let mut event = new_event(request, "secret", true);
        event.blocked_reason = Some("HTTP 400".into());
        persist(&dir, &event).unwrap();
        let rows = recent_rows(&dir).unwrap();
        assert!(rows[0]["blocked"].as_str().unwrap().contains("中途死亡"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test] fn event_omits_access_code_and_preserves_identity() {
        let event = new_event(serde_json::json!({"name":"call","attributes":{"srcid":"ORIGINAL"}}), "secret-code", true);
        let encoded = serde_json::to_string(&event).unwrap();
        assert!(!encoded.contains("secret-code"));
        assert!(encoded.contains("ORIGINAL"));
        assert!(chrono::DateTime::parse_from_rfc3339(&event.occurred_at).is_ok());
    }
    #[test] fn only_matching_ac_chart_is_uploadable() {
        let song = |title: &str| core::Song { title: title.into(), genre: String::new(), artist: String::new(), levels: [0,0,0,12,0,0,0,0,0,0] };
        let ac = core::Catalog { version: 34, maximum: 100, sha256: String::new(), songs: [(42,song("AC SONG"))].into() };
        let mut played = core::Catalog { version: 34, maximum: 100, sha256: String::new(), songs: [(42,song("AC SONG")),(43,song("OMNI"))].into() };
        let request = |id| serde_json::json!({"children":[{"name":"IIDX34music","attributes":{"method":"reg","mid":id,"clid":"3"}}]});
        assert!(official_chart(&request("42"),&ac,&played));
        assert!(!official_chart(&request("43"),&ac,&played));
        played.songs.insert(42,song("REPLACED"));
        assert!(!official_chart(&request("42"),&ac,&played));
    }
    #[test] fn uploaded_event_stays_visible_and_needs_duplicate_confirmation() {
        let dir = std::env::temp_dir().join(format!("anytofire-upload-test-{}", uuid::Uuid::new_v4()));
        let mut event = new_event(serde_json::json!({"children":[{"name":"IIDX34music","attributes":{"mid":"42"}}]}), "secret", true);
        event.uploaded_at = Some(Local::now().to_rfc3339());
        persist(&dir, &event).unwrap();
        let rows = recent_rows(&dir).unwrap();
        assert_eq!(rows[0]["uploadedAt"], event.uploaded_at.unwrap());
        let settings = SyncSettings { auto_sync: false, access_code: "secret".into(), omni_overlay: true };
        let result = manual_upload(&dir, &[event.event_id], false, &settings);
        assert!(result.unwrap_err().contains("确认再次上传"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test] fn recent_rows_show_ex_score_and_success_count() {
        let dir = std::env::temp_dir().join(format!("anytofire-recent-test-{}", uuid::Uuid::new_v4()));
        let mut event = new_event(serde_json::json!({"children":[{"name":"IIDX34music","attributes":{"mid":"42","pgnum":"900","gnum":"100"}}]}), "secret", true);
        mark_uploaded(&mut event);
        mark_uploaded(&mut event);
        persist(&dir, &event).unwrap();
        let rows = recent_rows(&dir).unwrap();
        assert_eq!(rows[0]["exScore"], 1900);
        assert_eq!(rows[0]["uploadCount"], 2);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test] fn recent_rows_use_omni_title_when_ac_catalog_misses_song() {
        fn write_catalog(path: &Path, id: u32, title: &str) {
            let maximum = 128u32;
            let mut data = vec![0; 16 + maximum as usize * 4 + 2040];
            data[..4].copy_from_slice(b"IIDX");
            data[4..8].copy_from_slice(&34u32.to_le_bytes());
            data[8..12].copy_from_slice(&1u32.to_le_bytes());
            data[12..16].copy_from_slice(&maximum.to_le_bytes());
            let start = 16 + maximum as usize * 4;
            for (i, unit) in title.encode_utf16().enumerate() {
                data[start + i * 2..start + i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
            }
            data[start + 0x67c..start + 0x680].copy_from_slice(&id.to_le_bytes());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, data).unwrap();
        }
        let dir = std::env::temp_dir().join(format!("anytofire-omni-title-test-{}", uuid::Uuid::new_v4()));
        let ac_path = dir.join("data/info/0/music_data.bin");
        let omni_path = dir.join("data_mods/ominimix/info/1/music_omni.bin");
        write_catalog(&ac_path, 1, "AC SONG");
        write_catalog(&omni_path, 42, "OMNI SONG");
        let event = new_event(serde_json::json!({"children":[{"name":"IIDX34music","attributes":{"mid":"42"}}]}), "", false);
        persist(&dir, &event).unwrap();
        assert_eq!(recent_rows(&dir).unwrap()[0]["title"], "OMNI SONG");
        write_catalog(&ac_path, 42, "AC TITLE");
        assert_eq!(recent_rows(&dir).unwrap()[0]["title"], "AC TITLE");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test] fn native_ack_marks_one_upload_without_repeating() {
        let dir = std::env::temp_dir().join(format!("anytofire-native-ack-test-{}", uuid::Uuid::new_v4()));
        let event = new_event(serde_json::json!({"children":[]}), "", true);
        persist(&dir, &event).unwrap();
        assert!(mark_native_uploaded(&dir, &event.event_id).unwrap());
        assert!(!mark_native_uploaded(&dir, &event.event_id).unwrap());
        let rows = recent_rows(&dir).unwrap();
        assert_eq!(rows[0]["uploadCount"], 1);
        assert!(rows[0]["uploadedAt"].is_string());
        fs::remove_dir_all(dir).unwrap();
    }
}
