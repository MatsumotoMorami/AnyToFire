use chrono::Local;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const DIFFS: [&str; 5] = ["BEGINNER", "NORMAL", "HYPER", "ANOTHER", "LEGGENDARIA"];
pub const LAMPS: [&str; 8] = ["NO PLAY", "FAILED", "ASSIST CLEAR", "EASY CLEAR", "CLEAR", "HARD CLEAR", "EX HARD CLEAR", "FULL COMBO"];
const CHARTS: [&str; 10] = ["spb", "spn", "sph", "spa", "spl", "dpb", "dpn", "dph", "dpa", "dpl"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Score {
    pub music: u32,
    pub chart: u8,
    pub ex: u32,
    pub bp: Option<u32>,
    pub lamp: u8,
    pub title: String,
    pub genre: String,
    pub artist: String,
    pub level: u8,
    #[serde(default)] pub levels: [u8; 10],
    #[serde(default)] pub pgreat: Option<u32>,
    #[serde(default)] pub great: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Collection {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub server_url: String,
    pub player_internal: String,
    pub iidx_id: Option<String>,
    pub nickname: Option<String>,
    pub captured_at: String,
    pub catalog_sha256: String,
    #[serde(default)] pub asphyxia_refid: Option<String>,
    #[serde(default)] pub parents: Vec<String>,
    pub scores: Vec<Score>,
}

#[derive(Clone)]
pub struct Song {
    pub title: String,
    pub genre: String,
    pub artist: String,
    pub levels: [u8; 10],
}

pub struct Catalog {
    pub version: u32,
    pub maximum: usize,
    pub sha256: String,
    pub songs: HashMap<u32, Song>,
}

fn nul_utf16(data: &[u8]) -> String {
    let u: Vec<u16> = data.chunks_exact(2).map(|x| u16::from_le_bytes([x[0], x[1]])).take_while(|v| *v != 0).collect();
    String::from_utf16_lossy(&u)
}
fn nul_utf8(data: &[u8]) -> String {
    let end = data.iter().position(|x| *x == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).to_string()
}
fn u32_at(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}
pub fn read_catalog(path: &Path) -> io::Result<Catalog> {
    let data = fs::read(path)?;
    if data.len() < 16 || &data[..4] != b"IIDX" { return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid catalog header")); }
    let version = u32_at(&data, 4);
    let count = u32_at(&data, 8) as usize;
    let maximum = u32_at(&data, 12) as usize;
    let start = 16usize.checked_add(maximum.checked_mul(4).ok_or(io::ErrorKind::InvalidData)?).ok_or(io::ErrorKind::InvalidData)?;
    if !(version == 33 || version == 34) || count == 0 || count > 35000 || maximum == 0 || maximum > 100000 || start.checked_add(count * 2040) != Some(data.len()) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported catalog layout"));
    }
    let mut songs = HashMap::with_capacity(count);
    for i in 0..count {
        let row = &data[start + i * 2040..start + (i + 1) * 2040];
        let id = u32_at(row, 0x67c);
        if id as usize >= maximum || songs.contains_key(&id) { return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid or duplicate music ID")); }
        let mut levels = [0u8; 10]; levels.copy_from_slice(&row[0x3ec..0x3f6]);
        songs.insert(id, Song { title: nul_utf16(&row[..0x100]), genre: nul_utf16(&row[0x140..0x1c0]), artist: nul_utf16(&row[0x1c0..0x2c0]), levels });
        let _ = nul_utf8(&row[0x100..0x140]);
    }
    let sha256 = format!("{:x}", Sha256::digest(&data));
    Ok(Catalog { version, maximum, sha256, songs })
}

fn is_local_server(url: &str) -> bool {
    let authority = url.split_once("://").map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#']).next().unwrap_or("")
        .rsplit('@').next().unwrap_or("");
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host.eq_ignore_ascii_case("localhost") { return true; }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
        Err(_) => false,
    }
}

pub fn server_name(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if is_local_server(url) { "本地服务器".into() }
    else if lower.contains("maomani") { "Maomani".into() }
    else if lower.contains("firenet") { "Firenet".into() }
    else if lower.contains("bjmania") || lower.contains("bjm") { "BJM".into() }
    else if lower.contains("fervidex") { "Fervidex".into() }
    else if url.is_empty() { "未知服务器".into() }
    else { url.into() }
}

pub fn new_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}-{}", Local::now().format("%Y%m%d-%H%M%S-%6f"), std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed))
}
pub fn now() -> String { Local::now().format("%Y-%m-%d %H:%M:%S").to_string() }

