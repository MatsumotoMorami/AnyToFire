use crate::core::{self, Catalog, Collection, Score};
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};
use std::sync::atomic::{AtomicPtr, Ordering};
use sha2::{Digest, Sha256};

type Method = unsafe extern "system" fn(*mut c_void, *mut c_void) -> bool;
static ORIGINAL_RANK: OnceLock<Method> = OnceLock::new();
static ORIGINAL_PROFILE: OnceLock<Method> = OnceLock::new();
static SENDER: OnceLock<mpsc::Sender<Event>> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static LOG_LOCK: Mutex<()> = Mutex::new(());
static INSTANCE: OnceLock<usize> = OnceLock::new();

const DLL_PROCESS_ATTACH: u32 = 1;
const PAGE_READWRITE: u32 = 4;
const RANK_RVA: usize = 0xabf4a0;
const RANK_END: usize = 0xac0080;
const RANK_SLOT: usize = 0x10540e8;
const RANK_PREFIX_HASH: u64 = 0x883334e56ba575de;
const RANK_TAIL_HASH: u64 = 0x21048fdf5a9cf31d;
const PROFILE_RVA: usize = 0xaaa1a0;
const PROFILE_END: usize = 0xab5030;
const PROFILE_SLOT: usize = 0x10536c8;
const PROFILE_PREFIX_SHA256: &str = "50bc87a2d3abbc57962eef434ed68e717e4e7bf78e3407dd620a1693effdc62b";
const PROFILE_TAIL_SHA256: &str = "0fdece9cd449d7838c4680de45998ad64d3cf2f8349e06be461de4761cb55263";
const GAME_TIMESTAMP: u32 = 0x6aa23d57;
const GAME_IMAGE_SIZE: u32 = 0xc5ff000;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, buffer: *mut u16, len: u32) -> u32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, name_or_address: *const u16, module: *mut *mut c_void) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn ReadProcessMemory(process: *mut c_void, from: *const c_void, to: *mut c_void, len: usize, read: *mut usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn VirtualProtect(address: *mut c_void, len: usize, protection: u32, old: *mut u32) -> i32;
    fn CreateThread(attrs: *mut c_void, stack: usize, start: unsafe extern "system" fn(*mut c_void) -> u32, arg: *mut c_void, flags: u32, id: *mut u32) -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn DisableThreadLibraryCalls(module: *mut c_void) -> i32;
    fn Sleep(milliseconds: u32);
    fn MultiByteToWideChar(codepage: u32, flags: u32, input: *const u8, input_len: i32, output: *mut u16, output_len: i32) -> i32;
}

