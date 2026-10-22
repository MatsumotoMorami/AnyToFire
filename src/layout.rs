use std::path::{Path, PathBuf};

/// Binaries may live either in the game root (older releases) or in
/// `<game>/AnyToFire/`. User data and IIDXScore compatibility stay in the root.
pub fn game_dir_from_binary(binary: &Path) -> PathBuf {
    let parent = binary.parent().unwrap_or(Path::new("."));
    if parent.file_name().is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("AnyToFire")) {
        if let Some(game_dir) = parent.parent() {
            if game_dir.join("data/info/0/music_data.bin").is_file() { return game_dir.to_path_buf(); }
        }
    }
    parent.to_path_buf()
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
        std::fs::remove_dir_all(game).unwrap();
    }
}
