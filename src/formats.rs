use crate::core::{self, Catalog, Collection, Score, LAMPS};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, BufRead, BufReader, Cursor};
use std::path::Path;

const CHARTS: [&str; 10] = [
    "spb", "spn", "sph", "spa", "spl", "dpb", "dpn", "dph", "dpa", "dpl",
];

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn score(
    catalog: &Catalog,
    music: u32,
    chart: usize,
    ex: u32,
    bp: Option<u32>,
    lamp: u8,
    pgreat: Option<u32>,
    great: Option<u32>,
) -> io::Result<Score> {
    if chart >= 10 || lamp > 7 || ex > 300_000 || bp.is_some_and(|v| v > 100_000) {
        return Err(invalid("成绩字段超出范围"));
    }
    let song = catalog
        .songs
        .get(&music)
        .ok_or_else(|| invalid(format!("曲库中不存在曲目 {music}")))?;
    let level = song.levels[chart];
    if level == 0 {
        return Err(invalid(format!("曲目 {music} 不存在谱面 {chart}")));
    }
    Ok(Score {
        music,
        chart: chart as u8,
        ex,
        bp,
        lamp,
        title: song.title.clone(),
        genre: song.genre.clone(),
        artist: song.artist.clone(),
        level,
        levels: song.levels,
        pgreat,
        great,
    })
}
fn collection(
    catalog: &Catalog,
    name: String,
    scores: Vec<Score>,
    refid: Option<String>,
    iidx_id: Option<String>,
    nickname: Option<String>,
) -> io::Result<Collection> {
    if scores.is_empty() {
        return Err(invalid("文件中没有可导入的成绩"));
    }
    Ok(Collection {
        schema: 1,
        id: core::new_id(),
        name,
        server_url: "导入".into(),
        player_internal: String::new(),
        iidx_id,
        nickname,
        captured_at: core::now(),
        catalog_sha256: catalog.sha256.clone(),
        asphyxia_refid: refid,
        parents: vec![],
        scores,
    })
}
fn u32_field(value: &Value, key: &str) -> io::Result<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| invalid(format!("缺少或无效的 {key}")))
}
fn optional_u32(value: &Value, key: &str) -> io::Result<Option<u32>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| invalid(format!("无效的 {key}"))),
    }
}

pub fn import_fervidex(
    bytes: &[u8],
    catalog: &Catalog,
    name: String,
) -> io::Result<(Collection, usize)> {
    let value: Value = serde_json::from_slice(bytes)?;
    let songs = value
        .get("scores")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("不是 Fervidex static-profile JSON"))?;
    let mut scores = BTreeMap::new();
    let mut skipped = 0;
    for (id, charts) in songs {
        let music: u32 = id.parse().map_err(|_| invalid("无效曲目 ID"))?;
        let chart_map = charts.as_object().ok_or_else(|| invalid("无效谱面对象"))?;
        for (chart_name, value) in chart_map {
            let chart = CHARTS
                .iter()
                .position(|x| x == chart_name)
                .ok_or_else(|| invalid(format!("未知谱面 {chart_name}")))?;
            let ex = u32_field(value, "ex_score")?;
            let lamp =
                u8::try_from(u32_field(value, "clear_type")?).map_err(|_| invalid("无效灯值"))?;
            let bp = optional_u32(value, "miss_count")?;
            if catalog
                .songs
                .get(&music)
                .is_none_or(|song| song.levels[chart] == 0)
            {
                skipped += 1;
                continue;
            }
            scores.insert(
                (music, chart),
                score(catalog, music, chart, ex, bp, lamp, None, None)?,
            );
        }
    }
    Ok((
        collection(
            catalog,
            name,
            scores.into_values().collect(),
            None,
            None,
            None,
        )?,
        skipped,
    ))
}

