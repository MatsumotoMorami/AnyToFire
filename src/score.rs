//! IIDXScore 0.5.x database and score snapshot compatibility.
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Best { pub ex: i32, pub bp: i32, pub lamp: i32 }
impl Default for Best { fn default() -> Self { Self { ex: -1, bp: -1, lamp: 0 } } }
pub fn merge(mut a: Best, b: Best) -> Best {
    a.ex = a.ex.max(b.ex); a.lamp = a.lamp.max(b.lamp);
    if b.bp >= 0 && (a.bp < 0 || b.bp < a.bp) { a.bp = b.bp; }
    a
}
pub struct Attempt { pub player: String, pub music: i32, pub chart: i32, pub pg: i32, pub great: i32, pub bp: i32, pub lamp: i32, pub nocount: i32, pub grade: i32, pub nosave: i32, pub raw: Vec<u8> }
impl Attempt {
    pub fn from_raw(raw: Vec<u8>, music_limit: usize) -> Option<Self> {
        if raw.len()!=0xea0 { return None; }
        let end=raw[..16].iter().position(|&b|b==0)?;
        if end==0 || !raw[..end].iter().all(u8::is_ascii_digit) { return None; }
        let int=|at:usize|i32::from_le_bytes(raw[at..at+4].try_into().unwrap());
        let a=Self {player:String::from_utf8(raw[..end].to_vec()).ok()?,music:int(0x10),chart:raw[0x14] as i8 as i32,pg:int(0x1c),great:int(0x20),bp:int(0x24),lamp:raw[0x28] as i8 as i32,nocount:int(0x374),grade:int(0x378),nosave:raw[0x7fe] as i32,raw};
        (a.music>=0 && (a.music as usize)<music_limit && (0..=9).contains(&a.chart) && (0..=100000).contains(&a.pg) && (0..=100000).contains(&a.great) && (-1..=100000).contains(&a.bp) && (0..=7).contains(&a.lamp)).then_some(a)
    }
    fn eligible(&self)->bool { self.nocount==0 && self.grade==0 && self.nosave==0 }
}
#[derive(Clone, Debug)]
pub struct Config { pub database: PathBuf, pub scope: String, pub music_limit: usize, pub game_version:u32, pub migrations:Vec<(String,Vec<(i32,i32,i32)>)> }
#[derive(Clone,PartialEq,Eq)]struct Song { title:[u8;256], levels:[u8;10] }
fn catalog(path:&Path)->Result<(u32,HashMap<i32,Song>,String,usize),String>{
    let data=fs::read(path).map_err(|e|format!("曲库 {}: {e}",path.display()))?;
    if data.len()<16||&data[..4]!=b"IIDX"{return Err(format!("曲库格式无效: {}",path.display()))}
    let word=|off|u32::from_le_bytes(data[off..off+4].try_into().unwrap());
    let ver=word(4);let count=word(8) as usize;let maximum=word(12) as usize;
    if !(ver==33||ver==34)||count==0||count>35000||maximum==0||maximum>100000||16usize.checked_add(maximum*4).and_then(|x|x.checked_add(count*2040))!=Some(data.len()) {return Err(format!("曲库布局无效: {}",path.display()))}
    let mut songs=HashMap::new();let start=16+maximum*4;
    for i in 0..count {let row=&data[start+i*2040..start+(i+1)*2040];let id=i32::from_le_bytes(row[0x67c..0x680].try_into().unwrap());
        if id<0||id as usize>=maximum{return Err("曲库曲号无效".into())}
        let mut title=[0;256];title.copy_from_slice(&row[..256]);let mut levels=[0;10];levels.copy_from_slice(&row[0x3ec..0x3f6]);
        if songs.insert(id,Song{title,levels}).is_some(){return Err("曲库曲号重复".into())}
    }
    Ok((ver,songs,format!("{:x}",Sha256::digest(&data)),maximum))
}
fn combine(ac:&mut HashMap<i32,Song>,omni:&HashMap<i32,Song>){for (&id,song) in omni{
    if let Some(existing)=ac.get_mut(&id){if existing.title==song.title{for i in 0..10{if existing.levels[i]==0{existing.levels[i]=song.levels[i];}}}}
    else{ac.insert(id,song.clone());}
}}
fn matching(old:&HashMap<i32,Song>,new:&HashMap<i32,Song>)->Vec<(i32,i32,i32)>{
    let mut old_counts=HashMap::<[u8;256],usize>::new();let mut new_counts=HashMap::<[u8;256],usize>::new();let mut new_ids=HashMap::<[u8;256],i32>::new();
    for song in old.values(){*old_counts.entry(song.title).or_default()+=1;}
    for (&id,song) in new{*new_counts.entry(song.title).or_default()+=1;new_ids.insert(song.title,id);}
    let mut out=Vec::new();
    for (&id,song) in old {let target=if new.get(&id).is_some_and(|current|current.title==song.title){Some(id)}else if old_counts[&song.title]==1&&new_counts.get(&song.title)==Some(&1){new_ids.get(&song.title).copied()}else{None};
        let Some(target)=target else{continue};let to=&new[&target];if song.title!=to.title{continue}
        for chart in 0..10{if song.levels[chart]!=0&&song.levels[chart]==to.levels[chart]{out.push((id,target,chart as i32));}}
    }
    out.sort();out
}
fn ini(game_dir:&Path)->HashMap<String,String> {
    let mut map=HashMap::new(); let Ok(text)=fs::read_to_string(game_dir.join("IIDXScore.ini")) else{return map};
    let mut section=false;
    for line in text.lines() { let line=line.trim(); if line.starts_with('[') {section=line.eq_ignore_ascii_case("[Score]");continue;} if !section||line.starts_with(';')||line.starts_with('#'){continue;} if let Some((k,v))=line.split_once('='){map.insert(k.trim().to_ascii_lowercase(),v.trim().to_string());} }
    map
}
pub fn legacy_enabled(game_dir:&Path)->bool {
    if !game_dir.join("IIDXScore.dll").is_file() || !game_dir.join("IIDXScore.ini").is_file(){return false}
    ini(game_dir).get("enabled").map_or(true,|value|value.parse::<i32>().unwrap_or(1)!=0)
}
fn configured_path(game_dir:&Path, name:&str)->PathBuf { let path=PathBuf::from(name);if path.is_absolute(){path}else{game_dir.join(path)} }
pub fn config(game_dir:&Path)->Result<Config,String>{
    let ini=ini(game_dir);
    let music=configured_path(game_dir,ini.get("musicdatabase").map(String::as_str).unwrap_or("data/info/0/music_data.bin"));
    let (ver,mut target_songs,hash,maximum)=catalog(&music)?;
    let mut scope=format!("{}/iidx{ver}/{hash}",ini.get("serverkey").map(String::as_str).unwrap_or("default"));
    let ac_scope=scope.clone();let mut migrations=Vec::new();let mut omni_scope=String::new();
    if let Some(name)=ini.get("omnimusicdatabase").filter(|x|!x.is_empty()){
        let path=configured_path(game_dir,name);let (omni_ver,omni_songs,omni_hash,_)=catalog(&path)?;
        omni_scope=format!("{}/iidx{omni_ver}/{omni_hash}",ini.get("serverkey").map(String::as_str).unwrap_or("default"));
        scope.push_str(&format!("/omni/{omni_hash}"));
        let ac_songs=target_songs.clone();combine(&mut target_songs,&omni_songs);
        migrations.push((ac_scope.clone(),matching(&ac_songs,&target_songs)));
        if omni_scope!=ac_scope{migrations.push((omni_scope.clone(),matching(&omni_songs,&target_songs)));}
    }
    for key in ["legacymusicdatabase","legacymusicdatabase2","legacymusicdatabase3","legacymusicdatabase4"] {
      if let Some(name)=ini.get(key).filter(|x|!x.is_empty()){
        let path=configured_path(game_dir,name);let (old_ver,old_songs,old_hash,_)=catalog(&path)?;
        let old_scope=format!("{}/iidx{old_ver}/{old_hash}",ini.get("serverkey").map(String::as_str).unwrap_or("default"));
        if old_scope!=scope&&old_scope!=omni_scope&&!migrations.iter().any(|(s,_)|s==&old_scope){migrations.push((old_scope,matching(&old_songs,&target_songs)));}
      }
    }
    let database=configured_path(game_dir,ini.get("databasepath").map(String::as_str).unwrap_or("scores/IIDXScore.sqlite3"));
    Ok(Config{database,scope,music_limit:maximum,game_version:ver,migrations})
}
pub fn fallback_config(game_dir:&Path)->Result<Config,String>{
    let ini=ini(game_dir);
    let music=game_dir.join("data/info/0/music_data.bin");
    let (ver,_,hash,maximum)=catalog(&music)?;
    let scope=format!("{}/iidx{ver}/{hash}",ini.get("serverkey").map(String::as_str).unwrap_or("default"));
    let database=configured_path(game_dir,ini.get("databasepath").map(String::as_str).unwrap_or("scores/IIDXScore.sqlite3"));
    Ok(Config{database,scope,music_limit:maximum,game_version:ver,migrations:vec![]})
}
pub struct Store { db:Connection, pub config:Config }
impl Store {
    pub fn open(config:Config)->Result<Self,String>{
        if let Some(parent)=config.database.parent(){fs::create_dir_all(parent).map_err(|e|e.to_string())?;}
        let db=Connection::open(&config.database).map_err(|e|e.to_string())?;
        db.busy_timeout(std::time::Duration::from_millis(1500)).map_err(|e|e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS attempts(id INTEGER PRIMARY KEY,scope TEXT NOT NULL,player TEXT NOT NULL,music INTEGER NOT NULL,chart INTEGER NOT NULL,pg INTEGER,great INTEGER,bp INTEGER,lamp INTEGER,eligible INTEGER,nocount INTEGER,grade INTEGER,nosave INTEGER,raw BLOB NOT NULL,created TEXT DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')));
          CREATE TABLE IF NOT EXISTS best(scope TEXT,player TEXT,music INTEGER,chart INTEGER,ex INTEGER,bp INTEGER,lamp INTEGER,PRIMARY KEY(scope,player,music,chart));
          CREATE TABLE IF NOT EXISTS cache_observations(scope TEXT,player TEXT,music INTEGER,chart INTEGER,ex INTEGER,bp INTEGER,lamp INTEGER,observed TEXT DEFAULT CURRENT_TIMESTAMP);
          CREATE INDEX IF NOT EXISTS attempts_player ON attempts(scope,player,music,chart);
          CREATE INDEX IF NOT EXISTS attempts_migration ON attempts(scope,music,chart);
          CREATE INDEX IF NOT EXISTS best_migration ON best(scope,music,chart);").map_err(|e|e.to_string())?;
        let mut store=Self{db,config};
        for (from,charts) in store.config.migrations.clone(){store.migrate(&from,&charts)?;}
        Ok(store)
    }
    fn migrate(&mut self,from:&str,charts:&[(i32,i32,i32)])->Result<(),String>{
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS migrated_attempts(source_scope TEXT NOT NULL,source_id INTEGER NOT NULL,target_scope TEXT NOT NULL,target_id INTEGER NOT NULL,PRIMARY KEY(source_scope,source_id,target_scope))").map_err(|e|e.to_string())?;
        let tx=self.db.transaction().map_err(|e|e.to_string())?;
        for &(music,target,chart) in charts {
            let mut q=tx.prepare("SELECT player,ex,bp,lamp FROM best WHERE scope=? AND music=? AND chart=?").map_err(|e|e.to_string())?;
            let rows=q.query_map(params![from,music,chart],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i32>(1)?,r.get::<_,i32>(2)?,r.get::<_,i32>(3)?))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
            drop(q);
            for (player,ex,bp,lamp) in rows{
                tx.execute("INSERT INTO best(scope,player,music,chart,ex,bp,lamp) VALUES(?,?,?,?,?,?,?) ON CONFLICT(scope,player,music,chart) DO UPDATE SET ex=max(best.ex,excluded.ex),lamp=max(best.lamp,excluded.lamp),bp=CASE WHEN excluded.bp<0 THEN best.bp WHEN best.bp<0 THEN excluded.bp ELSE min(best.bp,excluded.bp) END WHERE excluded.ex>best.ex OR excluded.lamp>best.lamp OR (excluded.bp>=0 AND (best.bp<0 OR excluded.bp<best.bp))",params![self.config.scope,player,target,chart,ex,bp,lamp]).map_err(|e|e.to_string())?;
            }
            let mut q=tx.prepare("SELECT a.id,a.raw FROM attempts a WHERE a.scope=? AND a.music=? AND a.chart=? AND a.eligible=1 AND NOT EXISTS(SELECT 1 FROM migrated_attempts m WHERE m.source_scope=a.scope AND m.source_id=a.id AND m.target_scope=?)").map_err(|e|e.to_string())?;
            let attempts=q.query_map(params![from,music,chart,self.config.scope],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
            drop(q);
            for (id,mut raw) in attempts{
                if music!=target {if raw.len()!=0xea0||i32::from_le_bytes(raw[0x10..0x14].try_into().unwrap())!=music{return Err("旧成绩快照曲号校验失败；迁移回滚".into())}raw[0x10..0x14].copy_from_slice(&target.to_le_bytes());}
                tx.execute("INSERT INTO attempts(scope,player,music,chart,pg,great,bp,lamp,eligible,nocount,grade,nosave,raw,created) SELECT ?,player,?,chart,pg,great,bp,lamp,eligible,nocount,grade,nosave,?,created FROM attempts WHERE id=?",params![self.config.scope,target,raw,id]).map_err(|e|e.to_string())?;
                let new_id=tx.last_insert_rowid();
                tx.execute("INSERT INTO migrated_attempts(source_scope,source_id,target_scope,target_id) VALUES(?,?,?,?)",params![from,id,self.config.scope,new_id]).map_err(|e|e.to_string())?;
            }
        }
        tx.commit().map_err(|e|e.to_string())
    }
    pub fn save(&mut self,a:&Attempt)->Result<(),String>{
        let tx=self.db.transaction().map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO attempts(scope,player,music,chart,pg,great,bp,lamp,eligible,nocount,grade,nosave,raw) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",params![self.config.scope,a.player,a.music,a.chart,a.pg,a.great,a.bp,a.lamp,a.eligible() as i32,a.nocount,a.grade,a.nosave,a.raw]).map_err(|e|e.to_string())?;
        if a.eligible(){tx.execute("INSERT INTO best VALUES(?,?,?,?,?,?,?) ON CONFLICT(scope,player,music,chart) DO UPDATE SET ex=max(ex,excluded.ex),lamp=max(lamp,excluded.lamp),bp=CASE WHEN excluded.bp<0 THEN bp WHEN bp<0 THEN excluded.bp ELSE min(bp,excluded.bp) END",params![self.config.scope,a.player,a.music,a.chart,a.pg*2+a.great,a.bp,a.lamp]).map_err(|e|e.to_string())?;}
        tx.commit().map_err(|e|e.to_string())
    }
    pub fn rows(&self,player:&str)->Result<Vec<(i32,i32,Best)>,String>{
        let mut q=self.db.prepare("SELECT music,chart,ex,bp,lamp FROM best WHERE scope=? AND player=? ORDER BY music,chart").map_err(|e|e.to_string())?;
        let rows=q.query_map(params![self.config.scope,player],|r|Ok((r.get(0)?,r.get(1)?,Best{ex:r.get(2)?,bp:r.get(3)?,lamp:r.get(4)?}))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string());
        rows
    }
    pub fn observe(&self,player:&str,music:i32,chart:i32,remote:Best)->Result<(),String>{
        self.db.execute("INSERT INTO cache_observations(scope,player,music,chart,ex,bp,lamp) VALUES(?,?,?,?,?,?,?)",params![self.config.scope,player,music,chart,remote.ex,remote.bp,remote.lamp]).map_err(|e|e.to_string())?;Ok(())
    }
    pub fn ghost(&self,player:&str,music:i32,chart:i32)->Result<Option<(i32,Vec<u8>,Vec<u8>)>,String>{
        let mut q=self.db.prepare("SELECT a.raw,b.ex FROM best b JOIN attempts a ON a.scope=b.scope AND a.player=b.player AND a.music=b.music AND a.chart=b.chart AND a.pg*2+a.great=b.ex WHERE b.scope=? AND b.player=? AND b.music=? AND b.chart=? AND a.eligible=1 AND a.nocount=0 AND a.grade=0 AND a.nosave=0 ORDER BY a.id ASC").map_err(|e|e.to_string())?;
        let mut rows=q.query(params![self.config.scope,player,music,chart]).map_err(|e|e.to_string())?;
        while let Some(row)=rows.next().map_err(|e|e.to_string())?{
            let raw:Vec<u8>=row.get(0).map_err(|e|e.to_string())?;let ex:i32=row.get(1).map_err(|e|e.to_string())?;
            if raw.len()!=0xea0 || !raw.starts_with(player.as_bytes()) || raw.get(player.len())!=Some(&0){continue}
            let at=|off|i32::from_le_bytes(raw[off..off+4].try_into().unwrap());
            if at(0x10)!=music||raw[0x14] as i32!=chart||at(0x374)!=0||at(0x378)!=0||raw[0x7fe]!=0||at(0x1c)*2+at(0x20)!=ex {continue}
            if raw[0x34..0x74].iter().map(|&b|b as i32).sum::<i32>()!=ex {continue}
            return Ok(Some((ex,raw[0x34..0x74].to_vec(),raw[0x74..0x374].to_vec())));
        }
        Ok(None)
    }
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn reused_id_can_map_by_unique_title(){
        let song=|name:u8|{let mut title=[0u8;256];title[0]=name;let mut levels=[0u8;10];levels[0]=12;Song{title,levels}};
        let old=HashMap::from([(100,song(b'A'))]);
        let new=HashMap::from([(100,song(b'B')),(200,song(b'A'))]);
        assert_eq!(matching(&old,&new),vec![(100,200,0)]);
    }
    #[test]fn best_merge(){assert_eq!(merge(Best{ex:10,bp:40,lamp:3},Best{ex:9,bp:20,lamp:5}),Best{ex:10,bp:20,lamp:5});}
    #[test]fn legacy_enabled_requires_dll_and_enabled_ini(){
        let dir=std::env::temp_dir().join(format!("atf-legacy-{}",crate::core::new_id()));fs::create_dir(&dir).unwrap();
        fs::write(dir.join("IIDXScore.ini"),"[Score]\nEnabled=1\n").unwrap();
        assert!(!legacy_enabled(&dir));
        fs::write(dir.join("IIDXScore.dll"),b"fixture").unwrap();
        assert!(legacy_enabled(&dir));
        fs::write(dir.join("IIDXScore.ini"),"[Score]\nEnabled=0\n").unwrap();
        assert!(!legacy_enabled(&dir));
        fs::remove_file(dir.join("IIDXScore.dll")).unwrap();fs::remove_file(dir.join("IIDXScore.ini")).unwrap();fs::remove_dir(dir).unwrap();
    }
    #[test]fn old_database_migration_preserves_snapshot_and_is_idempotent(){
        let path=std::env::temp_dir().join(format!("atf-score-{}.sqlite3",crate::core::new_id()));
        let old="server/iidx33/old".to_string();let new="server/iidx34/new".to_string();
        let base=Config{database:path.clone(),scope:old.clone(),music_limit:5000,game_version:33,migrations:vec![]};
        let mut store=Store::open(base).unwrap();
        let mut raw=vec![0u8;0xea0];raw[..4].copy_from_slice(b"123\0");raw[0x10..0x14].copy_from_slice(&1000i32.to_le_bytes());raw[0x14]=3;raw[0x1c..0x20].copy_from_slice(&100i32.to_le_bytes());raw[0x24..0x28].copy_from_slice(&20i32.to_le_bytes());raw[0x28]=5;
        for b in &mut raw[0x34..0x74]{*b=3;}for b in &mut raw[0x34..0x3c]{*b=4;}
        store.save(&Attempt::from_raw(raw,5000).unwrap()).unwrap();drop(store);
        let cfg=Config{database:path.clone(),scope:new.clone(),music_limit:5000,game_version:34,migrations:vec![(old.clone(),vec![(1000,2000,3)])]};
        let store=Store::open(cfg.clone()).unwrap();
        assert_eq!(store.rows("123").unwrap(),vec![(2000,3,Best{ex:200,bp:20,lamp:5})]);
        assert!(store.ghost("123",2000,3).unwrap().is_some());drop(store);
        let store=Store::open(cfg).unwrap();
        let count:i64=store.db.query_row("SELECT count(*) FROM attempts WHERE scope=?",params![new],|r|r.get(0)).unwrap();
        assert_eq!(count,1);drop(store);
        for suffix in ["","-wal","-shm"]{let _=fs::remove_file(format!("{}{}",path.display(),suffix));}
    }
}
