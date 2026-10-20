#![windows_subsystem = "windows"]
use anytofire::core::{self, Collection};
use anytofire::formats;
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

type Hwnd = *mut c_void;
type Hinst = *mut c_void;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf0000;
const WS_VISIBLE: u32 = 0x10000000;
const WS_CHILD: u32 = 0x40000000;
const WS_BORDER: u32 = 0x00800000;
const WS_VSCROLL: u32 = 0x00200000;
const LBS_EXTENDEDSEL: u32 = 0x0800;
const ES_AUTOHSCROLL: u32 = 0x0080;
const WM_DESTROY: u32 = 2;
const WM_COMMAND: u32 = 0x111;
const WM_SETFONT: u32 = 0x30;
const LB_ADDSTRING: u32 = 0x180;
const LB_RESETCONTENT: u32 = 0x184;
const LB_GETSELCOUNT: u32 = 0x190;
const LB_GETSELITEMS: u32 = 0x191;
const MB_ICONERROR: u32 = 0x10;
const MB_ICONINFORMATION: u32 = 0x40;
const OFN_OVERWRITEPROMPT: u32 = 2;

#[repr(C)] struct WndClass { style: u32, wnd_proc: Option<unsafe extern "system" fn(Hwnd,u32,usize,isize)->isize>, cls_extra: i32, wnd_extra: i32, instance: Hinst, icon: Hwnd, cursor: Hwnd, background: Hwnd, menu_name: *const u16, class_name: *const u16 }
#[repr(C)] struct Point { x: i32, y: i32 }
#[repr(C)] struct Msg { hwnd: Hwnd, message: u32, wparam: usize, lparam: isize, time: u32, point: Point, private: u32 }
#[repr(C)] struct OpenFileName { struct_size: u32, owner: Hwnd, instance: Hinst, filter: *const u16, custom_filter: *mut u16, max_custom_filter: u32, filter_index: u32, file: *mut u16, max_file: u32, file_title: *mut u16, max_file_title: u32, initial_dir: *const u16, title: *const u16, flags: u32, file_offset: u16, file_extension: u16, default_ext: *const u16, cust_data: isize, hook: *mut c_void, template_name: *const u16, reserved: *mut c_void, reserved_dword: u32, flags_ex: u32 }
#[link(name="user32")]
extern "system" {
    fn RegisterClassW(cls: *const WndClass) -> u16;
    fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: Hwnd, menu: Hwnd, instance: Hinst, param: *mut c_void) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn SendMessageW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    fn MessageBoxW(hwnd: Hwnd, text: *const u16, title: *const u16, flags: u32) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
}
#[link(name="kernel32")] extern "system" { fn GetModuleHandleW(name: *const u16) -> Hinst; }
#[link(name="comdlg32")] extern "system" { fn GetSaveFileNameW(ofn: *mut OpenFileName) -> i32; fn GetOpenFileNameW(ofn: *mut OpenFileName) -> i32; }
#[link(name="gdi32")] extern "system" { fn GetStockObject(index: i32) -> Hwnd; }

