use std::{fs, io::{Read, Write}, path::Path, time::Duration};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const API: &str = "https://api.github.com/repos/MatsumotoMorami/AnyToFire/releases/latest";
#[derive(Deserialize)] struct Release { tag_name: String, draft: bool, prerelease: bool, assets: Vec<Asset> }
#[derive(Deserialize)] struct Asset { name: String, browser_download_url: String }
fn version(s: &str) -> Option<[u32;3]> {
    let parts: Vec<_> = s.trim_start_matches('v').split('.').collect();
    if parts.len()!=3 {return None} Some([parts[0].parse().ok()?,parts[1].parse().ok()?,parts[2].parse().ok()?])
}
pub fn newer(remote: &str, current: &str) -> bool { matches!((version(remote),version(current)),(Some(a),Some(b)) if a>b) }
fn fetch(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://github.com/MatsumotoMorami/AnyToFire/releases/download/") && url!=API {return Err("更新地址不属于项目 Release".into())}
    let response=ureq::AgentBuilder::new().timeout(Duration::from_secs(90)).build().get(url).set("User-Agent","AnyToFire-Updater").call().map_err(|e|e.to_string())?;
    let mut bytes=Vec::new(); response.into_reader().take(limit+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
    if bytes.len() as u64>limit {return Err("更新文件过大".into())} Ok(bytes)
}
fn verify(bytes: &[u8], checksum: &[u8]) -> Result<(), String> {
    let text=std::str::from_utf8(checksum).map_err(|_|"更新校验文件格式错误")?;
    let expected=text.split_whitespace().next().ok_or("更新校验文件为空")?;
    if expected.len()!=64 || !expected.bytes().all(|b|b.is_ascii_hexdigit()) || !format!("{:x}",Sha256::digest(bytes)).eq_ignore_ascii_case(expected) {return Err("更新包 SHA-256 校验失败".into())} Ok(())
}
/// Downloads only project release assets. The helper waits for EXE/game exit;
/// it replaces a fixed allowlist of binaries and never copies user data.
pub fn check_and_stage(binary: &Path) -> Result<Option<String>, String> {
    let release: Release=serde_json::from_slice(&fetch(API,2_000_000)?).map_err(|e|e.to_string())?;
    if release.draft || release.prerelease || !newer(&release.tag_name,env!("CARGO_PKG_VERSION")){return Ok(None)}
    let name=format!("AnyToFire-{}.zip",release.tag_name);
    let zip=release.assets.iter().find(|a|a.name==name).ok_or("新版尚无安装包")?;
    let sum=release.assets.iter().find(|a|a.name==format!("{name}.sha256")).ok_or("新版尚无校验文件，未安装")?;
    let bytes=fetch(&zip.browser_download_url,128*1024*1024)?;verify(&bytes,&fetch(&sum.browser_download_url,4096)?)?;
    let stage=std::env::temp_dir().join(format!("AnyToFire-update-{}",uuid::Uuid::new_v4()));fs::create_dir(&stage).map_err(|e|e.to_string())?;
    fs::write(stage.join("package.zip"),bytes).map_err(|e|e.to_string())?;
    let target=binary.parent().ok_or("无法确定安装目录")?;
    let config=serde_json::json!({"target":target,"pid":std::process::id()});
    fs::write(stage.join("config.json"),config.to_string()).map_err(|e|e.to_string())?;
    let script=stage.join("install.ps1");let mut f=fs::File::create(&script).map_err(|e|e.to_string())?;
    f.write_all(include_bytes!("update.ps1")).map_err(|e|e.to_string())?;
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("powershell.exe").args(["-NoLogo","-NoProfile","-ExecutionPolicy","Bypass","-File"]).arg(script).creation_flags(0x08000000).spawn().map_err(|e|e.to_string())?;
    }
    Ok(Some(release.tag_name))
}
#[cfg(test)] mod tests {
    use super::*;
    #[test]
    #[ignore = "reads published release and verifies download only; never stages or installs"]
    fn published_release_detected_from_previous_version() {
        let expected=std::env::var("ATF_EXPECT_RELEASE").expect("ATF_EXPECT_RELEASE");
        let current=std::env::var("ATF_PREVIOUS_VERSION").expect("ATF_PREVIOUS_VERSION");
        let release:Release=serde_json::from_slice(&fetch(API,2_000_000).unwrap()).unwrap();
        assert!(!release.draft && !release.prerelease);
        assert_eq!(release.tag_name,expected);
        assert!(newer(&release.tag_name,&current));
        let name=format!("AnyToFire-{}.zip",release.tag_name);
        let zip=release.assets.iter().find(|a|a.name==name).expect("ZIP asset");
        let sum=release.assets.iter().find(|a|a.name==format!("{name}.sha256")).expect("SHA256 asset");
        let bytes=fetch(&zip.browser_download_url,128*1024*1024).unwrap();
        verify(&bytes,&fetch(&sum.browser_download_url,4096).unwrap()).unwrap();
        println!("PASS: {current} detects {} and validates published ZIP/checksum; no install",release.tag_name);
    }
    #[test] fn versions_and_integrity() {
        assert!(newer("v0.3.10","0.3.9"));assert!(!newer("v0.3.7-rc1","0.3.6"));assert!(!newer("v0.3.7","0.3.7"));
        let hash=format!("{:x}  package.zip",Sha256::digest(b"test"));assert!(verify(b"test",hash.as_bytes()).is_ok());assert!(verify(b"changed",hash.as_bytes()).is_err());assert!(verify(b"test",b"").is_err());
    }
}
