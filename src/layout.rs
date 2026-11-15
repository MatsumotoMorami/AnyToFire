use std::path::{Path, PathBuf};

pub const CATALOG_PATH: &str = "data/info/0/music_data.bin";
pub const PLUGIN_DLL: &str = "AnyToFire.dll";

fn find_game_dir(dir: &Path) -> Option<&Path> {
    dir.ancestors().find(|parent| parent.join(CATALOG_PATH).is_file())
}

/// The desktop app uses the closest ancestor containing the game's AC catalog.
pub fn game_dir_from_binary(binary: &Path) -> PathBuf {
    let dir = binary.parent().unwrap_or_else(|| Path::new("."));
    find_game_dir(dir).unwrap_or(dir).to_path_buf()
}

pub fn game_dir_for_plugin(binary: &Path, working_dir: Option<&Path>, host: Option<&Path>) -> PathBuf {
    working_dir.filter(|dir| dir.join(CATALOG_PATH).is_file())
        .or_else(|| host.and_then(Path::parent).and_then(find_game_dir))
        .or_else(|| working_dir.and_then(find_game_dir))
        .or_else(|| binary.parent().and_then(find_game_dir))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| game_dir_from_binary(binary))
}

fn hook_token(path: &Path) -> Option<String> {
    let token = path.to_str()?.replace('/', "\\");
    // Spice splits the hook list on whitespace. CFG and BAT both need a literal token.
    (!token.is_empty() && !token.bytes().any(|byte| {
        byte.is_ascii_whitespace() || matches!(byte, b'"' | b'%' | b'&' | b'^' | b'!' | b'<' | b'>' | b'|' | b'(' | b')')
    })).then_some(token)
}

#[cfg(windows)]
fn short_directory(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetShortPathNameW(path: *const u16, output: *mut u16, size: u32) -> u32;
    }
    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut output = vec![0; unsafe { GetShortPathNameW(input.as_ptr(), std::ptr::null_mut(), 0) } as usize];
    if output.is_empty() {
        return None;
    }
    let len = unsafe { GetShortPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32) } as usize;
    (len > 0 && len < output.len()).then(|| std::ffi::OsString::from_wide(&output[..len]).into())
}

pub fn plugin_hook_path(binary: &Path) -> Result<String, String> {
    let install_dir = binary.parent().ok_or("找不到 AnyToFire 安装目录")?;
    let dll = install_dir.join(PLUGIN_DLL);
    if !dll.is_file() {
        return Err(format!("找不到与 AnyToFire.exe 同目录的 DLL：{}", dll.display()));
    }
    let absolute = std::path::absolute(&dll).map_err(|error| error.to_string())?;
    if let Some(token) = hook_token(&absolute).filter(|token| token.is_ascii()) {
        return Ok(token);
    }
    #[cfg(windows)]
    if let Some(token) = absolute.parent().and_then(short_directory)
        .and_then(|dir| hook_token(&dir.join(PLUGIN_DLL)))
        .filter(|token| token.is_ascii())
    {
        return Ok(token);
    }

    Err(format!("Spice 的 -k 无法使用此绝对路径：{}。请将 AnyToFire 安装到绝对路径不含空格、非 ASCII 字符或批处理特殊字符的目录", absolute.display()))
}

/// Prefer the consolidated plugin configuration directory, retaining old installs.
pub fn config_path(game: &Path, name: &str) -> PathBuf {
    let plugins=game.join("plugins");
    if plugins.join(name).is_file() || (plugins.is_dir() && !game.join(name).is_file()) {plugins.join(name)} else {game.join(name)}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_install_uses_parent_game_dir_and_root_install_still_works() {
        let game = std::env::temp_dir().join(format!("atf-layout-{}", std::process::id()));
        std::fs::create_dir_all(game.join("data/info/0")).unwrap();
        std::fs::write(game.join("data/info/0/music_data.bin"), b"fixture").unwrap();
        assert_eq!(game_dir_from_binary(&game.join("AnyToFire/AnyToFire.exe")), game);
        assert_eq!(game_dir_from_binary(&game.join("AnyToFire.exe")), game);
        assert_eq!(game_dir_from_binary(&game.join("plugins/AnyToFire/AnyToFire.exe")), game);
        assert_eq!(game_dir_from_binary(&game.join("plugins/AnyToFire.dll")), game);
        let module = game.join("modules/score/custom/deep/AnyToFire.dll");
        assert_eq!(game_dir_from_binary(&module), game);
        let outside = game.with_extension("outside").join("AnyToFire.dll");
        assert_eq!(game_dir_for_plugin(&outside, Some(&game), None), game);
        assert_eq!(game_dir_for_plugin(&outside, None, Some(&game.join("spice64.exe"))), game);
        assert_eq!(game_dir_for_plugin(&outside, Some(&game.join("modules")), None), game);
        assert_eq!(game_dir_for_plugin(&outside, None, Some(&game.join("modules/spice64.exe"))), game);
        for dir in ["", "AnyToFire", "modules", "plugins/score/custom/deep"] {
            let install = game.join(dir);
            std::fs::create_dir_all(&install).unwrap();
            std::fs::write(install.join(PLUGIN_DLL), b"fixture").unwrap();
            let token = plugin_hook_path(&install.join("AnyToFire.exe")).unwrap();
            #[cfg(not(windows))]
            let token = token.replace('\\', "/");
            assert!(Path::new(&token).is_absolute());
            let resolved = Path::new(&token).canonicalize().unwrap();
            assert_eq!(resolved, install.join(PLUGIN_DLL).canonicalize().unwrap());
        }
        assert!(plugin_hook_path(&game.join("missing/AnyToFire.exe")).is_err());
        std::fs::create_dir_all(game.join("plugins")).unwrap();
        assert_eq!(config_path(&game,"IIDXScore.ini"),game.join("plugins/IIDXScore.ini"));
        std::fs::write(game.join("IIDXScore.ini"),"legacy").unwrap();
        assert_eq!(config_path(&game,"IIDXScore.ini"),game.join("IIDXScore.ini"));
        std::fs::write(game.join("plugins/IIDXScore.ini"),"new").unwrap();
        assert_eq!(config_path(&game,"IIDXScore.ini"),game.join("plugins/IIDXScore.ini"));
        std::fs::remove_dir_all(game).unwrap();
    }
}
