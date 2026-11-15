#![windows_subsystem = "windows"]

mod spice_config;

use spice_config::{SpiceConfig, EMPTY_XML};

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
#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum HookSource { Cfg, Bat }
impl HookSource {
    fn as_str(self) -> &'static str { if self == Self::Cfg { "cfg" } else { "bat" } }
    fn parse(value: &str) -> Result<Self, String> {
        match value { "cfg" => Ok(Self::Cfg), "bat" => Ok(Self::Bat), _ => Err("请选择 XML 自动配置或命令行手动配置".into()) }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchSettings {
    source: HookSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cfg_enabled: Option<bool>,
}

fn source_path(game_dir: &Path) -> PathBuf { layout::config_path(game_dir, "AnyToFire_launch.json") }

fn load_launch(game_dir: &Path) -> Result<Option<LaunchSettings>, String> {
    match fs::read(source_path(game_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("启动方式配置损坏：{e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("无法读取启动方式配置：{error}")),
    }
}

fn save_launch(game_dir: &Path, settings: &LaunchSettings) -> Result<(), String> {
    let encoded = serde_json::to_vec(settings).map_err(|e| format!("启动方式配置无法编码：{e}"))?;
    core::atomic_write(&source_path(game_dir), &encoded).map_err(|e| format!("保存启动方式失败：{e}"))
}

fn configured_launch(game_dir: &Path) -> Result<Option<LaunchSettings>, String> {
    Ok(load_launch(game_dir)?.or_else(|| initialization_marker(game_dir).is_file().then_some(
        LaunchSettings { source: HookSource::Cfg, cfg_enabled: None },
    )))
}

fn spice_config_path() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA").ok_or("找不到 APPDATA 目录")?;
    Ok(PathBuf::from(appdata).join("spicetools.xml"))
}

fn read_hook_config(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("无法读取 {}：{error}", path.display())),
    }
}

fn cfg_plugin_enabled() -> Result<bool, String> {
    let xml = read_hook_config(&spice_config_path()?)?;
    Ok(SpiceConfig::parse(xml.as_deref().unwrap_or(EMPTY_XML))?.plugin_enabled())
}

fn repair_cfg_hook(path: &Path, binary: &Path, restore_missing: bool) -> Result<(), String> {
    let original = read_hook_config(path)?;
    let cfg = SpiceConfig::parse(original.as_deref().unwrap_or(EMPTY_XML))?;
    if !(cfg.plugin_enabled() || restore_missing && cfg.hook_value().is_none()) {
        return Ok(());
    }
    let hook = layout::plugin_hook_path(binary)?;
    let updated = cfg.set_hook(Some(&hook), false);
    write_hook_config(path, original.as_deref(), &updated)
}

fn migrate_plugin_hook(game_dir: &Path, binary: &Path) -> Result<(), String> {
    let Some(launch) = configured_launch(game_dir)? else { return Ok(()); };
    if launch.source == HookSource::Bat { return Ok(()); }
    let restore_missing = launch.cfg_enabled.unwrap_or_else(|| initialization_marker(game_dir).is_file());
    repair_cfg_hook(&spice_config_path()?, binary, restore_missing)
}

fn write_hook_config(path: &Path, original: Option<&str>, updated: &str) -> Result<(), String> {
    if original == Some(updated) { return Ok(()); }
    if read_hook_config(path)?.as_deref() != original {
        return Err(format!("{} 已被其他程序更新，请重试", path.display()));
    }
    if let Some(original) = original {
        let backup = path.with_extension("xml.AnyToFire.bak");
        core::atomic_write(&backup, original.as_bytes()).map_err(|e| format!("备份启动配置失败：{e}"))?;
    }
    core::atomic_write(path, updated.as_bytes()).map_err(|e| format!("更新 {} 失败：{e}", path.display()))
}

fn select_hook_source(game_dir: &Path, source: HookSource) -> Result<(), String> {
    let cfg_enabled = load_launch(game_dir)?.and_then(|settings| settings.cfg_enabled);
    save_launch(game_dir, &LaunchSettings { source, cfg_enabled })
}

fn set_plugin_hooks(game_dir: &Path, binary: &Path, enabled: bool, initializing: bool) -> Result<(), String> {
    let Some(mut launch) = configured_launch(game_dir)? else { return Err("请先选择插件加载方式".into()); };
    if launch.source == HookSource::Bat {
        return if initializing { Ok(()) } else { Err("请在实际的启动命令中增删 AnyToFire 的 -k 参数".into()) };
    }
    let path = spice_config_path()?;
    let original = read_hook_config(&path)?;
    let cfg = SpiceConfig::parse(original.as_deref().unwrap_or(EMPTY_XML))?;
    if enabled || cfg.plugin_enabled() {
        let hook = enabled.then(|| layout::plugin_hook_path(binary)).transpose()?;
        let updated = cfg.set_hook(hook.as_deref(), initializing);
        write_hook_config(&path, original.as_deref(), &updated)?;
    }
    launch.cfg_enabled = Some(enabled);
    save_launch(game_dir, &launch)
}

fn initialization_marker(game_dir: &Path) -> PathBuf { anytofire::layout::config_path(game_dir,"AnyToFire_initialized.json") }
fn game_version(game_dir: &Path) -> Result<u32, String> {
    let catalog_path = game_dir.join(layout::CATALOG_PATH);
    core::read_catalog(&catalog_path)
        .map(|catalog| catalog.version).map_err(|e| format!("无法读取游戏曲库 {} ({e})", catalog_path.display()))
}
fn score_ini_path(game_dir: &Path) -> PathBuf { anytofire::layout::config_path(game_dir,"IIDXScore.ini") }
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
    const PACKAGE_HOOK: Option<&str> = Some(r"D:\IIDX\AnyToFire\AnyToFire.dll");