pub fn import_konami_csv(
    bytes: &[u8],
    catalog: &Catalog,
    name: String,
) -> io::Result<(Collection, usize)> {
    let lower_name = name.to_ascii_lowercase();
    let style = if ["-sp", "_sp", " sp"].iter().any(|part| {
        lower_name.ends_with(part)
            || lower_name.contains(&format!("{part}-"))
            || lower_name.contains(&format!("{part}_"))
    }) {
        0
    } else if ["-dp", "_dp", " dp"].iter().any(|part| {
        lower_name.ends_with(part)
            || lower_name.contains(&format!("{part}-"))
            || lower_name.contains(&format!("{part}_"))
    }) {
        1
    } else {
        return Err(invalid("CSV 文件名必须包含 SP 或 DP（如 iidx-sp.csv）"));
    };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid("CSV 必须是 UTF-8"))?
        .trim_start_matches('\u{feff}');
    let mut reader = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(text.as_bytes());
    let header = reader.headers()?.clone();
    if header.len() != 41
        || header.get(0) != Some("バージョン")
        || header.get(1) != Some("タイトル")
        || header.get(5) != Some("BEGINNER 難易度")
    {
        return Err(invalid("不是 KONAMI IIDX CSV"));
    }
    let mut title_map: HashMap<&str, Vec<u32>> = HashMap::new();
    for (&id, song) in &catalog.songs {
        title_map.entry(&song.title).or_default().push(id);
    }
    let mut scores = BTreeMap::new();
    let mut skipped = 0;
    for record in reader.records() {
        let record = record?;
        let title = record.get(1).unwrap_or("");
        let version = record.get(0).unwrap_or("").parse::<u32>().ok();
        let artist = record.get(3).unwrap_or("");
        let candidates: Vec<u32> = title_map
            .get(title)
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| {
                let song = &catalog.songs[id];
                Some(id / 1000) == version && song.artist == artist
            })
            .collect();
        if candidates.len() != 1 {
            skipped += (0..5)
                .filter(|diff| {
                    let base = 5 + diff * 7;
                    record.get(base + 1).unwrap_or("0") != "0"
                        || record.get(base + 5).unwrap_or("NO PLAY") != "NO PLAY"
                })
                .count();
            continue;
        }
        let music = candidates[0];
        for diff in 0..5 {
            let base = 5 + diff * 7;
            let _csv_level = record.get(base).unwrap_or("0").parse::<u8>().unwrap_or(0);
            let ex = record
                .get(base + 1)
                .unwrap_or("0")
                .parse::<u32>()
                .map_err(|_| invalid("CSV 分数无效"))?;
            let lamp_text = record.get(base + 5).unwrap_or("NO PLAY");
            if ex == 0 && lamp_text == "NO PLAY" {
                continue;
            }
            let lamp = LAMPS
                .iter()
                .position(|x| x == &lamp_text)
                .ok_or_else(|| invalid(format!("未知灯值: {lamp_text}")))?
                as u8;
            let chart = style * 5 + diff;
            if catalog.songs[&music].levels[chart] == 0 {
                skipped += 1;
                continue;
            }
            let pgreat = record
                .get(base + 2)
                .filter(|s| !s.is_empty())
                .map(str::parse::<u32>)
                .transpose()
                .map_err(|_| invalid("CSV PGreat 无效"))?;
            let great = record
                .get(base + 3)
                .filter(|s| !s.is_empty())
                .map(str::parse::<u32>)
                .transpose()
                .map_err(|_| invalid("CSV Great 无效"))?;
            let bp = record
                .get(base + 4)
                .filter(|s| !s.is_empty() && *s != "---")
                .map(str::parse::<u32>)
                .transpose()
                .map_err(|_| invalid("CSV BP 无效"))?;
            scores.insert(
                (music, chart),
                score(catalog, music, chart, ex, bp, lamp, pgreat, great)?,
            );
        }
    }
    Ok((
        collection(
            catalog,
            name,
            scores.into_values().collect(),
            None,
            None,
            None,
        )?,
        skipped,
    ))
}

pub fn asphyxia_db(item: &Collection) -> io::Result<Vec<u8>> {
    let refid = item
        .asphyxia_refid
        .clone()
        .unwrap_or_else(|| format!("ANYTOFIRE-{}", item.id));
    let mut by_song: BTreeMap<u32, Vec<&Score>> = BTreeMap::new();
    for score in &item.scores {
        by_song.entry(score.music).or_default().push(score);
    }
    let mut out = Vec::new();
    for (mid, scores) in by_song {
        let mut pg = [0u32; 10];
        let mut g = [0u32; 10];
        let mut miss = [-1i32; 10];
        let mut clear = [0u8; 10];
        let mut ex = [0u32; 10];
        for score in scores {
            let i = score.chart as usize;
            if i >= 10 || score.lamp > 7 {
                return Err(invalid("成绩字段超出范围"));
            }
            pg[i] = score.pgreat.unwrap_or(0);
            g[i] = score.great.unwrap_or(0);
            miss[i] = score.bp.map(|n| n as i32).unwrap_or(-1);
            clear[i] = score.lamp;
            ex[i] = score.ex;
        }
        let digest = Sha256::digest(format!("{refid}:{mid}").as_bytes());
        let rank = [-1i32; 10];
        let options = [0u8; 10];
        let timestamp = chrono::Local::now().timestamp_millis();
        let row = json!({"__s":"plugins_profile","__a":"iidx@asphyxia","__refid":refid,"_id":format!("{:x}", digest)[..16].to_string(),"createdAt":{"$$date":timestamp},"updatedAt":{"$$date":timestamp},"collection":"score","mid":mid,"pgArray":pg,"gArray":g,"mArray":miss,"cArray":clear,"rArray":rank,"esArray":ex,"optArray":options,"opt2Array":options,"anytofire":{"iidx_id":item.iidx_id,"nickname":item.nickname}});
        serde_json::to_writer(&mut out, &row)?;
        out.push(b'\n');
    }
    Ok(out)
}