pub fn merge(items: &[Collection], name: String) -> Result<Collection, String> {
    if items.len() < 2 { return Err("至少选择两份成绩".into()); }
    let hash = &items[0].catalog_sha256;
    if items.iter().any(|x| x.catalog_sha256 != *hash) { return Err("曲库索引不同，不能按曲目 ID 安全合并".into()); }
    if items.iter().any(|x| x.iidx_id.is_none()) { return Err("请先为所有待合并成绩设置 IIDX ID".into()); }
    let known_ids: std::collections::HashSet<&str> = items.iter().filter_map(|x| x.iidx_id.as_deref()).collect();
    if known_ids.len() > 1 { return Err("所选成绩的 IIDX ID 不同".into()); }
    let mut scores: BTreeMap<(u32, u8), Score> = BTreeMap::new();
    for item in items { for score in &item.scores {
        let key = (score.music, score.chart);
        match scores.get_mut(&key) {
            None => { scores.insert(key, score.clone()); }
            Some(old) => {
                if old.title != score.title || old.level != score.level || old.levels != score.levels { return Err(format!("曲目 {} 谱面信息冲突", score.music)); }
                if score.ex > old.ex { old.ex = score.ex; old.pgreat = score.pgreat; old.great = score.great; }
                if score.lamp > old.lamp { old.lamp = score.lamp; }
                old.bp = match (old.bp, score.bp) { (Some(a), Some(b)) => Some(a.min(b)), (Some(a), None) => Some(a), (None, Some(b)) => Some(b), _ => None };
            }
        }
    } }
    Ok(Collection { schema: 1, id: new_id(), name, server_url: "合并".into(), player_internal: String::new(), iidx_id: known_ids.into_iter().next().map(str::to_owned), nickname: items.iter().find_map(|x| x.nickname.clone()), captured_at: now(), catalog_sha256: hash.clone(), asphyxia_refid: None, parents: items.iter().map(|x| x.id.clone()).collect(), scores: scores.into_values().collect() })
}

pub fn fervidex_json(collection: &Collection) -> io::Result<Vec<u8>> {
    let mut songs: BTreeMap<String, BTreeMap<&str, serde_json::Value>> = BTreeMap::new();
    for s in &collection.scores {
        if s.chart >= 10 || s.lamp > 7 { return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid score")); }
        songs.entry(s.music.to_string()).or_default().insert(CHARTS[s.chart as usize], serde_json::json!({ "clear_type": s.lamp, "ex_score": s.ex, "miss_count": s.bp }));
    }
    // The "scores" object is the Fervidex static-profile body accepted by Tachi.
    serde_json::to_vec_pretty(&serde_json::json!({"scores": songs})).map_err(io::Error::other)
}

fn grade(ex: u32, notes: u32) -> &'static str {
    if notes == 0 { return "---"; }
    let n = ex as u64 * 9;
    let d = notes as u64 * 2;
    for (threshold, name) in [(8, "AAA"), (7, "AA"), (6, "A"), (5, "B"), (4, "C"), (3, "D"), (2, "E")] { if n >= d * threshold { return name; } }
    "F"
}
pub fn konami_csv(collection: &Collection, style: usize) -> io::Result<Vec<u8>> {
    let mut writer = csv::WriterBuilder::new().has_headers(false).from_writer(Vec::new());
    let mut header = vec!["バージョン".to_string(), "タイトル".into(), "ジャンル".into(), "アーティスト".into(), "プレー回数".into()];
    for diff in DIFFS { for field in ["難易度", "スコア", "PGreat", "Great", "ミスカウント", "クリアタイプ", "DJ LEVEL"] { header.push(format!("{diff} {field}")); } }
    header.push("最終プレー日時".into()); writer.write_record(header)?;
    let mut by_song: BTreeMap<u32, Vec<Option<&Score>>> = BTreeMap::new();
    for s in &collection.scores { if s.chart as usize / 5 == style { by_song.entry(s.music).or_insert_with(|| vec![None; 5])[s.chart as usize % 5] = Some(s); } }
    for (music, charts) in by_song {
        let first = (*charts.iter().flatten().next().unwrap()).clone();
        let mut row = vec![(music / 1000).to_string(), first.title.clone(), first.genre.clone(), first.artist.clone(), String::new()];
        for (index, maybe) in charts.into_iter().enumerate() { if let Some(s) = maybe {
            row.extend([s.level.to_string(), s.ex.to_string(), s.pgreat.map(|n| n.to_string()).unwrap_or_default(), s.great.map(|n| n.to_string()).unwrap_or_default(), s.bp.map(|n| n.to_string()).unwrap_or_else(|| "---".into()), LAMPS[s.lamp as usize].into(), grade(s.ex, 0).into()]);
        } else { row.extend([first.levels[style * 5 + index].to_string(), "0".into(), String::new(), String::new(), "---".into(), "NO PLAY".into(), "---".into()]); } }
        row.push(String::new()); writer.write_record(row)?;
    }
    let bytes = writer.into_inner().map_err(|e| io::Error::other(e.into_error()))?;
    let mut with_bom = Vec::with_capacity(bytes.len() + 3);
    with_bom.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    with_bom.extend_from_slice(&bytes);
    Ok(with_bom)
}