    #[test]
    fn xml_hook_list_changes_only_iidx() {
        let xml = "\u{feff}<games><!-- <game name='Beatmania IIDX'/> --><game name='Beatmania IIDX' extra='1'><options><option value='ifs&amp;hook.dll IIDXRecorder.dll' name = 'k' /></options></game><game name='Other'><options><option name='k' value='Other.dll'/></options></game></games>";
        let enabled = SpiceConfig::parse(xml).unwrap().set_hook(PACKAGE_HOOK, false);
        assert_eq!(enabled, xml.replace("IIDXRecorder.dll'", "IIDXRecorder.dll D:\\IIDX\\AnyToFire\\AnyToFire.dll'"));
        let cfg = SpiceConfig::parse(&enabled).unwrap();
        assert!(cfg.plugin_enabled());
        assert_eq!(cfg.set_hook(PACKAGE_HOOK, false), enabled);
        assert_eq!(cfg.set_hook(None, false), xml);
    }

    #[test]
    fn adds_missing_iidx_options_without_touching_the_other_games() {
        let other = "<game name='Other'><options><option name='k' value='Other.dll'/></options></game>";
        for iidx in [
            "",
            "<game name='Beatmania IIDX'/>",
            "<game name='Beatmania IIDX'><options /></game>",
            "<game name='Beatmania IIDX'><options><option name='url' value='https://example/core/'/></options></game>",
            "<game name='Beatmania IIDX'><options><option name='k'/></options></game>",
        ] {
            let xml = format!("<games>{other}{iidx}</games>");
            let updated = SpiceConfig::parse(&xml).unwrap().set_hook(PACKAGE_HOOK, false);
            assert_eq!(SpiceConfig::parse(&updated).unwrap().hook_value(), PACKAGE_HOOK);
            assert!(updated.contains(other));
            assert_eq!(updated.matches("name='k'").count() + updated.matches("name=\"k\"").count(), 2);
        }
        let updated = SpiceConfig::parse("<games />").unwrap().set_hook(PACKAGE_HOOK, false);
        assert_eq!(SpiceConfig::parse(&updated).unwrap().hook_value(), PACKAGE_HOOK);
    }