pub fn import_asphyxia_db(
    bytes: &[u8],
    catalog: &Catalog,
    name: String,
) -> io::Result<(Vec<Collection>, usize)> {
    let mut grouped: BTreeMap<String, BTreeMap<(u32, usize), Score>> = BTreeMap::new();
    let mut profiles: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut skipped = 0;
    for (line_no, line) in BufReader::new(Cursor::new(bytes)).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line)
            .map_err(|e| invalid(format!("DB 第 {} 行 JSON 无效: {e}", line_no + 1)))?;
        if value.get("__a").and_then(Value::as_str) != Some("iidx@asphyxia") {
            continue;
        }
        let Some(refid) = value.get("__refid").and_then(Value::as_str) else {
            continue;
        };
        if value.get("collection").and_then(Value::as_str) == Some("profile") {
            let id = value
                .get("idstr")
                .and_then(Value::as_str)
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    value
                        .get("id")
                        .and_then(Value::as_u64)
                        .map(|n| format!("{n:08}"))
                });
            let id = id
                .map(|x| x.replace('-', ""))
                .filter(|x| x.len() == 8 && x.bytes().all(|b| b.is_ascii_digit()))
                .map(|x| format!("{}-{}", &x[..4], &x[4..]));
            let nickname = value.get("name").and_then(Value::as_str).map(str::to_owned);
            profiles.insert(refid.to_owned(), (id, nickname));
            continue;
        }
        if value.get("collection").and_then(Value::as_str) != Some("score") {
            continue;
        }
        if let Some(meta) = value.get("anytofire") {
            profiles.entry(refid.to_owned()).or_insert_with(|| {
                (
                    meta.get("iidx_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    meta.get("nickname")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                )
            });
        }
        let mid = u32_field(&value, "mid")?;
        let arrays: Vec<&Vec<Value>> = ["esArray", "cArray", "mArray"]
            .iter()
            .map(|key| {
                value
                    .get(key)
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid(format!("DB 缺少 {key}")))
            })
            .collect::<io::Result<_>>()?;
        if arrays.iter().any(|x| x.len() != 10) {
            return Err(invalid(format!("曲目 {mid} 的数组长度不是 10")));
        }
        let pg = value.get("pgArray").and_then(Value::as_array);
        let great = value.get("gArray").and_then(Value::as_array);
        for chart in 0..10 {
            let ex = arrays[0][chart]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| invalid("DB 分数无效"))?;
            let lamp = arrays[1][chart]
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| invalid("DB 灯值无效"))?;
            let miss = arrays[2][chart]
                .as_i64()
                .ok_or_else(|| invalid("DB BP 无效"))?;
            if ex == 0 && lamp == 0 && miss < 0 {
                continue;
            }
            if catalog
                .songs
                .get(&mid)
                .is_none_or(|song| song.levels[chart] == 0)
            {
                skipped += 1;
                continue;
            }
            let bp = if miss < 0 {
                None
            } else {
                Some(u32::try_from(miss).map_err(|_| invalid("DB BP 超出范围"))?)
            };
            let pgreat = pg
                .and_then(|a| a.get(chart))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0);
            let g = great
                .and_then(|a| a.get(chart))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0);
            grouped.entry(refid.to_owned()).or_default().insert(
                (mid, chart),
                score(catalog, mid, chart, ex, bp, lamp, pgreat, g)?,
            );
        }
    }
    if grouped.is_empty() {
        return Err(invalid("DB 中没有 IIDX 成绩"));
    }
    let collections = grouped
        .into_iter()
        .map(|(refid, scores)| {
            let (iidx_id, nickname) = profiles.remove(&refid).unwrap_or_default();
            collection(
                catalog,
                format!(
                    "{name} · {}",
                    nickname.as_deref().or(iidx_id.as_deref()).unwrap_or(&refid)
                ),
                scores.into_values().collect(),
                Some(refid),
                iidx_id,
                nickname,
            )
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok((collections, skipped))
}

pub struct ImportResult {
    pub collections: Vec<Collection>,
    pub skipped: usize,
}
pub fn import_path(path: &Path, catalog: &Catalog) -> io::Result<ImportResult> {
    let bytes = fs::read(path)?;
    let name = path
        .file_stem()
        .and_then(|x| x.to_str())
        .unwrap_or("导入成绩")
        .to_string();
    match path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "json" => {
            let (item, skipped) = import_fervidex(&bytes, catalog, name)?;
            Ok(ImportResult {
                collections: vec![item],
                skipped,
            })
        }
        "csv" => {
            let (item, skipped) = import_konami_csv(&bytes, catalog, name)?;
            Ok(ImportResult {
                collections: vec![item],
                skipped,
            })
        }
        "db" => {
            let (collections, skipped) = import_asphyxia_db(&bytes, catalog, name)?;
            Ok(ImportResult {
                collections,
                skipped,
            })
        }
        _ => Err(invalid("请选择 .json、.csv 或 .db 文件")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Song;

    fn fixture() -> (Catalog, Collection) {
        let song = Song {
            title: "5.1.1.".into(),
            genre: "PIANO AMBIENT".into(),
            artist: "dj nagureo".into(),
            levels: [1, 2, 6, 10, 0, 0, 2, 6, 10, 0],
        };
        let catalog = Catalog {
            version: 34,
            maximum: 1001,
            sha256: "catalog".into(),
            songs: HashMap::from([(1000, song)]),
        };
        let item = collection(
            &catalog,
            "test".into(),
            vec![score(&catalog, 1000, 3, 1500, Some(21), 5, Some(700), Some(100)).unwrap()],
            None,
            Some("1234-5678".into()),
            Some("DJ".into()),
        )
        .unwrap();
        (catalog, item)
    }

    #[test]
    fn fervidex_roundtrip() {
        let (catalog, item) = fixture();
        let bytes = core::fervidex_json(&item).unwrap();
        let (imported, skipped) = import_fervidex(&bytes, &catalog, "fervidex".into()).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(imported.scores[0].ex, 1500);
        assert_eq!(imported.scores[0].bp, Some(21));
    }

    #[test]
    fn konami_sp_roundtrip_and_style_required() {
        let (catalog, item) = fixture();
        let bytes = core::konami_csv(&item, 0).unwrap();
        let (imported, skipped) = import_konami_csv(&bytes, &catalog, "iidx-sp".into()).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(imported.scores.len(), 1);
        assert_eq!(imported.scores[0].chart, 3);
        assert_eq!(imported.scores[0].pgreat, Some(700));
        assert!(import_konami_csv(&bytes, &catalog, "unknown".into()).is_err());
    }

    #[test]
    fn asphyxia_db_roundtrip() {
        let (catalog, item) = fixture();
        let bytes = asphyxia_db(&item).unwrap();
        let first: Value =
            serde_json::from_slice(bytes.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(first["collection"], "score");
        assert_eq!(first["__s"], "plugins_profile");
        assert_eq!(first["__a"], "iidx@asphyxia");
        assert_eq!(first["esArray"][3], 1500);
        let (imported, skipped) = import_asphyxia_db(&bytes, &catalog, "asphyxia".into()).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].scores[0].ex, 1500);
        assert_eq!(imported[0].scores[0].bp, Some(21));
        assert_eq!(
            imported[0].asphyxia_refid.as_deref(),
            first["__refid"].as_str()
        );
        assert_eq!(imported[0].iidx_id.as_deref(), Some("1234-5678"));
    }

    #[test]
    fn native_asphyxia_profile_joins_by_refid() {
        let (catalog, item) = fixture();
        let mut score: Value =
            serde_json::from_slice(asphyxia_db(&item).unwrap().trim_ascii()).unwrap();
        score["__refid"] = json!("REAL-REFID");
        score.as_object_mut().unwrap().remove("anytofire");
        let profile = json!({"__a":"iidx@asphyxia","__s":"plugins_profile","__refid":"REAL-REFID","collection":"profile","idstr":"87654321","name":"NATIVE"});
        let bytes = format!("{}\n{}\n", profile, score);
        let (imported, skipped) =
            import_asphyxia_db(bytes.as_bytes(), &catalog, "native".into()).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(imported[0].iidx_id.as_deref(), Some("8765-4321"));
        assert_eq!(imported[0].nickname.as_deref(), Some("NATIVE"));
        assert_eq!(imported[0].asphyxia_refid.as_deref(), Some("REAL-REFID"));
    }

    #[test]
    fn external_csv_sample_when_configured() {
        let (Ok(csv_path), Ok(catalog_path)) = (
            std::env::var("ANYTOFIRE_TEST_CSV"),
            std::env::var("ANYTOFIRE_TEST_CATALOG"),
        ) else {
            return;
        };
        let catalog = core::read_catalog(Path::new(&catalog_path)).unwrap();
        let path = Path::new(&csv_path);
        let imported = import_path(path, &catalog).unwrap();
        assert_eq!(imported.collections.len(), 1);
        assert!(imported.collections[0].scores.len() > 100);
        println!(
            "CSV sample: {} charts imported, {} unmapped items skipped",
            imported.collections[0].scores.len(),
            imported.skipped
        );
    }
}
