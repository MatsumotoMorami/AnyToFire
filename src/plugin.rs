use crate::core::{self, Catalog, Collection, Score};
use crate::{music_reg::AvsNodes, push, score};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use sha2::{Digest, Sha256};
use crate::builds::{Build, MethodSignature, BUILDS};

type Method = unsafe extern "system" fn(*mut c_void, *mut c_void) -> bool;
static ORIGINAL_RANK: OnceLock<Method> = OnceLock::new();
static ORIGINAL_PROFILE: OnceLock<Method> = OnceLock::new();
static ORIGINAL_REG: OnceLock<Method> = OnceLock::new();
static ORIGINAL_REG_RESPONSE: OnceLock<Method> = OnceLock::new();
static AVS_NODES: OnceLock<AvsNodes> = OnceLock::new();
static SENDER: OnceLock<mpsc::Sender<Event>> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static LOG_LOCK: Mutex<()> = Mutex::new(());
static INSTANCE: OnceLock<usize> = OnceLock::new();
static REG_RESPONSE_ACTIVE: AtomicBool = AtomicBool::new(false);
static SCORE_ACTIVE: AtomicBool = AtomicBool::new(false);
static SCORE_STORE: OnceLock<Mutex<score::Store>> = OnceLock::new();
static ORIGINAL_APPOINT: OnceLock<Method> = OnceLock::new();
static OBSERVED_EX: OnceLock<Mutex<HashMap<(String,i32,i32),i32>>> = OnceLock::new();
static GAME_BUILD: OnceLock<&'static Build> = OnceLock::new();