    #[test]
    fn startup_repairs_missing_xml_and_keeps_disabled_or_malformed_configs() {
        let dir = std::env::temp_dir().join(format!("atf-spice-xml-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let xml = dir.join("spicetools.xml");
        let backup = xml.with_extension("xml.AnyToFire.bak");
        let exe = dir.join("AnyToFire.exe");
        fs::write(dir.join(layout::PLUGIN_DLL), b"fixture").unwrap();
        let without_hook = "<games/>";

        repair_cfg_hook(&xml, &exe, false).unwrap();
        assert!(!xml.exists());
        fs::write(&xml, without_hook).unwrap();
        repair_cfg_hook(&xml, &exe, false).unwrap();
        assert_eq!(fs::read_to_string(&xml).unwrap(), without_hook);
        repair_cfg_hook(&xml, &exe, true).unwrap();
        let repaired = fs::read_to_string(&xml).unwrap();
        assert_eq!(SpiceConfig::parse(&repaired).unwrap().hook_value(), Some(layout::plugin_hook_path(&exe).unwrap().as_str()));
        assert_eq!(fs::read_to_string(&backup).unwrap(), without_hook);
        repair_cfg_hook(&xml, &exe, true).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), without_hook);

        fs::remove_file(&xml).unwrap();
        repair_cfg_hook(&xml, &exe, true).unwrap();
        assert!(SpiceConfig::parse(&fs::read_to_string(&xml).unwrap()).unwrap().plugin_enabled());
        fs::write(&xml, "<games>").unwrap();
        assert!(repair_cfg_hook(&xml, &exe, true).is_err());
        assert_eq!(fs::read_to_string(&xml).unwrap(), "<games>");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn manual_launch_keeps_existing_scripts_and_accepts_custom_names() {
        let game = std::env::temp_dir().join(format!("atf-hook-layout-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&game).unwrap();
        let start = game.join("start.bat");
        let custom = game.join("play-iidx.cmd");
        let duplicate_hooks = b"spice64.exe -k \"firenet.dll AnyToFire\\AnyToFire.dll\" -k AnyToFire.dll\r\n";
        fs::write(&start, duplicate_hooks).unwrap();
        fs::write(&custom, duplicate_hooks).unwrap();
        let binary = game.join("AnyToFire.exe");

        select_hook_source(&game, HookSource::Bat).unwrap();
        migrate_plugin_hook(&game, &binary).unwrap();
        set_plugin_hooks(&game, &binary, true, true).unwrap();
        assert!(set_plugin_hooks(&game, &binary, true, false).is_err());
        assert!(set_plugin_hooks(&game, &binary, false, false).is_err());
        assert_eq!(fs::read(&start).unwrap(), duplicate_hooks);
        assert_eq!(fs::read(&custom).unwrap(), duplicate_hooks);
        assert!(!game.join("start.bat.AnyToFire.bak").exists());

        fs::remove_file(&start).unwrap();
        select_hook_source(&game, HookSource::Bat).unwrap();
        migrate_plugin_hook(&game, &binary).unwrap();
        set_plugin_hooks(&game, &binary, true, true).unwrap();
        assert!(!start.exists());
        fs::remove_dir_all(game).unwrap();
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
enum Event { Command(Value), Update(Result<Option<String>, String>) }

struct App { root: PathBuf, game_dir: PathBuf, binary: PathBuf, window: Option<Window>, webview: Option<WebView>, proxy: EventLoopProxy<Event> }
impl App {
    fn send(&self, value: Value) {
        if let Some(webview) = &self.webview {
            let _ = webview.evaluate_script(&format!("window.anytofireReceive({value});"));
        }
    }
    fn items(&self) -> Result<Vec<Collection>, String> { core::load_collections(&self.root).map_err(|e| e.to_string()) }
    fn refresh(&self) -> Result<(), String> {
        if push::load_settings(self.game_dir()).map_err(|e|e.to_string())?.auto_merge { core::auto_merge_collections(&self.root).map_err(|e|e.to_string())?; }
        let rows: Vec<Value> = self.items()?.iter().map(|item| {
            let source = if !item.parents.is_empty() && !item.id.starts_with("auto-merge-") { "合并".to_owned() } else { core::server_name(&item.server_url) };
            let sp = item.scores.iter().filter(|s| s.chart < 5).count();
            json!({ "id": item.id, "name": item.name, "source": source,
                "url": item.server_url, "player": item.nickname.as_deref().unwrap_or("—"),
                "iidx": item.iidx_id.as_deref().unwrap_or("—"), "charts": item.scores.len(),
                "sp": sp, "dp": item.scores.len() - sp, "date": item.captured_at,
                "kind": if item.id.starts_with("auto-merge-") { "自动合并" } else if item.parents.is_empty() { "原始导出" } else { "合并结果" },
                "parents": item.parents })
        }).collect();
        self.send(json!({"type":"items", "items":rows}));
        let recent=push::recent_rows(self.game_dir()).map_err(|e| e.to_string())?;
        self.send(json!({"type":"recent", "items":recent})); Ok(())
    }
    fn game_dir(&self) -> &Path { &self.game_dir }
    fn hook_state(&self) -> Result<Value, String> {
        let source = configured_launch(self.game_dir())?.map(|launch| launch.source);
        let mut state = json!({"hookSource": source.unwrap_or(HookSource::Cfg).as_str()});
        if source == Some(HookSource::Bat) {
            match layout::plugin_hook_path(&self.binary) {
                Ok(path) => state["hookArg"] = json!(format!("-k {path}")),
                Err(error) => state["hookError"] = json!(error),
            }
        } else {
            match source.map(|_| cfg_plugin_enabled()).transpose() {
                Ok(enabled) => state["pluginEnabled"] = json!(enabled.unwrap_or(false)),
                Err(error) => state["hookError"] = json!(error),
            }
        }
        Ok(state)
    }
    fn send_settings(&self) -> Result<(), String> {
        let settings=push::load_settings(self.game_dir()).map_err(|e| e.to_string())?;
        let mut state = self.hook_state()?;
        state["type"] = json!("settings");
        state["autoSync"] = json!(settings.auto_sync_enabled());
        state["hasAccessCode"] = json!(settings.has_access_code());
        state["omniOverlay"] = json!(settings.omni_overlay);
        state["omniLocalOnly"] = json!(settings.omni_local_only);
        state["autoMerge"] = json!(settings.auto_merge);
        state["autoUpdate"] = json!(settings.auto_update);
        self.send(state);
        Ok(())
    }
    fn send_hook_state(&self) -> Result<(), String> {
        let mut state = self.hook_state()?;
        state["type"] = json!("hook_state");
        self.send(state);
        Ok(())
    }
    fn item(&self, id: &str) -> Result<Collection, String> {
        self.items()?.into_iter().find(|x| x.id == id).ok_or_else(|| "找不到这份成绩".into())
    }
    fn check_update(&self) {
        static STARTED: std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
        if !push::load_settings(self.game_dir()).map(|s|s.auto_update).unwrap_or(false) {return}
        if STARTED.swap(true,std::sync::atomic::Ordering::AcqRel){return}
        let proxy=self.proxy.clone();
        let binary = self.binary.clone();
        let game_dir = self.game_dir.clone();
        std::thread::spawn(move || {
            let result = anytofire::update::check_and_stage(&binary, &game_dir);
            let _ = proxy.send_event(Event::Update(result));
        });
    }
    fn notify(&self, message: &str) { self.send(json!({"type":"toast","message":message})); }
    fn command(&self, request: Value) -> Result<(), String> {
        match request["action"].as_str().unwrap_or("") {
            "ready" => {
                if let Err(error) = migrate_plugin_hook(self.game_dir(), &self.binary) {
                    self.notify(&format!("无法更新 Spice XML：{error}"));
                }
                self.refresh()?; self.send_settings()?;
                self.check_update();
                let needs_initialization = !initialization_marker(self.game_dir()).is_file()
                    || !self.game_dir().join("AnyToFire_data/music_omni_33_final.bin").is_file()
                    || !self.game_dir().join("AnyToFire_data/music_omni_34_0a.bin").is_file();
                self.send(json!({"type":"initialization","needed":needs_initialization,"version":game_version(self.game_dir())?}));
                Ok(())
            },
            "refresh" => { self.refresh()?; self.send_settings() },
            "init_prepare" => {
                if load_launch(self.game_dir())?.is_none() { return Err("请先选择插件加载方式".into()); }
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
                set_plugin_hooks(self.game_dir(), &self.binary, true, true)?;
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
                if let Some(enabled)=request["omniLocalOnly"].as_bool() { settings.omni_local_only=enabled; }
                if let Some(enabled)=request["autoMerge"].as_bool() { settings.auto_merge=enabled; }
                if let Some(enabled)=request["autoUpdate"].as_bool() { settings.auto_update=enabled; }
                settings.auto_sync = settings.auto_sync_enabled();
                push::save_settings(self.game_dir(),&settings).map_err(|e| e.to_string())?;
                self.send(json!({"type":"settings_saved"}));
                self.refresh()?; self.check_update(); self.send_settings()?; self.notify("设置已保存；Omnifix 显示与本地存分开关在下次启动游戏时生效"); Ok(())
            }
            "set_hook_source" => {
                let source=HookSource::parse(request["source"].as_str().unwrap_or(""))?;
                select_hook_source(self.game_dir(), source)?;
                if let Err(error) = migrate_plugin_hook(self.game_dir(), &self.binary) {
                    self.notify(&format!("无法更新 Spice XML：{error}"));
                }
                self.send_hook_state()?;
                self.send(json!({"type":"hook_source_saved"})); Ok(())
            }
            "set_plugin_enabled" => {
                let enabled=request["enabled"].as_bool().ok_or("缺少插件开关状态")?;
                set_plugin_hooks(self.game_dir(), &self.binary, enabled, false)?;
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
        let request=match event {
            Event::Command(request)=>request,
            Event::Update(result)=>{match result {
                Ok(Some(version))=>self.notify(&format!("{version} 已下载，退出游戏和软件后自动安装")),
                Ok(None)=>{},Err(error)=>trace(&format!("自动更新检查未完成: {error}")),
            } return;}
        };
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
    let mut app = App { root, game_dir, binary, window: None, webview: None, proxy: event_loop.create_proxy() };
    event_loop.run_app(&mut app).expect("窗口运行失败");
}