pub fn write_collection(root: &Path, collection: &Collection) -> io::Result<PathBuf> {
    let dir = root.join(&collection.id);
    fs::create_dir_all(&dir)?;
    let manifest = serde_json::to_vec_pretty(collection)?;
    atomic_write(&dir.join("manifest.json"), &manifest)?;
    atomic_write(&dir.join("fervidex.json"), &fervidex_json(collection)?)?;
    atomic_write(&dir.join("konami_sp.csv"), &konami_csv(collection, 0)?)?;
    atomic_write(&dir.join("konami_dp.csv"), &konami_csv(collection, 1)?)?;
    atomic_write(&dir.join("asphyxia.db"), &crate::formats::asphyxia_db(collection)?)?;
    Ok(dir)
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" { fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32; }
        let from: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Replace the old export atomically when editing a collection on Windows.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
            let error = io::Error::last_os_error();
            let _ = fs::remove_file(&tmp);
            return Err(error);
        }
    }
    #[cfg(not(windows))] fs::rename(&tmp, path)?;
    Ok(())
}
pub fn load_collections(root: &Path) -> io::Result<Vec<Collection>> {
    let mut all = Vec::new();
    if !root.exists() { return Ok(all); }
    for dir in fs::read_dir(root)? { let path = dir?.path().join("manifest.json"); if path.is_file() {
        if let Ok(item) = serde_json::from_slice::<Collection>(&fs::read(path)?) { if item.schema == 1 { all.push(item); } }
    } }
    all.sort_by(|a,b| b.captured_at.cmp(&a.captured_at)); Ok(all)
}

