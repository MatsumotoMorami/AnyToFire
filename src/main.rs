#![windows_subsystem = "windows"]

use anytofire::core::{self, Collection};
use anytofire::formats;
use anytofire::push;
use anytofire::layout;
use base64::Engine;
use serde_json::{json, Value};
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use rusqlite::Connection;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::platform::windows::WindowExtWindows;
use winit::window::{Icon, Window, WindowId};
use wry::{WebView, WebViewBuilder};

type Hwnd = *mut c_void;
const OFN_OVERWRITEPROMPT: u32 = 2;
const OFN_FILEMUSTEXIST: u32 = 0x1000;
const OFN_PATHMUSTEXIST: u32 = 0x800;

#[repr(C)]
struct OpenFileName {
    struct_size: u32, owner: Hwnd, instance: Hwnd, filter: *const u16,
    custom_filter: *mut u16, max_custom_filter: u32, filter_index: u32,
    file: *mut u16, max_file: u32, file_title: *mut u16, max_file_title: u32,
    initial_dir: *const u16, title: *const u16, flags: u32,
    file_offset: u16, file_extension: u16, default_ext: *const u16,
    cust_data: isize, hook: *mut c_void, template_name: *const u16,
    reserved: *mut c_void, reserved_dword: u32, flags_ex: u32,
}
#[link(name = "comdlg32")]
extern "system" {
    fn GetSaveFileNameW(ofn: *mut OpenFileName) -> i32;
    fn GetOpenFileNameW(ofn: *mut OpenFileName) -> i32;
}
#[link(name = "user32")]
extern "system" { fn MessageBoxW(hwnd: Hwnd, text: *const u16, title: *const u16, flags: u32) -> i32; }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
const PLUGIN_DLL: &str = r"AnyToFire\AnyToFire.dll";
const LEGACY_PLUGIN_DLL: &str = "AnyToFire.dll";
fn is_plugin_hook(name: &str) -> bool {
    name.eq_ignore_ascii_case(PLUGIN_DLL) || name.eq_ignore_ascii_case(LEGACY_PLUGIN_DLL)
}
fn is_spice_launch_line(line: &str) -> bool {
    let line = line.trim_start().to_ascii_lowercase();
    line.contains("spice64.exe") && !["rem ", "::", "echo ", "set ", "if ", "title "]
        .iter().any(|prefix| line.starts_with(prefix))
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum HookSource { Cfg, Bat }
impl HookSource {
    fn as_str(self) -> &'static str { if self == Self::Cfg { "cfg" } else { "bat" } }
    fn parse(value: &str) -> Result<Self, String> {
        match value { "cfg" => Ok(Self::Cfg), "bat" => Ok(Self::Bat), _ => Err("请选择 cfg 配置或 start.bat 启动".into()) }
    }
}
fn source_path(game_dir: &Path) -> PathBuf { game_dir.join("AnyToFire_launch.json") }
fn load_source(game_dir: &Path) -> Result<Option<HookSource>, String> {
    let path = source_path(game_dir);
    if !path.exists() { return Ok(None); }
    let value: Value = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| format!("启动方式配置损坏: {e}"))?;
    Ok(Some(HookSource::parse(value["source"].as_str().unwrap_or(""))?))
}
fn save_source(game_dir: &Path, source: HookSource) -> Result<(), String> {
    fs::write(source_path(game_dir), json!({"source":source.as_str()}).to_string()).map_err(|e| format!("保存启动方式失败: {e}"))
}
fn hook_line_range(line: &str) -> Option<(usize, usize)> {
    if !is_spice_launch_line(line) { return None; }
    let marker = "-k \"";
    let begin = line.find(marker)? + marker.len();
    let end = begin + line[begin..].find('"')?;
    Some((begin, end))
}
fn spice_config_path() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA").ok_or("找不到 APPDATA 目录")?;
    Ok(PathBuf::from(appdata).join("spicetools.xml"))
}
fn xml_hook_range(xml: &str) -> Option<(usize, usize)> {
    let game_begin = xml.find("<game name=\"Beatmania IIDX\">")?;
    let game_end = game_begin + xml[game_begin..].find("</game>")?;
    let game = &xml[game_begin..game_end];
    let option_begin = game.find("<option name=\"k\" value=\"")? + "<option name=\"k\" value=\"".len();
    let option_end = option_begin + game[option_begin..].find('"')?;
    Some((game_begin + option_begin, game_begin + option_end))
}
fn ensure_xml_hook(mut xml: String) -> Result<(String, (usize, usize)), String> {
    if let Some(range) = xml_hook_range(&xml) { return Ok((xml, range)); }
    let game_begin = xml.find("<game name=\"Beatmania IIDX\">").ok_or("spicetools.xml 中找不到 Beatmania IIDX 配置")?;
    let game_end = game_begin + xml[game_begin..].find("</game>").ok_or("IIDX 的 cfg 配置不完整")?;
    let options_end = game_begin + xml[game_begin..game_end].find("</options>").ok_or("IIDX 的 cfg 配置中找不到 options")?;
    xml.insert_str(options_end, "    <option name=\"k\" value=\"\"/>\n        ");
    let range = xml_hook_range(&xml).ok_or("无法添加 IIDX 的 k 配置")?;
    Ok((xml, range))
}
fn batch_hook_range(batch: &str) -> Option<(usize, usize)> {
    batch.split_inclusive('\n').scan(0usize, |offset, line| {
        let current = *offset; *offset += line.len(); Some((current, line))
    }).find_map(|(offset, line)| hook_line_range(line).map(|(begin,end)| (offset+begin,offset+end)))
}
fn ensure_batch_hook(mut batch: String) -> Result<(String, (usize, usize)), String> {
    if let Some(range) = batch_hook_range(&batch) { return Ok((batch, range)); }
    let start = batch.split_inclusive('\n').scan(0usize, |offset, line| {
        let current = *offset; *offset += line.len(); Some((current,line))
    }).find_map(|(offset,line)| is_spice_launch_line(line).then_some(offset))
      .ok_or("start.bat 中找不到 spice64.exe 启动行")?;
    let end = start + batch[start..].find('\n').unwrap_or(batch.len()-start);
    let at = batch[start..end].find("%*").map(|x| start+x).unwrap_or_else(|| if end>start && batch.as_bytes()[end-1]==b'\r' { end-1 } else { end });
    batch.insert_str(at, "-k \"\" ");
    let range = batch_hook_range(&batch).ok_or("无法添加 start.bat 的 -k 参数")?;
    Ok((batch, range))
}
fn update_hook_list(content: &str, range: (usize, usize), enabled: bool, remove_legacy: bool) -> String {
    let (begin, end) = range;
    let mut hooks: Vec<&str> = content[begin..end].split_whitespace()
        .filter(|name| !is_plugin_hook(name) && (!remove_legacy || !name.eq_ignore_ascii_case("IIDXScore.dll"))).collect();
    if enabled { hooks.push(PLUGIN_DLL); }
    format!("{}{}{}", &content[..begin], hooks.join(" "), &content[end..])
}
fn plugin_enabled(game_dir: &Path) -> Result<bool, String> {
    let source = match load_source(game_dir)? { Some(source) => source, None if !initialization_marker(game_dir).is_file() => return Ok(false), None => HookSource::Cfg };
    let content = match source {
        HookSource::Cfg => { let path=spice_config_path()?; fs::read_to_string(&path).map_err(|e| format!("无法读取 {}: {e}",path.display()))? },
        HookSource::Bat => { let path=game_dir.join("start.bat"); fs::read_to_string(&path).map_err(|e| format!("无法读取 {}: {e}",path.display()))? },
    };
    let range = match source { HookSource::Cfg => xml_hook_range(&content), HookSource::Bat => batch_hook_range(&content) };
    Ok(range.is_some_and(|(begin,end)| content[begin..end].split_whitespace().any(is_plugin_hook)))
}
fn migrate_legacy_hook(game_dir: &Path) -> Result<(), String> {
    let candidates = [spice_config_path().ok().map(|path| (HookSource::Cfg, path)), Some((HookSource::Bat, game_dir.join("start.bat")))];
    for (source, path) in candidates.into_iter().flatten() {
        if !path.is_file() { continue; }
        let content = fs::read_to_string(&path).map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
        let range = match source { HookSource::Cfg => xml_hook_range(&content), HookSource::Bat => batch_hook_range(&content) };
        let Some((begin,end)) = range else { continue; };
        if !content[begin..end].split_whitespace().any(|name| name.eq_ignore_ascii_case(LEGACY_PLUGIN_DLL)) { continue; }
        let updated = update_hook_list(&content, (begin,end), true, false);
        let backup = match source { HookSource::Cfg => path.with_extension("xml.AnyToFire.bak"), HookSource::Bat => game_dir.join("start.bat.AnyToFire.bak") };
        fs::copy(&path, backup).map_err(|e| format!("备份启动配置失败: {e}"))?;
        fs::write(&path, updated).map_err(|e| format!("迁移 AnyToFire 注入路径失败: {e}"))?;
    }
    Ok(())
}
fn set_plugin_enabled(game_dir: &Path, enabled: bool) -> Result<(), String> {
    set_plugin_hooks(game_dir, enabled, false)
}
fn set_plugin_hooks(game_dir: &Path, enabled: bool, remove_legacy: bool) -> Result<(), String> {
    let source = match load_source(game_dir)? { Some(source)=>source, None if initialization_marker(game_dir).is_file()=>HookSource::Cfg, None=>return Err("请先选择启动配置来源".into()) };
    let path = match source { HookSource::Cfg => spice_config_path()?, HookSource::Bat => game_dir.join("start.bat") };
    let original = fs::read_to_string(&path).map_err(|e| format!("无法读取 {}: {e}",path.display()))?;
    let (content, range) = match source { HookSource::Cfg => ensure_xml_hook(original.clone())?, HookSource::Bat => ensure_batch_hook(original.clone())? };
    let updated = update_hook_list(&content, range, enabled, remove_legacy);
    if updated != original {
        let backup = match source { HookSource::Cfg => path.with_extension("xml.AnyToFire.bak"), HookSource::Bat => game_dir.join("start.bat.AnyToFire.bak") };
        fs::copy(&path,backup).map_err(|e| format!("备份启动配置失败: {e}"))?;
        fs::write(&path,updated).map_err(|e| format!("更新启动配置失败: {e}"))?;
    }
    Ok(())
}
fn initialization_marker(game_dir: &Path) -> PathBuf { game_dir.join("AnyToFire_initialized.json") }
fn game_version(game_dir: &Path) -> Result<u32, String> {
    core::read_catalog(&game_dir.join("data/info/0/music_data.bin"))
        .map(|catalog| catalog.version).map_err(|e| format!("无法识别游戏曲库版本: {e}"))
}
fn score_ini_path(game_dir: &Path) -> PathBuf { game_dir.join("IIDXScore.ini") }
fn score_ini_value(game_dir: &Path, key: &str) -> Option<String> {
    let content = fs::read_to_string(score_ini_path(game_dir)).ok()?;
    let mut score_section = false;
    let mut value = None;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') { score_section = line.eq_ignore_ascii_case("[Score]"); }
        else if score_section && !line.starts_with(';') && !line.starts_with('#') {
            if let Some((name, raw)) = line.split_once('=') {
                if name.trim().eq_ignore_ascii_case(key) { value = Some(raw.trim().to_owned()); }
            }
        }
    }
    value
}
fn set_score_ini_value(content: &str, key: &str, value: &str, replace: bool) -> String {
    let mut lines: Vec<String> = content.lines().map(str::to_owned).collect();
    let section = lines.iter().position(|line| line.trim().eq_ignore_ascii_case("[Score]"));
    let start = if let Some(index) = section { index + 1 } else { lines.push("[Score]".into()); lines.len() };
    let end = start + lines[start..].iter().position(|line| line.trim_start().starts_with('[')).unwrap_or(lines.len() - start);
    if let Some(index) = (start..end).find(|&i| lines[i].split_once('=').is_some_and(|(name,_)| name.trim().eq_ignore_ascii_case(key))) {
        if replace { lines[index] = format!("{key}={value}"); }
    } else { lines.insert(end, format!("{key}={value}")); }
    lines.join("\r\n") + "\r\n"
}
fn prepare_score(game_dir: &Path) -> Result<bool, String> {
    const OLD_A: &[u8] = include_bytes!("../assets/music_omni_33_a.bin");
    const OLD_B: &[u8] = include_bytes!("../assets/music_omni_33_b.bin");
    const FINAL_OMNI: &[u8] = include_bytes!("../assets/music_omni_33_final.bin");
    const OLD_AC: &[u8] = include_bytes!("../assets/music_ac_33.bin");
    const FINAL_34_OMNI: &[u8] = include_bytes!("../assets/music_omni_34_0a.bin");
    let old_database = score_ini_value(game_dir, "DatabasePath").unwrap_or_else(|| "scores/IIDXScore.sqlite3".into());
    let db_path = if Path::new(&old_database).is_absolute() { PathBuf::from(&old_database) } else { game_dir.join(&old_database) };
    let migration_needed = db_path.is_file() && fs::metadata(&db_path).map(|m| m.len() > 0).unwrap_or(false);
    let data_dir = game_dir.join("AnyToFire_data");
    fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    for (name, bytes) in [("music_omni_33_a.bin", OLD_A), ("music_omni_33_b.bin", OLD_B), ("music_omni_33_final.bin", FINAL_OMNI), ("music_ac_33.bin", OLD_AC), ("music_omni_34_0a.bin", FINAL_34_OMNI)] {
        let path = data_dir.join(name);
        if !path.is_file() || fs::metadata(&path).map(|m| m.len() != bytes.len() as u64).unwrap_or(true) {
            fs::write(&path, bytes).map_err(|e| format!("写入旧曲库索引失败: {e}"))?;
        }
    }
    let ini_path = score_ini_path(game_dir);
    let original = fs::read_to_string(&ini_path).unwrap_or_default();
    let mut ini = original.clone();
    for (key, value) in [
        ("DatabasePath", old_database.as_str()),
        ("MusicDatabase", "data/info/0/music_data.bin"),
        ("LegacyMusicDatabase", "AnyToFire_data/music_omni_33_a.bin"),
        ("LegacyMusicDatabase2", "AnyToFire_data/music_omni_33_b.bin"),
        ("LegacyMusicDatabase3", "AnyToFire_data/music_omni_33_final.bin"),
        ("LegacyMusicDatabase4", "AnyToFire_data/music_ac_33.bin"),
    ] {
        ini = set_score_ini_value(&ini, key, value, key.starts_with("Legacy"));
    }
    let mut valid_configured_omni = false;
    if let Some(omni) = score_ini_value(game_dir, "OmniMusicDatabase").filter(|value| !value.is_empty()) {
        let path = if Path::new(&omni).is_absolute() { PathBuf::from(&omni) } else { game_dir.join(&omni) };
        valid_configured_omni = core::read_catalog(&path).is_ok_and(|catalog| catalog.version == game_version(game_dir).unwrap_or(0));
    }
    if !valid_configured_omni {
        let default_omni = match game_version(game_dir)? {
            33 => "AnyToFire_data/music_omni_33_final.bin",
            34 => "AnyToFire_data/music_omni_34_0a.bin",
            other => return Err(format!("不支持的游戏版本: {other}")),
        };
        ini = set_score_ini_value(&ini, "OmniMusicDatabase", default_omni, true);
    }
    if ini != original {
        if ini_path.exists() { fs::copy(&ini_path, game_dir.join("IIDXScore.ini.AnyToFire.bak")).map_err(|e| e.to_string())?; }
        fs::write(&ini_path, ini).map_err(|e| format!("写入 IIDXScore.ini 失败: {e}"))?;
    }
    Ok(migration_needed)
}
fn migrate_score(game_dir: &Path) -> Result<(), String> {
    let config = anytofire::score::config(game_dir)?;
    if config.database.is_file() {
        let db = Connection::open(&config.database).map_err(|e| e.to_string())?;
        let backup = config.database.with_extension("sqlite3.AnyToFire-pre-migration.bak");
        if !backup.exists() {
            let escaped = backup.to_string_lossy().replace('\'', "''");
            db.execute_batch(&format!("VACUUM INTO '{escaped}'")).map_err(|e| format!("备份旧成绩失败: {e}"))?;
        }
    }
    anytofire::score::Store::open(config).map(|_| ())
}
#[cfg(test)]
mod hook_tests {
    use super::*;
    #[test]
    fn xml_hook_list_changes_only_iidx() {
        let xml = "<games><game name=\"Beatmania IIDX\"><options><option name=\"k\" value=\"ifs_hook.dll IIDXRecorder.dll\"/></options></game><game name=\"Other\"><option name=\"k\" value=\"Other.dll\"/></game></games>";
        let enabled = update_hook_list(xml, xml_hook_range(xml).unwrap(), true, false);
        assert!(enabled.contains("value=\"ifs_hook.dll IIDXRecorder.dll AnyToFire\\AnyToFire.dll\""));
        assert!(enabled.contains("value=\"Other.dll\""));
        let disabled = update_hook_list(&enabled, xml_hook_range(&enabled).unwrap(), false, false);
        assert_eq!(disabled, xml);
    }
    #[test]
    fn missing_iidx_k_is_added_without_changing_other_game() {
        let xml = "<games><game name=\"Beatmania IIDX\"><options><option name=\"url\" value=\"https://example/core/\"/></options></game><game name=\"Other\"><options><option name=\"k\" value=\"Other.dll\"/></options></game></games>";
        let (xml, range) = ensure_xml_hook(xml.to_owned()).unwrap();
        let updated = update_hook_list(&xml, range, true, false);
        assert!(updated.contains("<option name=\"k\" value=\"AnyToFire\\AnyToFire.dll\"/>"));
        assert!(updated.contains("value=\"Other.dll\""));
    }
    #[test]
    fn missing_batch_k_is_added_to_launch_command() {
        let batch = "@echo off\r\n\".\\spice64.exe\" -cfgpath \"config.xml\" %*\r\n";
        let (batch, range) = ensure_batch_hook(batch.to_owned()).unwrap();
        let updated = update_hook_list(&batch, range, true, false);
        assert!(updated.contains("-cfgpath \"config.xml\" -k \"AnyToFire\\AnyToFire.dll\" %*"));
    }
    #[test]
    fn batch_hook_list_preserves_other_arguments() {
        let line = "\".\\spice64.exe\" -cfgpath \"config.xml\" -k \"ifs_hook.dll IIDXRecorder.dll\" %*";
        let updated = update_hook_list(line, hook_line_range(line).unwrap(), true, false);
        assert_eq!(updated, "\".\\spice64.exe\" -cfgpath \"config.xml\" -k \"ifs_hook.dll IIDXRecorder.dll AnyToFire\\AnyToFire.dll\" %*");
    }
    #[test]
    fn initialization_replaces_legacy_hook_without_touching_others() {
        let line = "\".\\spice64.exe\" -k \"ifs_hook.dll IIDXScore.dll IIDXRecorder.dll\" %*";
        let updated = update_hook_list(line, hook_line_range(line).unwrap(), true, true);
        assert_eq!(updated, "\".\\spice64.exe\" -k \"ifs_hook.dll IIDXRecorder.dll AnyToFire\\AnyToFire.dll\" %*");
    }
    #[test]
    fn existing_root_hook_is_replaced_without_duplicate_injection() {
        let line = "\".\\spice64.exe\" -k \"ifs_hook.dll AnyToFire.dll IIDXRecorder.dll\" %*";
        let updated = update_hook_list(line, hook_line_range(line).unwrap(), true, false);
        assert_eq!(updated, "\".\\spice64.exe\" -k \"ifs_hook.dll IIDXRecorder.dll AnyToFire\\AnyToFire.dll\" %*");
    }
    #[test]
    fn score_ini_keeps_database_and_server_settings() {
        let original = "[Score]\r\nDatabasePath=scores/custom.sqlite3\r\nServerKey=firenet\r\nLegacyMusicDatabase=old.bin\r\n[Other]\r\nFoo=bar\r\n";
        let updated = set_score_ini_value(original, "LegacyMusicDatabase", "AnyToFire_data/music_omni_33_a.bin", true);
        assert!(updated.contains("DatabasePath=scores/custom.sqlite3"));
        assert!(updated.contains("ServerKey=firenet"));
        assert!(updated.contains("LegacyMusicDatabase=AnyToFire_data/music_omni_33_a.bin\r\n[Other]"));
        assert!(!updated.contains("LegacyMusicDatabase=old.bin"));
    }
    #[test]
    fn preparation_installs_legacy_indexes_and_34_omni() {
        let dir = std::env::temp_dir().join(format!("atf-init-{}", core::new_id()));
        fs::create_dir_all(dir.join("data/info/0")).unwrap();
        let catalog = Path::new("C:/IIDX/data/info/0/music_data.bin");
        if !catalog.is_file() { return; }
        fs::copy(catalog, dir.join("data/info/0/music_data.bin")).unwrap();
        fs::write(dir.join("IIDXScore.ini"), "[Score]\nServerKey=firenet\nDatabasePath=scores/custom.sqlite3\nOmniMusicDatabase=missing/music_omni.bin\n").unwrap();
        assert!(!prepare_score(&dir).unwrap());
        for name in ["music_ac_33.bin", "music_omni_33_a.bin", "music_omni_33_b.bin", "music_omni_33_final.bin", "music_omni_34_0a.bin"] {
            assert!(dir.join("AnyToFire_data").join(name).is_file());
        }
        let config = anytofire::score::config(&dir).unwrap();
        assert!(config.migrations.len() >= 4);
        assert_eq!(score_ini_value(&dir, "OmniMusicDatabase").as_deref(), Some("AnyToFire_data/music_omni_34_0a.bin"));
        assert_eq!(config.database, dir.join("scores/custom.sqlite3"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn preparation_uses_33_omni_for_33_catalog() {
        let dir = std::env::temp_dir().join(format!("atf-init-33-{}", core::new_id()));
        fs::create_dir_all(dir.join("data/info/0")).unwrap();
        fs::write(dir.join("data/info/0/music_data.bin"), include_bytes!("../assets/music_ac_33.bin")).unwrap();
        assert_eq!(game_version(&dir).unwrap(), 33);
        prepare_score(&dir).unwrap();
        assert_eq!(score_ini_value(&dir, "OmniMusicDatabase").as_deref(), Some("AnyToFire_data/music_omni_33_final.bin"));
        assert_eq!(anytofire::score::config(&dir).unwrap().game_version, 33);
        fs::remove_dir_all(dir).unwrap();
    }
}
fn trace(message: &str) {
    if let Some(path) = std::env::var_os("ANYTOFIRE_TRACE") {
        use std::io::Write;
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{} {message}", core::now());
        }
    }
}
fn file_path(suggested: &str, ext: &str, open: bool) -> Option<PathBuf> {
    let mut file = vec![0u16; 32768];
    let initial = wide(suggested);
    let count = initial.len().min(file.len());
    file[..count].copy_from_slice(&initial[..count]);
    let filter = if open {
        wide("成绩文件 (*.json;*.csv;*.db)\0*.json;*.csv;*.db\0所有文件 (*.*)\0*.*\0")
    } else { match ext {
        "json" => wide("JSON 文件 (*.json)\0*.json\0"),
        "csv" => wide("CSV 文件 (*.csv)\0*.csv\0"),
        _ => wide("Asphyxia DB (*.db)\0*.db\0"),
    }};
    let title = wide(if open { "选择要导入的成绩" } else { "选择另存为位置" });
    let default_ext = wide(ext);
    let mut ofn = OpenFileName {
        struct_size: std::mem::size_of::<OpenFileName>() as u32,
        owner: std::ptr::null_mut(), instance: std::ptr::null_mut(),
        filter: filter.as_ptr(), custom_filter: std::ptr::null_mut(), max_custom_filter: 0,
        filter_index: 1, file: file.as_mut_ptr(), max_file: file.len() as u32,
        file_title: std::ptr::null_mut(), max_file_title: 0, initial_dir: std::ptr::null(),
        title: title.as_ptr(), flags: OFN_PATHMUSTEXIST | if open { OFN_FILEMUSTEXIST } else { OFN_OVERWRITEPROMPT },
        file_offset: 0, file_extension: 0, default_ext: default_ext.as_ptr(), cust_data: 0,
        hook: std::ptr::null_mut(), template_name: std::ptr::null(), reserved: std::ptr::null_mut(),
        reserved_dword: 0, flags_ex: 0,
    };
    let result = unsafe { if open { GetOpenFileNameW(&mut ofn) } else { GetSaveFileNameW(&mut ofn) } };
    if result == 0 { return None; }
    let n = file.iter().position(|x| *x == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&file[..n])))
}