struct App { root: PathBuf, items: Vec<Collection>, list: Hwnd, edit: Hwnd, status: Hwnd }
unsafe impl Send for App {}
static APP: Mutex<Option<App>> = Mutex::new(None);
fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
fn popup(hwnd: Hwnd, message: &str, error: bool) { unsafe { MessageBoxW(hwnd, wide(message).as_ptr(), wide("AnyToFire").as_ptr(), if error { MB_ICONERROR } else { MB_ICONINFORMATION }); } }
fn set_text(hwnd: Hwnd, value: &str) { unsafe { SetWindowTextW(hwnd, wide(value).as_ptr()); } }
fn text_of(hwnd: Hwnd) -> String { let mut buf = vec![0u16; 1024]; let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) }; String::from_utf16_lossy(&buf[..n.max(0) as usize]) }
fn refresh(app: &mut App) {
    match core::load_collections(&app.root) {
        Ok(items) => app.items = items,
        Err(e) => { app.items.clear(); set_text(app.status, &format!("读取失败: {e}")); }
    }
    unsafe { SendMessageW(app.list, LB_RESETCONTENT, 0, 0); }
    for item in &app.items {
        let label = format!("{}   |   {}   |   {}   |   {}   |   {} 张谱面", item.name, item.iidx_id.as_deref().unwrap_or("IIDX ID 待识别"), item.nickname.as_deref().unwrap_or("昵称待识别"), item.captured_at, item.scores.len());
        unsafe { SendMessageW(app.list, LB_ADDSTRING, 0, wide(&label).as_ptr() as isize); }
    }
    set_text(app.status, &format!("{} 份成绩。多选后可以合并；CSV 会按 SP/DP 分成两个文件。", app.items.len()));
}
fn selected(app: &App) -> Vec<usize> {
    let count = unsafe { SendMessageW(app.list, LB_GETSELCOUNT, 0, 0) };
    if count <= 0 { return vec![]; }
    let mut indices = vec![0i32; count as usize];
    let n = unsafe { SendMessageW(app.list, LB_GETSELITEMS, indices.len(), indices.as_mut_ptr() as isize) };
    indices.into_iter().take(n.max(0) as usize).filter_map(|x| (x >= 0 && (x as usize) < app.items.len()).then_some(x as usize)).collect()
}
fn file_path(hwnd: Hwnd, suggested: &str, ext_name: &str, open: bool) -> Option<PathBuf> {
    let mut file = vec![0u16; 32768]; let initial = wide(suggested); file[..initial.len()].copy_from_slice(&initial);
    let filter = if open { wide("成绩文件 (*.json;*.csv;*.db)\0*.json;*.csv;*.db\0所有文件 (*.*)\0*.*\0") } else { match ext_name { "json" => wide("JSON 文件 (*.json)\0*.json\0所有文件 (*.*)\0*.*\0"), "csv" => wide("CSV 文件 (*.csv)\0*.csv\0所有文件 (*.*)\0*.*\0"), _ => wide("Asphyxia DB (*.db)\0*.db\0所有文件 (*.*)\0*.*\0") } };
    // wide() adds the final NUL; embedded NULs are intentional Win32 filter separators.
    let ext = wide(ext_name);
    let title = wide(if open { "选择要导入的成绩" } else { "选择另存为位置" });
    let mut ofn = OpenFileName { struct_size: std::mem::size_of::<OpenFileName>() as u32, owner: hwnd, instance: std::ptr::null_mut(), filter: filter.as_ptr(), custom_filter: std::ptr::null_mut(), max_custom_filter: 0, filter_index: 1, file: file.as_mut_ptr(), max_file: file.len() as u32, file_title: std::ptr::null_mut(), max_file_title: 0, initial_dir: std::ptr::null(), title: title.as_ptr(), flags: OFN_OVERWRITEPROMPT, file_offset: 0, file_extension: 0, default_ext: ext.as_ptr(), cust_data: 0, hook: std::ptr::null_mut(), template_name: std::ptr::null(), reserved: std::ptr::null_mut(), reserved_dword: 0, flags_ex: 0 };
    if unsafe { if open { GetOpenFileNameW(&mut ofn) } else { GetSaveFileNameW(&mut ofn) } } == 0 { return None; }
    let n = file.iter().position(|x| *x == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&file[..n])))
}
fn save_path(hwnd: Hwnd, suggested: &str, ext: &str) -> Option<PathBuf> { file_path(hwnd, suggested, ext, false) }
fn export_json(hwnd: Hwnd, item: &Collection) -> Result<(), String> {
    let Some(path) = save_path(hwnd, &format!("{}.json", item.name), "json") else { return Ok(()); };
    fs::write(&path, core::fervidex_json(item).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    popup(hwnd, &format!("已保存 {}", path.display()), false); Ok(())
}
fn export_csv(hwnd: Hwnd, item: &Collection) -> Result<(), String> {
    let Some(path) = save_path(hwnd, &format!("{}.csv", item.name), "csv") else { return Ok(()); };
    let has_sp = item.scores.iter().any(|s| s.chart < 5); let has_dp = item.scores.iter().any(|s| s.chart >= 5);
    if has_sp && has_dp {
        let stem = path.file_stem().and_then(|x| x.to_str()).unwrap_or("scores");
        let sp = path.with_file_name(format!("{stem}_SP.csv")); let dp = path.with_file_name(format!("{stem}_DP.csv"));
        fs::write(&sp, core::konami_csv(item, 0).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        fs::write(&dp, core::konami_csv(item, 1).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        popup(hwnd, &format!("已保存 {} 和 {}", sp.display(), dp.display()), false);
    } else {
        fs::write(&path, core::konami_csv(item, if has_dp { 1 } else { 0 }).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        popup(hwnd, &format!("已保存 {}", path.display()), false);
    }
    Ok(())
}
fn export_asphyxia(hwnd: Hwnd, item: &Collection) -> Result<(), String> {
    let Some(path) = save_path(hwnd, &format!("{}.db", item.name), "db") else { return Ok(()); };
    if path.exists() { return Err("请选择一个新的 .db 文件名；不会覆盖现有 Asphyxia 数据库".into()); }
    fs::write(&path, formats::asphyxia_db(item).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    popup(hwnd, &format!("已保存 {}", path.display()), false); Ok(())
}
fn import_scores(hwnd: Hwnd, app: &mut App) -> Result<(), String> {
    let Some(path) = file_path(hwnd, "", "", true) else { return Ok(()); };
    let catalog_path = app.root.parent().unwrap_or(Path::new(".")).join("data/info/0/music_data.bin");
    let catalog = core::read_catalog(&catalog_path).map_err(|e| format!("无法读取曲库索引 {}: {e}", catalog_path.display()))?;
    let imported = formats::import_path(&path, &catalog).map_err(|e| format!("导入失败: {e}"))?;
    let count = imported.collections.len();
    for item in &imported.collections { core::write_collection(&app.root, item).map_err(|e| e.to_string())?; }
    refresh(app); popup(hwnd, &format!("已导入 {count} 份成绩，跳过 {} 项无法映射的曲目或谱面", imported.skipped), false); Ok(())
}
fn merge_selected(hwnd: Hwnd, app: &mut App, indices: &[usize]) -> Result<(), String> {
    let sources: Vec<Collection> = indices.iter().map(|&i| app.items[i].clone()).collect();
    let merged = core::merge(&sources, "合并成绩".into())?;
    let Some(json_path) = save_path(hwnd, "合并成绩.json", "json") else { return Ok(()); };
    let target_dir = json_path.parent().ok_or("目标路径无效")?;
    let stem = json_path.file_stem().and_then(|x| x.to_str()).unwrap_or("合并成绩");
    let sp = target_dir.join(format!("{stem}_SP.csv")); let dp = target_dir.join(format!("{stem}_DP.csv"));
    let db = target_dir.join(format!("{stem}.db"));
    if sp.exists() || dp.exists() || db.exists() { return Err("目标目录已有同名 CSV 或 DB；请更换合并文件名".into()); }
    fs::write(&json_path, core::fervidex_json(&merged).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::write(&sp, core::konami_csv(&merged, 0).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::write(&dp, core::konami_csv(&merged, 1).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::write(&db, formats::asphyxia_db(&merged).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    core::write_collection(&app.root, &merged).map_err(|e| e.to_string())?;
    refresh(app); popup(hwnd, &format!("已合并 {} 份成绩，原成绩保留。\n保存至 {}", sources.len(), target_dir.display()), false); Ok(())
}
unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    if msg == WM_DESTROY { PostQuitMessage(0); return 0; }
    if msg == WM_COMMAND {
        let id = w & 0xffff;
        let mut guard = APP.lock().unwrap();
        if let Some(app) = guard.as_mut() {
            let indices = selected(app);
            let result = match id {
                101 => { refresh(app); Ok(()) }
                102 => { if indices.len() != 1 { Err("请选择一份成绩".into()) } else { export_json(hwnd, &app.items[indices[0]]) } }
                103 => { if indices.len() != 1 { Err("请选择一份成绩".into()) } else { export_csv(hwnd, &app.items[indices[0]]) } }
                104 => merge_selected(hwnd, app, &indices),
                106 => import_scores(hwnd, app),
                107 => { if indices.len() != 1 { Err("请选择一份成绩".into()) } else { export_asphyxia(hwnd, &app.items[indices[0]]) } }
                105 => { if indices.len() != 1 { Err("请选择一份成绩".into()) } else {
                    let name = text_of(app.edit).trim().to_string();
                    if name.is_empty() { Err("请输入新名称".into()) } else {
                        let mut item = app.items[indices[0]].clone(); item.name = name;
                        core::write_collection(&app.root, &item).map_err(|e| e.to_string()).map(|_| { refresh(app); })
                    }
                } },
                _ => Ok(()),
            };
            if let Err(e) = result { popup(hwnd, &e, true); }
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, w, l)
}
fn control(parent: Hwnd, class: &str, text: &str, id: usize, x: i32, y: i32, width: i32, height: i32, style: u32, instance: Hinst) -> Hwnd {
    let hwnd = unsafe { CreateWindowExW(0, wide(class).as_ptr(), wide(text).as_ptr(), WS_VISIBLE | WS_CHILD | style, x,y,width,height,parent,id as Hwnd,instance,std::ptr::null_mut()) };
    let font = unsafe { GetStockObject(17) }; unsafe { SendMessageW(hwnd, WM_SETFONT, font as usize, 1); }
    hwnd
}
fn main() {
    let root = std::env::args_os().skip_while(|x| x != "--data").nth(1).map(PathBuf::from).unwrap_or_else(|| std::env::current_exe().unwrap_or_default().parent().unwrap_or(Path::new(".")).join("AnyToFire_exports"));
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = wide("AnyToFireWindow");
    let wc = WndClass { style: 0, wnd_proc: Some(wnd_proc), cls_extra: 0, wnd_extra: 0, instance, icon: std::ptr::null_mut(), cursor: std::ptr::null_mut(), background: (6usize) as Hwnd, menu_name: std::ptr::null(), class_name: class.as_ptr() };
    if unsafe { RegisterClassW(&wc) } == 0 { return; }
    let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), wide("AnyToFire · 成绩管理").as_ptr(), WS_OVERLAPPEDWINDOW, 100,100,940,560,std::ptr::null_mut(),std::ptr::null_mut(),instance,std::ptr::null_mut()) };
    if hwnd.is_null() { return; }
    control(hwnd,"BUTTON","刷新",101,20,18,68,34,0,instance);
    control(hwnd,"BUTTON","导入成绩",106,98,18,92,34,0,instance);
    control(hwnd,"BUTTON","导出 Fervidex JSON",102,200,18,150,34,0,instance);
    control(hwnd,"BUTTON","导出 KONAMI CSV",103,360,18,150,34,0,instance);
    control(hwnd,"BUTTON","导出 Asphyxia DB",107,520,18,150,34,0,instance);
    control(hwnd,"BUTTON","合并选中成绩",104,680,18,145,34,0,instance);
    let list = control(hwnd,"LISTBOX","",201,20,64,882,365,WS_BORDER | WS_VSCROLL | LBS_EXTENDEDSEL,instance);
    let edit = control(hwnd,"EDIT","",202,20,445,350,30,WS_BORDER | ES_AUTOHSCROLL,instance);
    control(hwnd,"BUTTON","重命名选中成绩",105,380,443,160,34,0,instance);
    let status = control(hwnd,"STATIC","",203,20,489,882,26,0,instance);
    let mut app = App { root, items: vec![], list, edit, status }; refresh(&mut app); *APP.lock().unwrap() = Some(app);
    unsafe { ShowWindow(hwnd, 1); UpdateWindow(hwnd); let mut msg = std::mem::zeroed::<Msg>(); while GetMessageW(&mut msg, std::ptr::null_mut(),0,0) > 0 { TranslateMessage(&msg); DispatchMessageW(&msg); } }
}