pub fn archive_collection(root: &Path, id: &str) -> io::Result<PathBuf> {
    use std::path::Component;
    if !matches!(Path::new(id).components().collect::<Vec<_>>().as_slice(), [Component::Normal(_)]) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid collection ID"));
    }
    let source = root.join(id);
    if !source.join("manifest.json").is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "collection not found"));
    }
    let deleted = root.join(".deleted");
    fs::create_dir_all(&deleted)?;
    let destination = deleted.join(format!("{id}-{}", new_id()));
    fs::rename(source, &destination)?;
    Ok(destination)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn server_name_recognizes_local_addresses_and_maomani() {
        for url in ["http://localhost:8083/", "http://127.0.0.1:8083/", "http://192.168.1.12/core/", "http://10.1.2.3/", "http://172.16.0.1/", "http://[::1]:8083/", "http://[fd00::1]/"] {
            assert_eq!(server_name(url), "本地服务器", "{url}");
        }
        assert_eq!(server_name("http://maomani.cn:573/"), "Maomani");
        assert_eq!(server_name("https://example.com/core/maomani"), "Maomani");
        assert_eq!(server_name("https://example.com/localhost"), "https://example.com/localhost");
        assert_eq!(server_name("http://8.8.8.8/"), "http://8.8.8.8/");
    }
    fn collection(id: &str, ex: u32, bp: Option<u32>, lamp: u8) -> Collection {
        Collection { schema: 1, id: id.into(), name: id.into(), server_url: "".into(), player_internal: "1".into(), iidx_id: Some("1234-5678".into()), nickname: Some("DJ".into()), captured_at: now(), catalog_sha256: "same".into(), asphyxia_refid: None, parents: vec![], scores: vec![Score { music: 1000, chart: 3, ex, bp, lamp, title: "Song".into(), genre: "Genre".into(), artist: "Artist".into(), level: 10, levels: [0,0,0,10,0,0,0,0,0,0], pgreat: None, great: None }] }
    }
    #[test] fn merge_independent_bests() {
        let a = collection("a", 1500, Some(30), 5); let b = collection("b", 1400, Some(20), 6);
        let m = merge(&[a,b], "合并".into()).unwrap(); assert_eq!(m.scores[0].ex,1500); assert_eq!(m.scores[0].bp,Some(20)); assert_eq!(m.scores[0].lamp,6); assert_eq!(m.parents.len(),2);
    }
    #[test] fn merge_rejects_unknown_identity() {
        let a = collection("a", 1500, Some(30), 5);
        let mut b = collection("b", 1400, Some(20), 6); b.iidx_id = None;
        assert!(merge(&[a,b], "合并".into()).is_err());
    }
    #[test] fn static_json_schema() {
        let value: serde_json::Value = serde_json::from_slice(&fervidex_json(&collection("a",1500,None,5)).unwrap()).unwrap();
        assert_eq!(value["scores"]["1000"]["spa"]["ex_score"],1500);
    }
    #[test] fn konami_csv_header_and_unknown_fields() {
        let bytes = konami_csv(&collection("a", 1500, None, 5), 0).unwrap();
        assert!(bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        let text = std::str::from_utf8(&bytes[3..]).unwrap();
        let mut rows = csv::Reader::from_reader(text.as_bytes());
        assert_eq!(rows.headers().unwrap().len(), 41);
        let row = rows.records().next().unwrap().unwrap();
        assert_eq!(row.get(1), Some("Song"));
        assert_eq!(row.get(5 + 3 * 7 + 1), Some("1500"));
        assert_eq!(row.get(5 + 3 * 7 + 2), Some(""));
    }
    #[test] fn repeated_capture_replaces_same_collection() {
        let root = std::env::temp_dir().join(format!("anytofire-test-{}", new_id()));
        let mut entry = collection("session", 1000, Some(30), 4);
        write_collection(&root, &entry).unwrap();
        entry.scores[0].ex = 1500;
        write_collection(&root, &entry).unwrap();
        let saved = load_collections(&root).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].scores[0].ex, 1500);
        fs::remove_dir_all(root).unwrap();
    }
    #[test] fn deleted_collection_is_hidden_and_recoverable() {
        let root = std::env::temp_dir().join(format!("anytofire-delete-test-{}", new_id()));
        write_collection(&root, &collection("session", 1000, None, 4)).unwrap();
        assert!(archive_collection(&root, "../session").is_err());
        let archived = archive_collection(&root, "session").unwrap();
        assert!(archived.join("manifest.json").is_file());
        assert!(load_collections(&root).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test] fn installed_catalog_layout_when_available() {
        let path = Path::new(r"C:\IIDX\data\info\0\music_data.bin");
        if !path.exists() { return; }
        let catalog = read_catalog(path).unwrap();
        assert_eq!(catalog.version, 34);
        assert!(catalog.songs.len() > 1000);
        let first = catalog.songs.get(&1000).unwrap();
        assert_eq!(first.title, "5.1.1.");
        assert!(!first.genre.is_empty());
        assert!(!first.artist.is_empty());
    }
    #[test] fn simultaneous_players_get_distinct_export_directories() {
        assert_ne!(new_id(), new_id());
    }
}
