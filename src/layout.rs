use std::path::{Path, PathBuf};

pub const CATALOG_PATH: &str = "data/info/0/music_data.bin";
pub const PLUGIN_DLL: &str = "AnyToFire.dll";

fn find_game_dir(binary: &Path) -> Option<&Path> {
    binary.parent()?.ancestors().find(|dir| dir.join(CATALOG_PATH).is_file())
}

/// The desktop app uses the closest ancestor containing the game's AC catalog.
pub fn game_dir_from_binary(binary: &Path) -> PathBuf {
    find_game_dir(binary).or_else(|| binary.parent()).unwrap_or(Path::new(".")).to_path_buf()
}

/// Spice can load the DLL by absolute path from outside the game tree.
pub fn game_dir_for_plugin(binary: &Path, working_dir: Option<&Path>, host: Option<&Path>) -> PathBuf {
    working_dir.filter(|dir| dir.join(CATALOG_PATH).is_file())
        .or_else(|| find_game_dir(binary))
        .or_else(|| host.and_then(find_game_dir))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| game_dir_from_binary(binary))
}

pub fn plugin_hook_path(game_dir: &Path, binary: &Path) -> Result<String, String> {
    let install_dir = binary.parent().ok_or("找不到 AnyToFire 安装目录")?;
    let dll = install_dir.join(PLUGIN_DLL);
    if !dll.is_file() { return Err(format!("找不到与 AnyToFire.exe 同目录的 DLL：{}", dll.display())); }
    let relative = dll.strip_prefix(game_dir).unwrap_or(&dll);
    let hook = relative.to_str().ok_or("Spice 无法读取此 DLL 路径")?.replace('/', "\\");
    if hook.bytes().any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'"' | b'%' | b'&' | b'^' | b'!' | b'<' | b'>' | b'|')) {
        return Err("Spice 的 DLL 路径不能含空白或启动脚本特殊字符。请把三个 AnyToFire 文件放在游戏根目录或无空格的子目录".into());
    }
    Ok(hook)
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
        for dir in ["", "AnyToFire", "modules", "plugins/score/custom/deep"] {
            let install = game.join(dir);
            std::fs::create_dir_all(&install).unwrap();
            std::fs::write(install.join(PLUGIN_DLL), b"fixture").unwrap();
            let expected = Path::new(dir).join(PLUGIN_DLL).to_string_lossy().replace('/', "\\");
            assert_eq!(plugin_hook_path(&game, &install.join("AnyToFire.exe")).unwrap(), expected);
        }
        assert!(plugin_hook_path(&game, &game.join("missing/AnyToFire.exe")).is_err());
        std::fs::create_dir_all(game.join("plugins")).unwrap();
        assert_eq!(config_path(&game,"IIDXScore.ini"),game.join("plugins/IIDXScore.ini"));
        std::fs::write(game.join("IIDXScore.ini"),"legacy").unwrap();
        assert_eq!(config_path(&game,"IIDXScore.ini"),game.join("IIDXScore.ini"));
        std::fs::write(game.join("plugins/IIDXScore.ini"),"new").unwrap();
        assert_eq!(config_path(&game,"IIDXScore.ini"),game.join("plugins/IIDXScore.ini"));
        std::fs::remove_dir_all(game).unwrap();
    }
}
