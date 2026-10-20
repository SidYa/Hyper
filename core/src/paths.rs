//! Well-known directories. All of them live under the per-user config dir
//! (`%APPDATA%\hyper` on Windows, `~/.config/hyper` on Linux).
//!
//! NOTE: Original Heroic stores everything under `%APPDATA%\heroic`. We use a
//! separate folder on purpose so both launchers can coexist; a migration from
//! the old folder is planned for a later phase.

use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "hyper";

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

pub fn settings_file() -> PathBuf {
    config_dir().join("config.json")
}

pub fn game_configs_dir() -> PathBuf {
    config_dir().join("GamesConfig")
}

pub fn logs_dir() -> PathBuf {
    config_dir().join("logs")
}

/// Per-game log files (install and launch output).
pub fn game_logs_dir() -> PathBuf {
    logs_dir().join("games")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(config_dir)
        .join(APP_DIR_NAME)
}

pub fn images_cache_dir() -> PathBuf {
    cache_dir().join("images")
}

/// Games added by hand: `games.json` + one folder of images per game.
pub fn sideload_dir() -> PathBuf {
    config_dir().join("sideload")
}

/// Encrypted credential blobs (see `secrets::Vault`).
pub fn vault_dir() -> PathBuf {
    config_dir().join("vault")
}

/// Key of the Unix vault backend (`secrets::key_file`): in the data dir
/// (`~/.local/share/hyper`), not next to the vault, so a copy of the
/// config folder alone decrypts nothing. Unused on Windows (DPAPI).
pub fn vault_key_file() -> PathBuf {
    let data = dirs::data_local_dir().unwrap_or_else(|| {
        tracing::warn!(
            "no data dir (XDG_DATA_HOME / HOME unset): the vault key lands next to the config"
        );
        config_dir()
    });
    data.join(APP_DIR_NAME).join("vault.key")
}

/// Browser profile used by the store login windows (cookies etc.).
pub fn webview_data_dir(store: &str) -> PathBuf {
    config_dir().join("webview").join(store)
}

/// The Epic client's files (`installed.json`, `manifests/`, `tmp/`), in
/// legendary's layout and under legendary's folder name, so a library made
/// by legendary / Heroic keeps working.
pub fn legendary_config_dir() -> PathBuf {
    config_dir().join("legendaryConfig").join("legendary")
}

/// The GOG client's own files (the support folder, the user profile), in
/// gogdl's layout.
pub fn gogdl_config_dir() -> PathBuf {
    config_dir().join("gogdlConfig").join("hyper_gogdl")
}

/// Root of the GOG client's files; the manifests sit in
/// `heroic_gogdl/manifests` below it, where gogdl kept them. Our own folder,
/// never `%APPDATA%\heroic_gogdl` of an installed Heroic.
pub fn gogdl_root_dir() -> PathBuf {
    config_dir().join("gogdlConfig")
}

/// Manifests of installed GOG games (needed for update / repair).
pub fn gogdl_manifests_dir() -> PathBuf {
    gogdl_root_dir().join("heroic_gogdl").join("manifests")
}

/// GOG support files (setup scripts) per game, outside the game folder.
pub fn gog_support_dir() -> PathBuf {
    gogdl_config_dir().join("support")
}

/// Installers of the launchers some Epic titles need (the EA app, Ubisoft
/// Connect); Heroic's `tools/redist/legendary`.
pub fn epic_redist_dir() -> PathBuf {
    config_dir().join("tools").join("redist").join("legendary")
}

/// Galaxy's shared redistributables (`__redist`: DirectX, the script
/// interpreter…), installed once for every GOG game (Heroic: `tools/redist/gog`).
pub fn gog_redist_dir() -> PathBuf {
    config_dir().join("tools").join("redist").join("gog")
}

/// Non-secret GOG profile (id + username), next to the gogdl config.
pub fn gog_user_file() -> PathBuf {
    gogdl_config_dir().join("user.json")
}

pub fn ensure_dirs() -> std::io::Result<()> {
    for dir in [
        config_dir(),
        game_configs_dir(),
        logs_dir(),
        game_logs_dir(),
        images_cache_dir(),
        vault_dir(),
        legendary_config_dir(),
        gogdl_config_dir(),
    ] {
        std::fs::create_dir_all(dir)?;
    }
    // credentials (the vault, the store folders where old versions left
    // plaintext files) and their parent: owner-only on Linux, also for
    // folders made by older versions with the default 0755
    for dir in [
        config_dir(),
        vault_dir(),
        config_dir().join("legendaryConfig"),
        legendary_config_dir(),
        gogdl_root_dir(),
        gogdl_config_dir(),
    ] {
        crate::secrets::create_private_dir(&dir)?;
    }
    Ok(())
}