const DLL_PROCESS_ATTACH: u32 = 1;
const PAGE_READWRITE: u32 = 4;
const TRUSTED_AC_CATALOG_SHA256: &str = "2a817e42e9d9f6485b81b67104656da9f350c7106af9be15ec21c1e6ea147455";
const TRUSTED_AC_33_SHA256: &str = "6b7bed978c2734276412f6b51c6624f75a004ddf8257826f622c8fe8415721fc";
fn trusted_catalog(catalog: &Catalog) -> bool {
    (catalog.version == 34 && catalog.sha256 == TRUSTED_AC_CATALOG_SHA256)
        || (catalog.version == 33 && catalog.sha256 == TRUSTED_AC_33_SHA256)
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, buffer: *mut u16, len: u32) -> u32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, name_or_address: *const u16, module: *mut *mut c_void) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn ReadProcessMemory(process: *mut c_void, from: *const c_void, to: *mut c_void, len: usize, read: *mut usize) -> i32;
    fn WriteProcessMemory(process: *mut c_void, to: *mut c_void, from: *const c_void, len: usize, written: *mut usize) -> i32;
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
enum Event { Scores(Capture), Profile(Profile), Play { context: usize, tree: Value }, NativeAck { context: usize, music: u32, chart: u8 } }
fn log(message: &str) {
    let Some(path) = LOG_PATH.get() else { return; };
    let _guard = LOG_LOCK.lock();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) { let _ = writeln!(f, "{} {}", core::now(), message); }
}
fn read(address: usize, data: &mut [u8]) -> bool {
    let mut count = 0usize;
    unsafe { ReadProcessMemory(GetCurrentProcess(), address as *const c_void, data.as_mut_ptr().cast(), data.len(), &mut count) != 0 && count == data.len() }
}
fn write(address: usize, data: &[u8]) -> bool {
    let mut count=0usize;
    unsafe { WriteProcessMemory(GetCurrentProcess(),address as *mut c_void,data.as_ptr().cast(),data.len(),&mut count)!=0 && count==data.len() }
}
fn read_u32(address: usize) -> Option<u32> { let mut x = [0u8; 4]; read(address, &mut x).then(|| u32::from_le_bytes(x)) }
fn read_u64(address: usize) -> Option<u64> { let mut x = [0u8; 8]; read(address, &mut x).then(|| u64::from_le_bytes(x)) }
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
fn command_option(cmd: &str, option: &str) -> Option<String> {
    let words: Vec<&str> = cmd.split('"').collect();
    let mut tokens = Vec::new();
    for (i, part) in words.iter().enumerate() {
        if i % 2 == 1 { tokens.push((*part).to_owned()); }
        else { tokens.extend(part.split_whitespace().map(str::to_owned)); }
    }
    tokens.windows(2).find(|pair| pair[0].eq_ignore_ascii_case(option)).map(|pair| pair[1].clone())
}
fn xml_attribute(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let value = tag[start..].split('"').next()?;
    Some(value.replace("&amp;", "&").replace("&quot;", "\""))
}
fn spice_url(xml: &str) -> Option<String> {
    let start = xml.find("<game name=\"Beatmania IIDX\"")?;
    let game = &xml[start..xml[start..].find("</game>")? + start];
    let options = game.find("<options")?;
    let options = &game[options..];
    let options = &options[..options.find("</options>")?];
    for item in options.split("<option ").skip(1) {
        let tag = item.split('>').next()?;
        if xml_attribute(tag, "name").as_deref() == Some("url") {
            return xml_attribute(tag, "value").filter(|s| !s.is_empty());
        }
    }
    None
}
fn server_url() -> String {
    let cmd = command_line();
    if let Some(url) = command_option(&cmd, "-url") { return url; }
    let config = command_option(&cmd, "-cfgpath").map(PathBuf::from).or_else(||
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("spicetools.xml")));
    config.and_then(|p| fs::read_to_string(p).ok()).and_then(|xml| spice_url(&xml)).unwrap_or_default()
}
fn is_firenet_url(url: &str) -> bool {
    let authority = url.split_once("://").map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#']).next().unwrap_or("")
        .rsplit('@').next().unwrap_or("");
    let host = authority.split(':').next().unwrap_or("").to_ascii_lowercase();
    host == "firenet.iidx.pro" || host.ends_with(".firenet.iidx.pro")
}
fn signature_matches(base: usize, signature: &MethodSignature) -> bool {
    let mut bytes = [0u8; 256];
    read(base + signature.rva, &mut bytes) && sha(&bytes) == signature.sha256
}
fn detect_build(base: usize, version: u32) -> Option<&'static Build> {
    if read_u32(base + 0x3c).filter(|v| *v < 0x1000).is_none() { return None; }
    let pe = read_u32(base + 0x3c).unwrap() as usize;
    if read_u32(base + pe) != Some(0x4550) { return None; }
    BUILDS.iter().find(|build| build.version == version
        && read_u32(base + pe + 8) == Some(build.timestamp)
        && read_u32(base + pe + 0x50) == Some(build.image_size)
        && signature_matches(base, &build.rank))
}
fn build() -> &'static Build { GAME_BUILD.get().copied().expect("validated game build") }
fn is_score_hook(address: usize) -> bool {
    let mut module = std::ptr::null_mut();
    if unsafe { GetModuleHandleExW(6, address as *const u16, &mut module) } == 0 { return false; }
    module_path(module as usize).and_then(|p| p.file_name().map(|x| x.to_string_lossy().to_string())).is_some_and(|x| x.eq_ignore_ascii_case("IIDXScore.dll"))
}
fn is_known_reg_hook(address: usize) -> bool { is_score_hook(address) }
fn wait_for_score_hooks(base:usize)->bool{
    for _ in 0..500 {
        let Some(rank)=read_u64(base+build().rank.slot).map(|x|x as usize) else{return false};
        let Some(reg)=read_u64(base+build().reg.slot).map(|x|x as usize) else{return false};
        let Some(appoint)=read_u64(base+build().appoint.slot).map(|x|x as usize) else{return false};
        let native=rank==base+build().rank.rva && reg==base+build().reg.rva && appoint==base+build().appoint.rva;
        let score=is_score_hook(rank)&&is_score_hook(reg)&&is_score_hook(appoint);
        if native||score{return true}
        let partial=[rank!=base+build().rank.rva,reg!=base+build().reg.rva,appoint!=base+build().appoint.rva]
            .into_iter().any(|changed|changed);
        if !partial{return true}
        if (rank!=base+build().rank.rva&&!is_score_hook(rank)) || (reg!=base+build().reg.rva&&!is_score_hook(reg)) || (appoint!=base+build().appoint.rva&&!is_score_hook(appoint)){return false}
        unsafe{Sleep(20)};
    }
    false
}
fn validate_profile(base: usize) -> bool {
    let method = &build().profile;
    read_u64(base + method.slot) == Some((base + method.rva) as u64) && signature_matches(base, method)
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
    if !signature_matches(base, &build().rank) { return false; }
    let Some(previous) = read_u64(base + build().rank.slot).map(|x| x as usize) else { return false; };
    let chained = previous != base + build().rank.rva;
    if chained && !is_score_hook(previous) { return false; }
    let original: Method = unsafe { std::mem::transmute(previous) };
    if ORIGINAL_RANK.set(original).is_err() { return false; }
    if !replace_slot(base + build().rank.slot, previous, rank_hook as *const () as usize) { return false; }
    if chained { log("检测到 IIDXScore 钩子：以链式方式安装。导出包含其本地成绩覆盖结果"); }
    if validate_profile(base) {
        let original: Method = unsafe { std::mem::transmute(base + build().profile.rva) };
        let _ = ORIGINAL_PROFILE.set(original);
        if replace_slot(base + build().profile.slot, base + build().profile.rva, profile_hook as *const () as usize) { log("pc.get 资料钩子已安装"); }
        else { log("pc.get 资料钩子被占用；成绩仍会导出，但身份可能为空"); }
    } else { log("pc.get 资料 ABI 未匹配；成绩仍会导出，但身份可能为空"); }
    let mut reg_installed=false;
    if validate_reg(base) {
        if let Some(previous) = read_u64(base + build().reg.slot).map(|x| x as usize) {
            if (previous == base + build().reg.rva || is_known_reg_hook(previous)) && ORIGINAL_REG.set(unsafe { std::mem::transmute(previous) }).is_ok() {
                if replace_slot(base + build().reg.slot, previous, reg_hook as *const () as usize) { reg_installed=true; log("music.reg 完整请求钩子已安装"); }
                else { log("music.reg 钩子安装失败；单局采集不可用"); }
            } else { log("music.reg 槽位被未知插件占用；单局采集不可用"); }
        }
    } else { log("music.reg ABI 未匹配；单局采集不可用"); }
    if SCORE_STORE.get().is_some() && !chained && reg_installed {
        SCORE_ACTIVE.store(true,Ordering::Release);
        if validate_appoint(base) {
            let original:Method=unsafe{std::mem::transmute(base+build().appoint.rva)};
            let _=ORIGINAL_APPOINT.set(original);
            if replace_slot(base+build().appoint.slot,base+build().appoint.rva,appoint_hook as *const () as usize){log("IIDXScore 兼容分数柱钩子已安装");}
            else{log("IIDXScore 兼容分数柱槽位被占用；本地存分和最佳分数仍可用");}
        } else {log("IIDXScore 兼容分数柱 ABI 未匹配；本地存分和最佳分数仍可用");}
        log("AnyToFire 已独占 IIDXScore 兼容存分钩子");
    } else if chained { log("IIDXScore 已先安装完整钩子：AnyToFire 不重复存分"); }
    else if SCORE_STORE.get().is_some(){log("本地存分钩子未能完整安装；本次启动未接管存分");}
    if validate_reg_response(base) {
        if let Some(previous) = read_u64(base + build().reg_response.slot).map(|x| x as usize) {
            if (previous == base + build().reg_response.rva || is_known_reg_hook(previous)) && ORIGINAL_REG_RESPONSE.set(unsafe { std::mem::transmute(previous) }).is_ok() {
                if replace_slot(base + build().reg_response.slot, previous, reg_response_hook as *const () as usize) { REG_RESPONSE_ACTIVE.store(true, Ordering::Release); log("music.reg 原生响应钩子已安装"); }
                else { log("music.reg 原生响应钩子安装失败；无法确认原生上传"); }
            } else { log("music.reg 原生响应槽位被占用；无法确认原生上传"); }
        }
    } else { log("music.reg 原生响应 ABI 未匹配；无法确认原生上传"); }
    true
}
fn validate_appoint(base:usize)->bool{
    let method = &build().appoint;
    read_u64(base + method.slot) == Some((base + method.rva) as u64) && signature_matches(base, method)
}
fn validate_reg(base: usize) -> bool {
    signature_matches(base, &build().reg)
}
fn validate_reg_response(base: usize) -> bool {
    signature_matches(base, &build().reg_response)
}
unsafe extern "system" fn reg_hook(ctx: *mut c_void, node: *mut c_void) -> bool {
    if SCORE_ACTIVE.load(Ordering::Acquire) {
        let _=std::panic::catch_unwind(||{
            let Some(store)=SCORE_STORE.get() else{return};
            let Some(req)=read_u64(ctx as usize+0x70).map(|x|x as usize) else{return};
            if req==0{return}
            let mut raw=vec![0u8;0xea0];if !read(req,&mut raw){return}
            if let Ok(mut db)=store.lock(){
                if let Some(a)=score::Attempt::from_raw(raw,db.config.music_limit){
                    if let Err(e)=db.save(&a){log(&format!("IIDXScore 兼容单局保存失败: {e}"));}
                }
            }
        });
    }
    let result = ORIGINAL_REG.get().map(|f| f(ctx,node)).unwrap_or(false);
    if result {
        let _ = std::panic::catch_unwind(|| {
            let Some(avs) = AVS_NODES.get() else { return; };
            match unsafe { avs.encode_call(node) } {
                Ok(tree) => { if let Some(sender)=SENDER.get() { let _=sender.send(Event::Play { context: ctx as usize, tree }); } }
                Err(e) => log(&format!("music.reg 未采集: {e}")),
            }
        });
    }
    result
}
unsafe extern "system" fn reg_response_hook(ctx: *mut c_void, node: *mut c_void) -> bool {
    let result = ORIGINAL_REG_RESPONSE.get().map(|f| f(ctx, node)).unwrap_or(false);
    if result {
        let _ = std::panic::catch_unwind(|| {
            let Some(avs) = AVS_NODES.get() else { return; };
            match unsafe { avs.reg_ack(node) } {
                Ok(Some((music, chart))) => { if let Some(sender) = SENDER.get() { let _ = sender.send(Event::NativeAck { context: ctx as usize, music, chart }); } }
                Ok(None) => log("music.reg 原生响应缺少可验证的曲目/谱面；不标记已上传"),
                Err(e) => log(&format!("music.reg 原生响应读取失败: {e}")),
            }
        });
    }
    result
}
unsafe extern "system" fn rank_hook(ctx: *mut c_void, node: *mut c_void) -> bool {
    let result = ORIGINAL_RANK.get().map(|f| f(ctx, node)).unwrap_or(false);
    if !result { return result; }
    if SCORE_ACTIVE.load(Ordering::Acquire) {let _=std::panic::catch_unwind(||overlay_score(ctx));}
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
fn score_player(raw:&[u8])->Option<String>{
    let end=raw.get(..16)?.iter().position(|&b|b==0)?;
    (end>0&&raw[..end].iter().all(u8::is_ascii_digit)).then(||String::from_utf8_lossy(&raw[..end]).into_owned())
}
fn overlay_score(ctx:*mut c_void){
    let Some(store)=SCORE_STORE.get() else{return};
    let Some(req)=read_u64(ctx as usize+0x70).map(|x|x as usize) else{return};if req==0{return}
    let mut head=[0u8;0x30];if !read(req,&mut head){return}
    let Some(player)=score_player(&head) else{return};
    let style=i32::from_le_bytes(head[0x10..0x14].try_into().unwrap());if !(0..=1).contains(&style){return}
    let cache=u64::from_le_bytes(head[0x20+style as usize*8..0x28+style as usize*8].try_into().unwrap()) as usize;
    if cache==0{return}
    let Ok(db)=store.lock() else{return};
    let Ok(rows)=db.rows(&player) else{return};
    let mut pending:HashMap<usize,[u8;0x30]>=HashMap::new();
    for (music,chart,best) in rows {
        if music<0||music as usize>=db.config.music_limit||chart/5!=style{continue}
        let idx=chart as usize%5;let dest=cache+music as usize*0x30;
        let mut row=if let Some(old)=pending.get(&dest){*old}else{let mut r=[0u8;0x30];if !read(dest,&mut r){continue}r};
        let present=row[0x2d];let ex=i32::from_le_bytes(row[idx*4..idx*4+4].try_into().unwrap());
        let bp=i32::from_le_bytes(row[0x14+idx*4..0x18+idx*4].try_into().unwrap());
        let lamp=row[0x28+idx] as i32;
        if present>1||lamp>7||!(-1..=300000).contains(&ex)||!(-1..=100000).contains(&bp){continue}
        let remote=if present!=0{score::Best{ex,bp,lamp}}else{score::Best::default()};
        if let Some(observed)=OBSERVED_EX.get(){if let Ok(mut map)=observed.lock(){map.insert((player.clone(),music,chart),remote.ex);}}
        let _=db.observe(&player,music,chart,remote);
        let merged=score::merge(remote,best);
        row[idx*4..idx*4+4].copy_from_slice(&merged.ex.to_le_bytes());
        row[0x14+idx*4..0x18+idx*4].copy_from_slice(&merged.bp.to_le_bytes());
        row[0x28+idx]=merged.lamp as u8;row[0x2d]=1;
        pending.insert(dest,row);
    }
    for (dest,row) in pending {if !write(dest,&row){log("IIDXScore 兼容分数缓存写入失败");}}
}
unsafe extern "system" fn appoint_hook(ctx:*mut c_void,node:*mut c_void)->bool{
    let result=ORIGINAL_APPOINT.get().map(|f|f(ctx,node)).unwrap_or(false);
    if result&&SCORE_ACTIVE.load(Ordering::Acquire){let _=std::panic::catch_unwind(||overlay_ghost(ctx));}
    result
}
fn overlay_ghost(ctx:*mut c_void){
    let Some(store)=SCORE_STORE.get() else{return};
    let Some(req)=read_u64(ctx as usize+0x70).map(|x|x as usize) else{return};
    let Some(response)=read_u64(ctx as usize+0x78).map(|x|x as usize) else{return};
    if req==0||response==0{return}
    let mut head=[0u8;0x120];if !read(req,&mut head){return}
    let Some(player)=score_player(&head[0x110..]) else{return};
    let music=i32::from_le_bytes(head[0..4].try_into().unwrap());
    let chart=i32::from_le_bytes(head[4..8].try_into().unwrap());
    let Ok(db)=store.lock() else{return};
    if music<0||music as usize>=db.config.music_limit||!(0..=9).contains(&chart){return}
    let Ok(Some((ex,curve,gauge)))=db.ghost(&player,music,chart) else{return};
    if OBSERVED_EX.get().and_then(|m|m.lock().ok()).and_then(|m|m.get(&(player,music,chart)).copied()).is_some_and(|remote|remote>ex){return}
    let mut current=[0u8;65];if !read(response+0x65,&mut current)||current[64]>1{return}
    let remote:i32=current[..64].iter().map(|&x|x as i32).sum();if current[64]!=0&&remote>=ex{return}
    if !write(response+0x65,&curve)||!write(response+0x3a8,&gauge){return}
    let _=write(response+0xa5,&[1]);let _=write(response+0x6a8,&[1]);let _=write(response+0x6d8,&[0]);
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
        // getrank identifies this player by the eight digits of idstr, not pc.get's pid.
        let pid = u32::from_le_bytes(bytes[0x28..0x2c].try_into().unwrap());
        if pid == 0 { log("pc.get 未返回有效 pid；身份未写入"); return; }
        if let Some(sender) = SENDER.get() { let _ = sender.send(Event::Profile(Profile { player: normalized, iidx_id, nickname })); }
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
fn reg_identity(tree: &Value) -> Option<(u32, u8)> {
    let attrs = &tree["children"].as_array()?.first()?["attributes"];
    let music = attrs["mid"].as_str()?.parse::<u32>().ok()?;
    let chart = attrs["clid"].as_str()?.parse::<u8>().ok()?;
    (chart < 10).then_some((music, chart))
}
fn process_captures(rx: mpsc::Receiver<Event>, catalog: Catalog, dir: PathBuf, url: String, game_dir: PathBuf) {
    let mut sessions: HashMap<String, Collection> = HashMap::new();
    let mut profiles: HashMap<String, Profile> = HashMap::new();
    let mut recent_requests: HashMap<String, std::time::Instant> = HashMap::new();
    let mut native_pending: HashMap<usize, (String, u32, u8, std::time::Instant)> = HashMap::new();
    let native_firenet = is_firenet_url(&url);
    let mut last_flush = std::time::Instant::now() - std::time::Duration::from_secs(60);
    loop {
        let event = match rx.recv_timeout(std::time::Duration::from_secs(1)) { Ok(e)=>Some(e), Err(mpsc::RecvTimeoutError::Timeout)=>None, Err(_)=>break };
        if last_flush.elapsed() >= std::time::Duration::from_secs(1) {
            if let Ok(settings)=push::load_settings(&game_dir) {
                let _=push::flush_once(&game_dir,&settings,log);
            }
            last_flush=std::time::Instant::now();
        }
        let Some(event)=event else { continue; };
        if let Event::NativeAck { context, music, chart } = event {
            if let Some((id, expected_music, expected_chart, _)) = native_pending.remove(&context) {
                if native_firenet && (music, chart) == (expected_music, expected_chart) {
                    match push::mark_native_uploaded(&game_dir, &id) {
                        Ok(true) => log(&format!("单局 {id} 火网原生提交已确认，标记已上传")),
                        Ok(false) => log(&format!("单局 {id} 已有上传标记")),
                        Err(e) => log(&format!("单局 {id} 原生确认写入失败: {e}")),
                    }
                } else { log(&format!("单局 {id} 原生响应曲目或谱面不匹配；不标记已上传")); }
            }
            continue;
        }
        if let Event::Play { context, tree } = &event {
            let digest=sha(&serde_json::to_vec(tree).unwrap_or_default());
            // The game retries one reg request within seconds. A two-minute
            // window can also hide a distinct short/restarted play with the
            // same score, so only coalesce the immediate transport retries.
            if recent_requests.get(&digest).is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(5)) { continue; }
            recent_requests.retain(|_,at| at.elapsed() < std::time::Duration::from_secs(5));
            let settings=push::load_settings(&game_dir).unwrap_or_default();
            let official=trusted_catalog(&catalog) && push::official_chart(tree,&catalog,&catalog);
            let auto_upload=official && settings.auto_sync && !settings.access_code.is_empty();
            let expect_native=official && native_firenet && REG_RESPONSE_ACTIVE.load(Ordering::Acquire);
            let mut play=push::new_event(tree.clone(),&settings.access_code,official);
            if expect_native {
                play.next_attempt_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default().as_millis().saturating_add(120_000) as u64;
            }
            match push::persist(&game_dir,&play) {
                Ok(_) => {
                    recent_requests.insert(digest,std::time::Instant::now());
                    if expect_native {
                        if let Some((music, chart)) = reg_identity(tree) {
                            native_pending.retain(|_, (_, _, _, at)| at.elapsed() < std::time::Duration::from_secs(300));
                            native_pending.insert(*context, (play.event_id.clone(), music, chart, std::time::Instant::now()));
                        }
                    }
                    log(&format!("单局 {} 已保存；{}",play.event_id,if expect_native {"等待火网原生提交响应"} else if auto_upload {"AC 曲目，等待同步"} else if !official {"omni/删除/未知曲目，禁止上传"} else {"自动同步未开启，仅本地保存"}));
                }
                Err(e) => log(&format!("单局保存失败: {e}")),
            }
            continue;
        }
        if let Event::Profile(profile) = event {
            log(&format!("pc.get 资料: 玩家键 {}，IIDX ID {}，昵称 {}", profile.player, profile.iidx_id, profile.nickname));
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
    log(&format!("AnyToFire {} 启动", env!("CARGO_PKG_VERSION")));
    let catalog_path = dir.join("data/info/0/music_data.bin");
    let catalog = match core::read_catalog(&catalog_path) { Ok(x) => x, Err(e) => { log(&format!("曲库读取失败: {e}")); return 0; } };
    if !matches!(catalog.version, 33 | 34) { log("不支持的曲库版本；未安装钩子"); return 0; }
    if !trusted_catalog(&catalog) { log("AC 曲库哈希与已核对基线不同；单局上传将被禁止"); }
    let _ = CATALOG_MAXIMUM.set(catalog.maximum);
    let url = server_url();
    log(&format!("EA URL: {}；曲库 {} 首", if url.is_empty() { "未知" } else { &url }, catalog.songs.len()));
    let catalog_version=catalog.version;
    let exports = dir.join("AnyToFire_exports"); if let Err(e) = fs::create_dir_all(&exports) { log(&format!("输出目录创建失败: {e}")); return 0; }
    let (tx, rx) = mpsc::channel(); let _ = SENDER.set(tx);
    std::thread::spawn({let dir=dir.clone(); move || process_captures(rx, catalog, exports, url, dir)});
    let name = wstr("bm2dx.dll"); let mut base = 0usize;
    for _ in 0..600 { base = GetModuleHandleW(name.as_ptr()) as usize; if base != 0 { break; } Sleep(100); }
    let Some(game_build) = (base != 0).then(|| detect_build(base, catalog_version)).flatten() else {
        log("未识别的 bm2dx.dll；不打开 IIDXScore 数据库或安装钩子"); return 0;
    };
    let _ = GAME_BUILD.set(game_build);
    log(&format!("已识别 bm2dx.dll {} (IIDX {})", game_build.name, game_build.version));
    let atf_score_enabled=push::load_settings(&dir).unwrap_or_default().omni_overlay;
    let old_score_enabled=score::legacy_enabled(&dir);
    if atf_score_enabled || old_score_enabled {
        if old_score_enabled && !atf_score_enabled {log("IIDXScore 已启用：AnyToFire 保留同格式存分能力，先获得钩子的一方负责存分");}
        let configured=score::config(&dir).and_then(|config|{
            if config.game_version!=catalog_version {return Err("IIDXScore 当前索引版本与游戏版本不同".into())}
            Ok(config)
        });
        let config=match configured {Ok(x)=>Ok(x),Err(e)=>{log(&format!("IIDXScore 配置未就绪，尝试当前 AC 曲库默认作用域: {e}"));score::fallback_config(&dir)}};
        match config.and_then(score::Store::open) {
            Ok(store)=>{log(&format!("IIDXScore 兼容数据库: {}；作用域 {}",store.config.database.display(),store.config.scope));let _=SCORE_STORE.set(Mutex::new(store));let _=OBSERVED_EX.set(Mutex::new(HashMap::new()));}
            Err(e)=>log(&format!("IIDXScore 兼容功能未启用: {e}")),
        }
    } else {log("AnyToFire 与 IIDXScore 的本地存分均未启用");}
    let avs_module = GetModuleHandleW(wstr("avs2-core.dll").as_ptr());
    if !avs_module.is_null() {
        if let Some(api)=AvsNodes::resolve(avs_module) { let _=AVS_NODES.set(api); }
    }
    if AVS_NODES.get().is_none() { log("AVS2 node API 不可用；单局采集不可用"); }
    if SCORE_STORE.get().is_none() && old_score_enabled {
        log("AnyToFire 无法打开兼容库；保留原生槽位，等待 IIDXScore 自行接管");return 0;
    }
    if !wait_for_score_hooks(base){log("IIDXScore 钩子仍处于安装中或槽位不一致；AnyToFire 不抢占，避免重复或漏存分");return 0;}
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

#[cfg(test)]
mod config_tests {
    use super::{command_option, spice_url};

    #[test]
    fn resolves_iidx_url_from_spice_config() {
        let xml = r#"<games><game name="Beatmania IIDX"><options><option name="url" value="http://server.bjmania.com:20573/core/?a=1&amp;b=2"/></options></game><game name="Other"><options><option name="url" value="wrong"/></options></game></games>"#;
        assert_eq!(spice_url(xml).as_deref(), Some("http://server.bjmania.com:20573/core/?a=1&b=2"));
    }

    #[test]
    fn parses_quoted_config_path() {
        let cmd = r#""C:\IIDX\spice64.exe" -cfgpath "C:\Users\Some User\spicetools.xml" -url "https://example.com/core/""#;
        assert_eq!(command_option(cmd, "-cfgpath").as_deref(), Some(r"C:\Users\Some User\spicetools.xml"));
        assert_eq!(command_option(cmd, "-url").as_deref(), Some("https://example.com/core/"));
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn only_firenet_hosts_enable_native_ack() {
        assert!(is_firenet_url("https://services.firenet.iidx.pro/core/"));
        assert!(is_firenet_url("https://firenet.iidx.pro/core/"));
        assert!(!is_firenet_url("https://firenet.iidx.pro.evil.example/core/"));
        assert!(!is_firenet_url("https://server.bjmania.com:20573/core/"));
    }
    #[test] fn native_ack_uses_music_and_chart_identity() {
        let request = serde_json::json!({"children":[{"attributes":{"mid":"34000","clid":"3"}}]});
        assert_eq!(reg_identity(&request), Some((34000, 3)));
        assert_eq!(reg_identity(&serde_json::json!({"children":[{"attributes":{"mid":"34000","clid":"12"}}]})), None);
    }
}