#[derive(Clone)] struct Capture { player: String, style: usize, bytes: Vec<u8> }
#[derive(Clone)] struct Profile { player: String, iidx_id: String, nickname: String }
enum Event { Scores(Capture), Profile(Profile) }
fn log(message: &str) {
    let Some(path) = LOG_PATH.get() else { return; };
    let _guard = LOG_LOCK.lock();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) { let _ = writeln!(f, "{} {}", core::now(), message); }
}
fn read(address: usize, data: &mut [u8]) -> bool {
    let mut count = 0usize;
    unsafe { ReadProcessMemory(GetCurrentProcess(), address as *const c_void, data.as_mut_ptr().cast(), data.len(), &mut count) != 0 && count == data.len() }
}
fn read_u32(address: usize) -> Option<u32> { let mut x = [0u8; 4]; read(address, &mut x).then(|| u32::from_le_bytes(x)) }
fn read_u64(address: usize) -> Option<u64> { let mut x = [0u8; 8]; read(address, &mut x).then(|| u64::from_le_bytes(x)) }
fn fnv(bytes: &[u8]) -> u64 { bytes.iter().fold(14695981039346656037u64, |h, b| (h ^ *b as u64).wrapping_mul(1099511628211)) }
fn sha(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn wstr(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
fn module_path(module: usize) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let n = unsafe { GetModuleFileNameW(module as *mut c_void, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}
fn command_line() -> String {
    unsafe { let p = GetCommandLineW(); if p.is_null() { return String::new(); } let mut n = 0; while n < 32768 && *p.add(n) != 0 { n += 1; } String::from_utf16_lossy(std::slice::from_raw_parts(p, n)) }
}
fn server_url() -> String {
    let cmd = command_line();
    let lower = cmd.to_ascii_lowercase();
    if let Some(i) = lower.find("-url ") {
        let value = cmd[i + 5..].trim_start();
        if let Some(stripped) = value.strip_prefix('"') { return stripped.split('"').next().unwrap_or("").to_string(); }
        return value.split_whitespace().next().unwrap_or("").to_string();
    }
    String::new()
}
fn validate(base: usize) -> bool {
    if read_u32(base + 0x3c).filter(|v| *v < 0x1000).is_none() { return false; }
    let pe = read_u32(base + 0x3c).unwrap() as usize;
    if read_u32(base + pe) != Some(0x4550) || read_u32(base + pe + 8) != Some(GAME_TIMESTAMP) || read_u32(base + pe + 0x50) != Some(GAME_IMAGE_SIZE) { return false; }
    let mut prefix = [0u8; 32]; let mut tail = vec![0u8; RANK_END - RANK_RVA - 32];
    read(base + RANK_RVA, &mut prefix) && fnv(&prefix) == RANK_PREFIX_HASH && read(base + RANK_RVA + 32, &mut tail) && fnv(&tail) == RANK_TAIL_HASH
}
fn is_score_hook(address: usize) -> bool {
    let mut module = std::ptr::null_mut();
    if unsafe { GetModuleHandleExW(6, address as *const u16, &mut module) } == 0 { return false; }
    module_path(module as usize).and_then(|p| p.file_name().map(|x| x.to_string_lossy().to_string())).is_some_and(|x| x.eq_ignore_ascii_case("IIDXScore.dll"))
}
fn validate_profile(base: usize) -> bool {
    if read_u64(base + PROFILE_SLOT) != Some((base + PROFILE_RVA) as u64) { return false; }
    let mut prefix = [0u8; 32]; let mut tail = vec![0u8; PROFILE_END - PROFILE_RVA - 32];
    read(base + PROFILE_RVA, &mut prefix) && sha(&prefix) == PROFILE_PREFIX_SHA256 && read(base + PROFILE_RVA + 32, &mut tail) && sha(&tail) == PROFILE_TAIL_SHA256
}
fn replace_slot(slot: usize, expected: usize, new: usize) -> bool {
    let slot = slot as *mut *mut c_void; let mut old = 0u32;
    unsafe {
        if VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old) == 0 { return false; }
        let ok = (&*(slot as *const AtomicPtr<c_void>)).compare_exchange(expected as *mut c_void, new as *mut c_void, Ordering::SeqCst, Ordering::SeqCst).is_ok();
        let mut ignored = 0u32; VirtualProtect(slot.cast(), 8, old, &mut ignored); ok
    }
}
fn install(base: usize) -> bool {
    if !validate(base) { return false; }
    let Some(previous) = read_u64(base + RANK_SLOT).map(|x| x as usize) else { return false; };
    let chained = previous != base + RANK_RVA;
    if chained && !is_score_hook(previous) { return false; }
    let original: Method = unsafe { std::mem::transmute(previous) };
    if ORIGINAL_RANK.set(original).is_err() { return false; }
    if !replace_slot(base + RANK_SLOT, previous, rank_hook as *const () as usize) { return false; }
    if chained { log("检测到 IIDXScore 钩子：以链式方式安装。导出包含其本地成绩覆盖结果"); }
    if validate_profile(base) {
        let original: Method = unsafe { std::mem::transmute(base + PROFILE_RVA) };
        let _ = ORIGINAL_PROFILE.set(original);
        if replace_slot(base + PROFILE_SLOT, base + PROFILE_RVA, profile_hook as *const () as usize) { log("pc.get 资料钩子已安装"); }
        else { log("pc.get 资料钩子被占用；成绩仍会导出，但身份可能为空"); }
    } else { log("pc.get 资料 ABI 未匹配；成绩仍会导出，但身份可能为空"); }
    true
}
unsafe extern "system" fn rank_hook(ctx: *mut c_void, node: *mut c_void) -> bool {
    let result = ORIGINAL_RANK.get().map(|f| f(ctx, node)).unwrap_or(false);
    if !result { return result; }
    let _ = std::panic::catch_unwind(|| {
        let mut req = [0u8; 8]; if !read(ctx as usize + 0x70, &mut req) { return; }
        let req = u64::from_le_bytes(req) as usize; if req == 0 { return; }
        let mut head = [0u8; 0x30]; if !read(req, &mut head) { return; }
        let n = head[..16].iter().position(|b| *b == 0).unwrap_or(16);
        if n == 0 || n >= 16 || !head[..n].iter().all(u8::is_ascii_digit) { return; }
        let Ok(player_number) = String::from_utf8_lossy(&head[..n]).parse::<u64>() else { return; };
        if player_number == 0 { return; }
        let player = player_number.to_string();
        let style = u32::from_le_bytes(head[0x10..0x14].try_into().unwrap()) as usize;
        if style > 1 { return; }
        let ptr = u64::from_le_bytes(head[0x20 + style * 8..0x28 + style * 8].try_into().unwrap()) as usize;
        if ptr == 0 { return; }
        let catalog_maximum = CATALOG_MAXIMUM.get().copied().unwrap_or(0);
        if catalog_maximum == 0 { return; }
        let mut bytes = vec![0u8; catalog_maximum * 0x30];
        if read(ptr, &mut bytes) { if let Some(sender) = SENDER.get() { let _ = sender.send(Event::Scores(Capture { player, style, bytes })); } }
    });
    result
}
static CATALOG_MAXIMUM: OnceLock<usize> = OnceLock::new();

fn cstr(bytes: &[u8]) -> &[u8] { &bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())] }
fn cp932(bytes: &[u8]) -> String {
    let bytes = cstr(bytes); if bytes.is_empty() { return String::new(); }
    let mut buffer = vec![0u16; 32];
    let n = unsafe { MultiByteToWideChar(932, 0, bytes.as_ptr(), bytes.len() as i32, buffer.as_mut_ptr(), buffer.len() as i32) };
    if n > 0 { String::from_utf16_lossy(&buffer[..n as usize]) } else { String::from_utf8_lossy(bytes).into_owned() }
}
unsafe extern "system" fn profile_hook(ctx: *mut c_void, node: *mut c_void) -> bool {
    let result = ORIGINAL_PROFILE.get().map(|f| f(ctx, node)).unwrap_or(false);
    if !result { return result; }
    let _ = std::panic::catch_unwind(|| {
        let response = read_u64(ctx as usize + 0x78).unwrap_or(0) as usize;
        if response == 0 { return; }
        let profile = read_u64(response + 8).unwrap_or(0) as usize;
        if profile == 0 { return; }
        let mut bytes = [0u8; 0x2c]; if !read(profile, &mut bytes) { return; }
        let raw_id = cstr(&bytes[0x10..0x20]);
        let raw_name = cstr(&bytes[0x20..0x28]);
        let id = String::from_utf8_lossy(raw_id).to_string();
        let normalized = id.replace('-', "");
        if normalized.len() != 8 || !normalized.bytes().all(|b| b.is_ascii_digit()) { log("pc.get 已返回，但 idstr 格式未确认；身份未写入"); return; }
        let iidx_id = format!("{}-{}", &normalized[..4], &normalized[4..]);
        let nickname = cp932(raw_name);
        // pc.get writes the server's numeric pid at +0x28. getrank uses that same pid
        // as a decimal string in its first 16 bytes, so this is an exact join key.
        let pid = u32::from_le_bytes(bytes[0x28..0x2c].try_into().unwrap());
        if pid == 0 { log("pc.get 未返回有效 pid；身份未写入"); return; }
        if let Some(sender) = SENDER.get() { let _ = sender.send(Event::Profile(Profile { player: pid.to_string(), iidx_id, nickname })); }
    });
    result
}

