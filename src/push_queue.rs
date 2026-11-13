use super::{PlayEvent,SyncSettings};
use std::{collections::BTreeMap,fs,io,path::{Path,PathBuf},time::{Duration,Instant,SystemTime}};
#[derive(Clone,PartialEq,Eq)]
struct Stamp(u64,SystemTime);
fn stamp(path:&Path)->io::Result<Stamp>{let m=fs::metadata(path)?;Ok(Stamp(m.len(),m.modified()?))}
struct Changes(*mut std::ffi::c_void);
impl Changes{
 fn new(path:&Path)->io::Result<Self>{
  #[cfg(windows)]{use std::os::windows::ffi::OsStrExt;let p:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();let h=unsafe{FindFirstChangeNotificationW(p.as_ptr(),0,0x1|0x2|0x8|0x10)};if h.is_null()||h as isize == -1{return Err(io::Error::last_os_error())}Ok(Self(h))}
  #[cfg(not(windows))]{let _=path;Ok(Self(std::ptr::null_mut()))}
 }
 fn changed(&self)->bool{
  #[cfg(windows)]{if unsafe{WaitForSingleObject(self.0,0)}==0{unsafe{FindNextChangeNotification(self.0)};true}else{false}}
  #[cfg(not(windows))]{true}
 }
}
impl Drop for Changes{fn drop(&mut self){#[cfg(windows)]unsafe{FindCloseChangeNotification(self.0);}}}
#[cfg(windows)]
#[link(name="kernel32")]
extern "system"{
 fn FindFirstChangeNotificationW(path:*const u16,subtree:i32,filter:u32)->*mut std::ffi::c_void;
 fn FindNextChangeNotification(handle:*mut std::ffi::c_void)->i32;
 fn FindCloseChangeNotification(handle:*mut std::ffi::c_void)->i32;
 fn WaitForSingleObject(handle:*mut std::ffi::c_void,timeout:u32)->u32;
}
#[derive(Default)]
pub struct AutoSyncQueue{
 root:PathBuf,watch:Option<Changes>,checked:Option<Instant>,dirty:bool,
 files:BTreeMap<PathBuf,(Stamp,Option<PlayEvent>)>,
 settings:Option<(Option<Stamp>,SyncSettings)>,settings_file:PathBuf,
 #[cfg(test)] parsed:usize,
}
fn active(e:&PlayEvent)->bool{e.upload_allowed&&e.uploaded_at.is_none()&&e.blocked_reason.is_none()}
impl AutoSyncQueue{
 fn settings(&mut self,dir:&Path)->io::Result<SyncSettings>{
  let path=super::settings_path(dir);if self.settings_file!=path{self.settings=None;self.settings_file=path.clone();}let current=match stamp(&path){Ok(v)=>Some(v),Err(e)if e.kind()==io::ErrorKind::NotFound=>None,Err(e)=>return Err(e)};
  if self.settings.as_ref().is_none_or(|(s,_)|*s!=current){let settings=super::load_settings(dir)?;self.settings=Some((current,settings));}
  Ok(self.settings.as_ref().unwrap().1.clone())
 }
 fn refresh(&mut self,dir:&Path)->io::Result<()> {
  let queue=super::queue_path(dir);
  if self.root!=queue{self.root=queue.clone();self.files.clear();self.watch=None;self.checked=None;self.settings=None;self.dirty=true;}
  if !queue.exists(){self.files.clear();self.watch=None;self.checked=None;return Ok(())}
  if self.watch.is_none(){self.watch=Changes::new(&queue).ok();self.dirty=true;}
  let changed=self.watch.as_ref().is_some_and(Changes::changed);
  if !self.dirty&&!changed&&self.checked.is_some_and(|t|t.elapsed()<Duration::from_secs(60)){return Ok(())}
  // Reconcile names and stamps after a notification, but never reread unchanged history.
  let mut previous=std::mem::take(&mut self.files);let mut current=BTreeMap::new();
  let result=(||{for entry in fs::read_dir(&queue)?{
   let entry = entry?;
   let path = entry.path();
   if !path.extension().is_some_and(|x| x == "json") { continue; }
   match entry.file_type() {
    Ok(kind) if !kind.is_file() => continue,
    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
    Err(error) => return Err(error),
    Ok(_) => (),
   }
   let version = match stamp(&path) {
    Ok(version) => version,
    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
    Err(error) => return Err(error),
   };
   let item=if let Some((old,event))=previous.remove(&path).filter(|(old,_)|*old==version){(old,event)}else{
    #[cfg(test)]{self.parsed+=1;}
    let bytes = match fs::read(&path) {
     Ok(bytes) => bytes,
     Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
     Err(error) => return Err(error),
    };
    let event=serde_json::from_slice::<PlayEvent>(&bytes).ok().filter(active);(version,event)
   };current.insert(path,item);
  }Ok(())})();
  if result.is_err(){current.extend(previous.into_iter().filter(|(p,_)|p.is_file()));}self.files=current;
  self.dirty=result.is_err();if result.is_ok(){self.checked=Some(Instant::now());}result
 }
 pub fn flush(&mut self,dir:&Path,on_result:impl Fn(&str))->io::Result<()> {
  let settings=self.settings(dir)?;if !settings.auto_sync||settings.access_code.is_empty(){return Ok(())}
  self.refresh(dir)?;
  let mut pending:Vec<_>=self.files.iter().filter_map(|(p,(_,e))|e.as_ref().map(|e|(p.clone(),e.clone()))).collect();
  pending.sort_by(|a,b|a.1.occurred_at.cmp(&b.1.occurred_at));
  let result=super::flush_events(&mut pending,&settings,on_result);
  if result.is_err(){self.dirty=true;return result}
  for(path,event)in pending{if let Some((_,old))=self.files.get_mut(&path){*old=active(&event).then_some(event);}}
  Ok(())
 }
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn queue_reads_only_changed_files_and_observes_native_ack(){
  let dir=std::env::temp_dir().join(format!("atf-queue-cache-{}",uuid::Uuid::new_v4()));
  let a=super::super::new_event(serde_json::json!({"children":[]}),"",true);
  super::super::persist(&dir,&a).unwrap();let mut q=AutoSyncQueue::default();q.refresh(&dir).unwrap();assert_eq!(q.parsed,1);
  fs::create_dir(super::super::queue_path(&dir).join("ignored.json")).unwrap();
  assert_eq!(super::super::list(&dir).unwrap().len(), 1);
  for _ in 0..100{q.refresh(&dir).unwrap();}assert_eq!(q.parsed,1);
  super::super::mark_native_uploaded(&dir,&a.event_id).unwrap();
  for _ in 0..100{q.refresh(&dir).unwrap();if q.parsed==2{break}std::thread::sleep(Duration::from_millis(5));}
  assert_eq!(q.parsed,2);assert!(q.files.values().all(|(_,e)|e.is_none()));
  let b=super::super::new_event(serde_json::json!({"children":[]}),"",true);let p=super::super::persist(&dir,&b).unwrap();q.dirty=true;q.refresh(&dir).unwrap();assert_eq!(q.parsed,3);
  fs::remove_file(p).unwrap();q.dirty=true;q.refresh(&dir).unwrap();assert_eq!(q.files.len(),1);
  fs::remove_dir_all(dir).unwrap();
 }
 #[test]fn configured_readonly_queue_benchmark(){
  let Ok(dir)=std::env::var("ATF_QUEUE_BENCH_ROOT")else{return};let dir=PathBuf::from(dir);
  let start=Instant::now();let mut old_count=0;
  for _ in 0..100{old_count+=super::super::list(&dir).unwrap().len();}
  let old_ms=start.elapsed().as_secs_f64()*1000.0;
  let mut queue=AutoSyncQueue::default();let start=Instant::now();queue.refresh(&dir).unwrap();let initial=queue.parsed;
  let initial_ms=start.elapsed().as_secs_f64()*1000.0;
  let start=Instant::now();for _ in 0..100{queue.refresh(&dir).unwrap();}
  let new_ms=start.elapsed().as_secs_f64()*1000.0;
  assert_eq!(queue.parsed,initial);assert_eq!(old_count,initial*100);
  println!("QUEUE_BENCH files={} old_parse_count={} old_100_polls_ms={:.3} new_initial_ms={:.3} new_100_polls_ms={:.3} new_additional_parse_count={}",initial,old_count,old_ms,initial_ms,new_ms,queue.parsed-initial);
 }
 #[test]fn settings_refresh_and_retry_deadline_are_retained_without_network(){
  let dir=std::env::temp_dir().join(format!("atf-settings-cache-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&dir).unwrap();
  let mut q=AutoSyncQueue::default();assert!(!q.settings(&dir).unwrap().auto_sync);
  let settings=SyncSettings{auto_sync:true,access_code:"test-only".into(),..Default::default()};super::super::save_settings(&dir,&settings).unwrap();assert!(q.settings(&dir).unwrap().auto_sync);
  let mut event=super::super::new_event(serde_json::json!({"children":[]}),"test-only",true);event.next_attempt_ms=u64::MAX;
  super::super::persist(&dir,&event).unwrap();q.flush(&dir,|_|panic!("must not upload before retry deadline")).unwrap();assert_eq!(q.parsed,1);
  q.flush(&dir,|_|panic!("must not upload before retry deadline")).unwrap();assert_eq!(q.parsed,1);
  fs::remove_dir_all(dir).unwrap();
 }
}
