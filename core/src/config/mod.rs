//! Application settings. Serialised to `config.json` in the config directory.
//! Every field has a default so older config files keep loading when new
//! options are added.

pub mod game;

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::paths;
use crate::platform;
use crate::types::Runner;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleModeDefaultTab {
    #[default]
    All,
    Installed,
    Legendary,
    Gog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleModeKeyboardLayout {
    Tvos,
    #[default]
    Qwerty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    // --- General ---
    pub language: String,
    pub default_install_path: String,
    pub theme: String,
    /// "system" | "light" | "dark"
    pub appearance: String,
    /// Accent colour as #rrggbb
    pub accent_color: String,
    /// "Minimize to tray": closing the window hides it, downloads carry on.
    pub exit_to_tray: bool,
    pub max_workers: u32,
    /// Cloud saves: download before every launch, upload after the game exits.
    pub auto_sync_saves: bool,

    // --- Library ---
    /// Installed games first, then the rest (Heroic default).
    pub library_installed_first: bool,
    /// Not-installed covers in black and white (colour on hover).
    pub library_grayscale_uninstalled: bool,
    /// When a title is owned in both stores, show it only once.
    pub hide_duplicates: bool,
    /// Which store's copy to hide when `hide_duplicates` is on.
    pub hide_duplicates_from: Runner,

    // --- Console mode ---
    pub start_in_console_mode: bool,
    pub console_mode_default_tab: ConsoleModeDefaultTab,
    pub console_mode_keyboard_layout: ConsoleModeKeyboardLayout,
    pub console_mode_theme: String,
    pub console_mode_sounds: bool,
    /// Title under each tile (the focused title is always shown on top).
    pub console_mode_show_card_titles: bool,
    /// The store's name under the title (off: the store tabs tell already).
    pub console_mode_show_card_store: bool,
    pub console_show_progress_percent: bool,

    // --- Proton (Linux, see compat.rs) ---
    /// Wine prefixes, one per game; "" = `<default_install_path>/Prefixes`.
    pub wine_prefixes_dir: String,

    // --- Logs ---
    pub verbose_logs: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: "en".into(),
            default_install_path: platform::current()
                .default_install_path()
                .to_string_lossy()
                .into_owned(),
            theme: "midnightMirage".into(),
            appearance: "dark".into(),
            accent_color: "#6a4df6".into(),
            exit_to_tray: false,
            max_workers: 0,
            auto_sync_saves: false,

            library_installed_first: true,
            library_grayscale_uninstalled: false,
            hide_duplicates: false,
            hide_duplicates_from: Runner::Gog,

            start_in_console_mode: false,
            console_mode_default_tab: ConsoleModeDefaultTab::All,
            console_mode_keyboard_layout: ConsoleModeKeyboardLayout::Qwerty,
            console_mode_theme: "default".into(),
            console_mode_sounds: true,
            console_mode_show_card_titles: false,
            console_mode_show_card_store: false,
            console_show_progress_percent: true,

            wine_prefixes_dir: String::new(),

            verbose_logs: false,
        }
    }
}

impl AppSettings {
    /// The settings file, or the defaults when there is none. A file that
    /// cannot be read is kept aside as `config.json.broken-<time>` (the
    /// user's choices are in there) before the defaults are written.
    pub fn load_or_default() -> Self {
        let path = paths::settings_file();
        match Self::load(&path) {
            Ok(s) => s,
            Err(e) => {
                if path.exists() {
                    tracing::warn!(path = %path.display(), error = %e, "failed to read settings, using defaults");
                    set_aside_broken(&path);
                }
                let s = Self::default();
                if let Err(e) = s.save() {
                    tracing::warn!(error = %e, "failed to write default settings");
                }
                s
            }
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = paths::settings_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// Move a config file that failed to parse to `<name>.broken-<unix time>`,
/// so it is neither overwritten by defaults nor read again. Logged either way.
pub(crate) fn set_aside_broken(path: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".broken-{stamp}"));
    let aside = path.with_file_name(name);
    match std::fs::rename(path, &aside) {
        Ok(()) => {
            tracing::warn!(from = %path.display(), to = %aside.display(), "unreadable config file kept aside")
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not move the unreadable config file aside")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let s: AppSettings =
            serde_json::from_str(r#"{"language":"uk","somethingNew":1,"customCss":"x"}"#).unwrap();
        assert_eq!(s.language, "uk");
        assert!(s.console_mode_sounds);
    }

    #[test]
    fn a_broken_file_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("hyper-config-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(AppSettings::load(&file).is_err());
        set_aside_broken(&file);
        assert!(!file.exists());
        let kept: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("config.json.broken-"))
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join(&kept[0])).unwrap(),
            "{ not json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