fn parse_scores(catalog: &Catalog, capture: &Capture) -> Vec<Score> {
    let mut scores = Vec::new();
    for (&id, song) in &catalog.songs {
        let off = id as usize * 0x30;
        let row = &capture.bytes[off..off + 0x30];
        if row[0x2d] != 1 { continue; }
        for index in 0..5 {
            let chart = capture.style * 5 + index;
            let level = song.levels[chart]; if level == 0 { continue; }
            let ex = i32::from_le_bytes(row[index * 4..index * 4 + 4].try_into().unwrap());
            let bp = i32::from_le_bytes(row[0x14 + index * 4..0x18 + index * 4].try_into().unwrap());
            let lamp = row[0x28 + index];
            if ex < 0 || ex > 300000 || bp < -1 || bp > 100000 || lamp > 7 || (ex == 0 && lamp == 0 && bp < 0) { continue; }
            scores.push(Score { music: id, chart: chart as u8, ex: ex as u32, bp: (bp >= 0).then_some(bp as u32), lamp, title: song.title.clone(), genre: song.genre.clone(), artist: song.artist.clone(), level, levels: song.levels, pgreat: None, great: None });
        }
    }
    scores.sort_by_key(|s| (s.music, s.chart)); scores
}
fn process_captures(rx: mpsc::Receiver<Event>, catalog: Catalog, dir: PathBuf, url: String) {
    let mut sessions: HashMap<String, Collection> = HashMap::new();
    let mut profiles: HashMap<String, Profile> = HashMap::new();
    for event in rx {
        if let Event::Profile(profile) = event {
            log(&format!("pc.get 资料: pid {}，IIDX ID {}，昵称 {}", profile.player, profile.iidx_id, profile.nickname));
            profiles.insert(profile.player.clone(), profile.clone());
            if let Some(entry) = sessions.get_mut(&profile.player) { entry.iidx_id = Some(profile.iidx_id); entry.nickname = Some(profile.nickname); let _ = core::write_collection(&dir, entry); }
            continue;
        }
        let Event::Scores(capture) = event else { continue; };
        let new_scores = parse_scores(&catalog, &capture);
        if new_scores.is_empty() { log(&format!("{}P: 个人成绩缓存为空，未导出", if capture.style == 0 { "S" } else { "D" })); continue; }
        let player = capture.player.clone();
        let entry = sessions.entry(player.clone()).or_insert_with(|| Collection { schema: 1, id: core::new_id(), name: core::server_name(&url), server_url: url.clone(), player_internal: player, iidx_id: None, nickname: None, captured_at: core::now(), catalog_sha256: catalog.sha256.clone(), asphyxia_refid: None, parents: vec![], scores: vec![] });
        if let Some(profile) = profiles.get(&capture.player) { entry.iidx_id = Some(profile.iidx_id.clone()); entry.nickname = Some(profile.nickname.clone()); }
        entry.scores.retain(|s| s.chart as usize / 5 != capture.style);
        entry.scores.extend(new_scores);
        entry.scores.sort_by_key(|s| (s.music, s.chart));
        match core::write_collection(&dir, entry) {
            Ok(path) => log(&format!("导出 {} 张谱面至 {}", entry.scores.len(), path.display())),
            Err(e) => log(&format!("导出失败: {e}")),
        }
    }
}
unsafe extern "system" fn worker(_: *mut c_void) -> u32 {
    let Some(path) = module_path(*INSTANCE.get().unwrap_or(&0)) else { return 0; };
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let _ = LOG_PATH.set(dir.join("AnyToFire.log"));
    log("AnyToFire 0.2.0 启动");
    let catalog_path = dir.join("data/info/0/music_data.bin");
    let catalog = match core::read_catalog(&catalog_path) { Ok(x) => x, Err(e) => { log(&format!("曲库读取失败: {e}")); return 0; } };
    if catalog.version != 34 { log("当前仅支持 IIDX 34 曲库；未安装钩子"); return 0; }
    let _ = CATALOG_MAXIMUM.set(catalog.maximum);
    let url = server_url();
    log(&format!("EA URL: {}；曲库 {} 首", if url.is_empty() { "未知" } else { &url }, catalog.songs.len()));
    let exports = dir.join("AnyToFire_exports"); if let Err(e) = fs::create_dir_all(&exports) { log(&format!("输出目录创建失败: {e}")); return 0; }
    let (tx, rx) = mpsc::channel(); let _ = SENDER.set(tx);
    std::thread::spawn(move || process_captures(rx, catalog, exports, url));
    let name = wstr("bm2dx.dll"); let mut base = 0usize;
    for _ in 0..600 { base = GetModuleHandleW(name.as_ptr()) as usize; if base != 0 { break; } Sleep(100); }
    if base == 0 || !install(base) { log("未识别的 bm2dx.dll 或成绩钩子已被占用；未安装"); return 0; }
    let mut pinned = std::ptr::null_mut();
    let _ = GetModuleHandleExW(5, worker as *const () as *const u16, &mut pinned);
    log("IIDX34 个人成绩缓存钩子已安装；登录后自动导出");
    0
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(instance: *mut c_void, reason: u32, _: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        let _ = INSTANCE.set(instance as usize);
        DisableThreadLibraryCalls(instance);
        let thread = CreateThread(std::ptr::null_mut(), 0, worker, std::ptr::null_mut(), 0, std::ptr::null_mut());
        if !thread.is_null() { CloseHandle(thread); }
    }
    1
}