#[derive(Debug)]
enum Event { Command(Value) }

struct App { root: PathBuf, game_dir: PathBuf, window: Option<Window>, webview: Option<WebView>, proxy: EventLoopProxy<Event> }
impl App {
    fn send(&self, value: Value) {
        if let Some(webview) = &self.webview {
            let _ = webview.evaluate_script(&format!("window.anytofireReceive({value});"));
        }
    }
    fn items(&self) -> Result<Vec<Collection>, String> { core::load_collections(&self.root).map_err(|e| e.to_string()) }
    fn refresh(&self) -> Result<(), String> {
        let rows: Vec<Value> = self.items()?.iter().map(|item| {
            let source = if !item.parents.is_empty() { "合并".to_owned() } else { core::server_name(&item.server_url) };
            let sp = item.scores.iter().filter(|s| s.chart < 5).count();
            json!({ "id": item.id, "name": item.name, "source": source,
                "url": item.server_url, "player": item.nickname.as_deref().unwrap_or("—"),
                "iidx": item.iidx_id.as_deref().unwrap_or("—"), "charts": item.scores.len(),
                "sp": sp, "dp": item.scores.len() - sp, "date": item.captured_at,
                "kind": if item.parents.is_empty() { "原始导出" } else { "合并结果" },
                "parents": item.parents })
        }).collect();
        self.send(json!({"type":"items", "items":rows}));
        let recent=push::recent_rows(self.game_dir()).map_err(|e| e.to_string())?;
        self.send(json!({"type":"recent", "items":recent})); Ok(())
    }
    fn game_dir(&self) -> &Path { &self.game_dir }
    fn send_settings(&self) -> Result<(), String> {
        let settings=push::load_settings(self.game_dir()).map_err(|e| e.to_string())?;
        self.send(json!({"type":"settings","autoSync":settings.auto_sync,"hasAccessCode":!settings.access_code.is_empty(),"omniOverlay":settings.omni_overlay,"pluginEnabled":plugin_enabled(self.game_dir())?,"hookSource":load_source(self.game_dir())?.unwrap_or(HookSource::Cfg).as_str()})); Ok(())
    }
    fn send_hook_state(&self) -> Result<(), String> {
        self.send(json!({"type":"hook_state","pluginEnabled":plugin_enabled(self.game_dir())?,"hookSource":load_source(self.game_dir())?.unwrap_or(HookSource::Cfg).as_str()})); Ok(())
    }
    fn item(&self, id: &str) -> Result<Collection, String> {
        self.items()?.into_iter().find(|x| x.id == id).ok_or_else(|| "找不到这份成绩".into())
    }
    fn notify(&self, message: &str) { self.send(json!({"type":"toast","message":message})); }
    fn command(&self, request: Value) -> Result<(), String> {
        match request["action"].as_str().unwrap_or("") {
            "ready" => {
                migrate_legacy_hook(self.game_dir())?;
                self.refresh()?; self.send_settings()?;
                let has_access_code = !push::load_settings(self.game_dir()).map_err(|e| e.to_string())?.access_code.is_empty();
                let needs_initialization = !initialization_marker(self.game_dir()).is_file()
                    || !self.game_dir().join("AnyToFire_data/music_omni_33_final.bin").is_file()
                    || !self.game_dir().join("AnyToFire_data/music_omni_34_0a.bin").is_file();
                self.send(json!({"type":"initialization","needed":needs_initialization,"hasAccessCode":has_access_code,"version":game_version(self.game_dir())?}));
                Ok(())
            },
            "refresh" => { self.refresh()?; self.send_settings() },
            "init_prepare" => {
                if load_source(self.game_dir())?.is_none() { return Err("请先选择 cfg 配置或 start.bat 启动".into()); }
                if !matches!(game_version(self.game_dir())?, 33 | 34) { return Err("当前游戏版本不受支持，未修改 IIDXScore 或注入配置".into()); }
                let migration_needed = prepare_score(self.game_dir())?;
                self.send(json!({"type":"init_prepared","migrationNeeded":migration_needed})); Ok(())
            }
            "init_migrate" => {
                migrate_score(self.game_dir())?;
                self.send(json!({"type":"init_migrated"})); Ok(())
            }
            "init_inject" => {
                if !matches!(game_version(self.game_dir())?, 33 | 34) { return Err("当前游戏版本不受支持，未改动注入配置".into()); }
                set_plugin_hooks(self.game_dir(), true, true)?;
                fs::write(initialization_marker(self.game_dir()), json!({"version":game_version(self.game_dir())?,"completedAt":core::now()}).to_string())
                    .map_err(|e| format!("保存初始化状态失败: {e}"))?;
                self.send_settings()?;
                self.send(json!({"type":"init_complete"})); Ok(())
            }
            "set_sync" => {
                let mut settings=push::load_settings(self.game_dir()).map_err(|e| e.to_string())?;
                if let Some(code)=request["accessCode"].as_str() { if !code.trim().is_empty() { settings.access_code=code.trim().to_owned(); } }
                if let Some(enabled)=request["autoSync"].as_bool() { settings.auto_sync=enabled; }
                if let Some(enabled)=request["omniOverlay"].as_bool() { settings.omni_overlay=enabled; }
                if settings.auto_sync && settings.access_code.is_empty() { return Err("请先填写火网第三方访问码".into()); }
                push::save_settings(self.game_dir(),&settings).map_err(|e| e.to_string())?;
                self.send_settings()?; self.notify("设置已保存；分数显示开关在下次启动游戏时生效"); Ok(())
            }
            "set_hook_source" => {
                let source=HookSource::parse(request["source"].as_str().unwrap_or(""))?;
                let path=match source { HookSource::Cfg=>spice_config_path()?, HookSource::Bat=>self.game_dir().join("start.bat") };
                let original=fs::read_to_string(&path).map_err(|e| format!("无法读取 {}: {e}",path.display()))?;
                let content=match source { HookSource::Cfg=>ensure_xml_hook(original.clone())?.0, HookSource::Bat=>ensure_batch_hook(original.clone())?.0 };
                if content != original {
                    let backup=match source { HookSource::Cfg=>path.with_extension("xml.AnyToFire.bak"), HookSource::Bat=>self.game_dir().join("start.bat.AnyToFire.bak") };
                    fs::copy(&path,backup).map_err(|e| format!("备份启动配置失败: {e}"))?;
                    fs::write(&path,content).map_err(|e| format!("补建 -k 配置失败: {e}"))?;
                }
                save_source(self.game_dir(),source)?;
                self.send_hook_state()?;
                self.send(json!({"type":"hook_source_saved"})); Ok(())
            }
            "set_plugin_enabled" => {
                let enabled=request["enabled"].as_bool().ok_or("缺少插件开关状态")?;
                set_plugin_enabled(self.game_dir(),enabled)?;
                self.send_hook_state()?;
                self.notify(if enabled { "已启用插件，下次启动游戏时生效" } else { "已停用插件，下次启动游戏时生效" }); Ok(())
            }
            "logout" => {
                let mut settings=push::load_settings(self.game_dir()).map_err(|e| e.to_string())?;
                settings.access_code.clear(); settings.auto_sync=false;
                push::save_settings(self.game_dir(),&settings).map_err(|e| e.to_string())?;
                self.send_settings()?; self.notify("已清除访问码并关闭自动同步"); Ok(())
            }
            "upload_recent" => {
                let ids: Vec<String> = request["ids"].as_array().ok_or("请选择最近游玩分数")?
                    .iter().filter_map(|x| x.as_str().map(str::to_owned)).collect();
                let confirm = request["confirmDuplicate"].as_bool().unwrap_or(false);
                let settings = push::load_settings(self.game_dir()).map_err(|e| e.to_string())?;
                let result = push::manual_upload(self.game_dir(), &ids, confirm, &settings);
                self.refresh()?;
                let count = result?;
                self.notify(&format!("已上传 {count} 项最近游玩分数")); Ok(())
            }
            "import" => {
                let Some(path) = file_path("", "", true) else { return Ok(()); };
                let catalog_path = self.game_dir().join("data/info/0/music_data.bin");
                let catalog = core::read_catalog(&catalog_path).map_err(|e| format!("无法读取曲库索引 {}: {e}", catalog_path.display()))?;
                let result = formats::import_path(&path, &catalog).map_err(|e| format!("导入失败: {e}"))?;
                for item in &result.collections { core::write_collection(&self.root, item).map_err(|e| e.to_string())?; }
                self.refresh()?;
                self.notify(&format!("已导入 {} 份成绩，跳过 {} 项", result.collections.len(), result.skipped)); Ok(())
            }
            "rename" | "identity" => {
                let mut item = self.item(request["id"].as_str().unwrap_or(""))?;
                if request["action"] == "rename" {
                    let name = request["name"].as_str().unwrap_or("").trim();
                    if name.is_empty() { return Err("名称不能为空".into()); }
                    item.name = name.to_owned();
                } else {
                    let raw = request["iidx"].as_str().unwrap_or("").trim().replace('-', "");
                    if raw.len() != 8 || !raw.bytes().all(|b| b.is_ascii_digit()) { return Err("IIDX ID 必须是 xxxx-xxxx".into()); }
                    item.iidx_id = Some(format!("{}-{}", &raw[..4], &raw[4..]));
                }
                core::write_collection(&self.root, &item).map_err(|e| e.to_string())?;
                self.refresh()?; self.notify("已保存"); Ok(())
            }
            "delete" => {
                let id = request["id"].as_str().ok_or("请选择要删除的档案")?;
                self.item(id)?;
                core::archive_collection(&self.root, id).map_err(|e| format!("删除档案失败: {e}"))?;
                self.refresh()?;
                self.notify("档案已移至 AnyToFire_exports/.deleted，可手动恢复"); Ok(())
            }
            "export" => {
                let item = self.item(request["id"].as_str().unwrap_or(""))?;
                let format = request["format"].as_str().unwrap_or("");
                let ext = match format { "json" => "json", "csv" => "csv", "db" => "db", _ => return Err("未知导出格式".into()) };
                let Some(path) = file_path(&format!("{}.{}", item.name, ext), ext, false) else { return Ok(()); };
                match format {
                    "json" => fs::write(&path, core::fervidex_json(&item).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?,
                    "db" => {
                        if path.exists() { return Err("请选择新的 .db 文件名；不会覆盖现有 Asphyxia 数据库".into()); }
                        fs::write(&path, formats::asphyxia_db(&item).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                    }
                    _ => {
                        let sp = item.scores.iter().any(|s| s.chart < 5);
                        let dp = item.scores.iter().any(|s| s.chart >= 5);
                        if sp && dp {
                            let stem = path.file_stem().and_then(|x| x.to_str()).unwrap_or("scores");
                            let sp_path = path.with_file_name(format!("{stem}_SP.csv"));
                            let dp_path = path.with_file_name(format!("{stem}_DP.csv"));
                            if sp_path.exists() || dp_path.exists() { return Err("目标目录已有同名 SP/DP CSV，请更换文件名".into()); }
                            fs::write(sp_path, core::konami_csv(&item, 0).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                            fs::write(dp_path, core::konami_csv(&item, 1).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                        } else { fs::write(&path, core::konami_csv(&item, if dp { 1 } else { 0 }).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?; }
                    }
                }
                self.notify(&format!("已导出到 {}", path.display())); Ok(())
            }
            "merge" => {
                let ids = request["ids"].as_array().ok_or("请选择成绩")?;
                let sources: Vec<Collection> = ids.iter().map(|id| self.item(id.as_str().unwrap_or(""))).collect::<Result<_,_>>()?;
                let name = request["name"].as_str().unwrap_or("").trim();
                if name.is_empty() { return Err("名称不能为空".into()); }
                let merged = core::merge(&sources, name.to_owned())?;
                let Some(json_path) = file_path(&format!("{}.json", name), "json", false) else { return Ok(()); };
                let parent = json_path.parent().ok_or("目标路径无效")?;
                let stem = json_path.file_stem().and_then(|x| x.to_str()).unwrap_or("合并成绩");
                let sp = parent.join(format!("{stem}_SP.csv"));
                let dp = parent.join(format!("{stem}_DP.csv"));
                let db = parent.join(format!("{stem}.db"));
                if sp.exists() || dp.exists() || db.exists() { return Err("目标目录已有同名 CSV 或 DB，请更换文件名".into()); }
                fs::write(&json_path, core::fervidex_json(&merged).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                fs::write(sp, core::konami_csv(&merged, 0).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                fs::write(dp, core::konami_csv(&merged, 1).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                fs::write(db, formats::asphyxia_db(&merged).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                core::write_collection(&self.root, &merged).map_err(|e| e.to_string())?;
                self.send(json!({"type":"merge_complete"}));
                self.refresh()?; self.notify(&format!("已合并 {} 份成绩", sources.len())); Ok(())
            }
            _ => Err("未知操作".into()),
        }
    }
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        trace("resumed");
        if self.window.is_some() { return; }
        let window_icon = Icon::from_rgba(include_bytes!("../ui-demo/assets/firenet-48.rgba").to_vec(), 48, 48).ok();
        let attrs = Window::default_attributes().with_title("AnyToFire · 成绩库")
            .with_inner_size(winit::dpi::LogicalSize::new(1180, 760))
            .with_min_inner_size(winit::dpi::LogicalSize::new(860, 560))
            .with_window_icon(window_icon.clone());
        let window = match event_loop.create_window(attrs) { Ok(w) => w, Err(e) => { eprintln!("{e}"); event_loop.exit(); return; } };
        window.set_taskbar_icon(window_icon);
        trace("window created");
        let icon = base64::engine::general_purpose::STANDARD.encode(include_bytes!("../ui-demo/assets/firenet-48.png"));
        let html = include_str!("app.html")
            .replace("__APP_SCRIPT__", include_str!("app.js"))
            .replace("__FIRE_ICON__", &format!("data:image/png;base64,{icon}"));
        let proxy = self.proxy.clone();
        let webview = WebViewBuilder::new().with_html(html)
            .with_ipc_handler(move |request| {
                if let Ok(value) = serde_json::from_str::<Value>(request.body()) {
                    let _ = proxy.send_event(Event::Command(value));
                }
            }).build(&window);
        match webview {
            Ok(view) => { trace("webview created"); self.webview = Some(view); self.window = Some(window); }
            Err(e) => {
                trace(&format!("webview error: {e}"));
                let msg = wide(&format!("WebView2 无法启动：{e}"));
                unsafe { MessageBoxW(std::ptr::null_mut(), msg.as_ptr(), wide("AnyToFire").as_ptr(), 0x10); }
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event { event_loop.exit(); }
    }
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: Event) {
        let Event::Command(request) = event;
        if let Err(error) = self.command(request) { self.send(json!({"type":"error","message":error})); }
    }
}

fn main() {
    trace("main");
    let args: Vec<_> = std::env::args_os().collect();
    let binary = std::env::current_exe().unwrap_or_default();
    let default_game_dir = layout::game_dir_from_binary(&binary);
    let supplied_root = args.windows(2).find(|w| w[0] == "--data").map(|w| PathBuf::from(&w[1]));
    let game_dir = supplied_root.as_ref().and_then(|root| root.parent().map(Path::to_path_buf)).unwrap_or(default_game_dir);
    let root = supplied_root.unwrap_or_else(|| game_dir.join("AnyToFire_exports"));
    let event_loop = EventLoop::<Event>::with_user_event().build().expect("事件循环启动失败");
    let mut app = App { root, game_dir, window: None, webview: None, proxy: event_loop.create_proxy() };
    event_loop.run_app(&mut app).expect("窗口运行失败");
}
