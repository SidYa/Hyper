//! Data shapes shared with the frontend. The TypeScript twin lives in
//! `app/src/types/index.ts`; keep both in sync.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    /// Epic Games Store, driven by `legendary`
    Legendary,
    /// GOG (`stores::gog`, a port of gogdl)
    Gog,
    /// Games added by hand (any executable, not from a store)
    Sideload,
}

impl Runner {
    pub fn all() -> [Runner; 3] {
        [Runner::Legendary, Runner::Gog, Runner::Sideload]
    }
    /// The web stores (login, store tab, import scan).
    pub fn stores() -> [Runner; 2] {
        [Runner::Legendary, Runner::Gog]
    }
    pub fn is_store(self) -> bool {
        self != Runner::Sideload
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Runner::Legendary => "Epic Games",
            Runner::Gog => "GOG",
            Runner::Sideload => "Other games",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Runner::Legendary => "legendary",
            Runner::Gog => "gog",
            Runner::Sideload => "sideload",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InstallPlatform {
    #[default]
    Windows,
    Linux,
    Mac,
}

impl InstallPlatform {
    /// Spelling `legendary --platform` expects.
    pub fn legendary_name(self) -> &'static str {
        match self {
            InstallPlatform::Windows => "Windows",
            InstallPlatform::Linux => "Linux",
            InstallPlatform::Mac => "Mac",
        }
    }
    pub fn from_legendary(s: &str) -> Self {
        match s {
            "Mac" => InstallPlatform::Mac,
            "Linux" => InstallPlatform::Linux,
            _ => InstallPlatform::Windows,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledInfo {
    pub executable: String,
    pub install_path: String,
    /// Human readable ("12.3 GB")
    pub install_size: String,
    pub install_size_bytes: u64,
    pub is_dlc: bool,
    pub version: String,
    pub platform: InstallPlatform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_dlcs: Option<Vec<String>>,
    /// GOG: the build was chosen by hand and is not updated.
    #[serde(default)]
    pub pinned_version: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    pub runner: Option<Runner>,
    pub app_name: String,
    pub title: String,
    pub art_cover: String,
    pub art_square: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_logo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_name: Option<String>,
    pub is_installed: bool,
    #[serde(default)]
    pub installable: bool,
    #[serde(default)]
    pub is_dlc: bool,
    #[serde(default)]
    pub can_run_offline: bool,
    #[serde(default)]
    pub cloud_save_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub third_party_managed_app: Option<String>,
    #[serde(default)]
    pub update_available: bool,
    #[serde(default)]
    pub install: Option<InstalledInfo>,
}

/// What a game is currently doing (mirrors Heroic's `GameStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameStatusKind {
    Queued,
    Installing,
    Updating,
    /// First start of a Windows game on Linux: umu fetches its runtime and
    /// Proton (once) and creates the game's prefix, before `Launching`.
    Preparing,
    Launching,
    Playing,
    Uninstalling,
    Repairing,
    Moving,
    /// Cloud saves are being downloaded (before a launch) or uploaded
    /// (after the game exited).
    Syncing,
    /// A download left over from the last session; waits for "Resume".
    Paused,
    Error,
    /// Clears the status in the UI.
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStatus {
    pub app_name: String,
    pub runner: Runner,
    pub status: GameStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<DownloadProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl GameStatus {
    pub fn new(runner: Runner, app_name: impl Into<String>, status: GameStatusKind) -> Self {
        Self {
            app_name: app_name.into(),
            runner,
            status,
            progress: None,
            message: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    /// 0..=100
    pub percent: f32,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// bytes / second
    pub speed: u64,
    /// bytes / second written to disk
    pub disk_speed: u64,
    /// seconds remaining
    pub eta: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreStatus {
    pub runner: Runner,
    pub logged_in: bool,
    pub username: Option<String>,
    pub helper_available: bool,
}

/// Sizes and versions needed before installing (from `legendary info`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    pub app_name: String,
    pub title: String,
    pub version: String,
    pub download_size: u64,
    pub disk_size: u64,
    pub platform: InstallPlatform,
    /// GOG: names of the game's other branches (beta…); the public one is unnamed.
    #[serde(default)]
    pub branches: Vec<String>,
    /// GOG: every build the account may install, newest first.
    #[serde(default)]
    pub builds: Vec<BuildEntry>,
}

/// One installable build of a GOG game (`builds` listing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildEntry {
    pub build_id: String,
    pub version: String,
    /// ISO-8601 publication date.
    pub date: String,
    /// `None` = the public branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// One add-on of a game, for the DLC list on the game page. Epic DLCs are
/// installed one by one into the game's folder (their `app_name` is a game
/// of its own to install / uninstall); GOG installs every owned DLC with
/// the game.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DlcEntry {
    pub app_name: String,
    pub title: String,
    pub installed: bool,
    /// Can be installed / removed on its own.
    pub installable: bool,
}

/// One achievement of a game, as the store reports it for the account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Achievement {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon_unlocked: String,
    pub icon_locked: String,
    /// Share of players who have it, in percent.
    pub rarity: f32,
    /// The store's wording for that share ("Rare", "Gold").
    pub rarity_label: String,
    /// Locked and the store hides what it is.
    pub hidden: bool,
    pub unlocked: bool,
    /// ISO-8601, when unlocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlocked_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Achievements {
    pub total: usize,
    pub unlocked: usize,
    /// Unlocked first (newest first), then locked.
    pub list: Vec<Achievement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOptions {
    /// Base folder; the game folder is created inside it.
    pub path: String,
    #[serde(default)]
    pub platform: InstallPlatform,
    /// GOG: a named branch instead of the public builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// GOG: this build instead of the branch's latest; updates then stay on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadKind {
    Install,
    Update,
    /// Verify the installed files and re-download broken ones.
    Repair,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadItem {
    pub runner: Runner,
    pub app_name: String,
    pub title: String,
    pub kind: DownloadKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub platform: InstallPlatform,
    pub running: bool,
    /// Restored from the last session: waits in the queue until the user
    /// resumes it (`DownloadManager::resume`).
    #[serde(default)]
    pub paused: bool,
    /// GOG install: branch / build chosen in the install dialog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
}

/// A game folder found on disk that belongs to an owned, not-installed title.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCandidate {
    pub runner: Runner,
    pub app_name: String,
    pub title: String,
    pub path: String,
}

/// Everything needed to start a game process (never sent to the UI).
#[derive(Debug, Clone, Default)]
pub struct LaunchSpec {
    pub executable: String,
    pub args: Vec<String>,
    pub working_dir: String,
    pub env: Vec<(String, String)>,
}

/// Store-page details fetched on demand for the game page (not part of the
/// library listing: one request per game, cached on disk).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameExtra {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// ISO-8601 date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Full-size screenshot URLs, in store order.
    #[serde(default)]
    pub screenshots: Vec<String>,
}

/// Time spent in a game, recorded by the game runner.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayStats {
    pub total_secs: u64,
    /// RFC 3339 timestamp of the last launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
}

/// A game added by hand, as stored in `sideload/games.json`. Image fields are
/// file names inside the game's own folder (`sideload/<app_name>/`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideloadGame {
    pub app_name: String,
    pub title: String,
    pub executable: String,
    /// Command-line arguments, split like a Windows command line.
    #[serde(default)]
    pub launch_args: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Icon taken from the executable; used by the generated card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Add / edit form of a sideloaded game. For the images: `None` keeps the
/// current one, an empty string removes it, a path copies that file in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideloadInput {
    pub title: String,
    pub executable: String,
    #[serde(default)]
    pub launch_args: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
}

/// One cloud-save folder of a game. Epic games have a single one (named
/// [`SaveLocation::EPIC`]); GOG games may have several, named by GOG.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveLocation {
    pub name: String,
    pub path: String,
}

impl SaveLocation {
    pub const EPIC: &'static str = "default";
}

/// A save folder as shown in the game's cloud-save settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveLocationInfo {
    pub name: String,
    /// Empty when the store's path could not be worked out.
    pub path: String,
    pub exists: bool,
}

/// Cloud-save state of one game.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSaves {
    /// The store keeps saves of this game in the cloud.
    pub supported: bool,
    pub locations: Vec<SaveLocationInfo>,
    /// Saves changed offline (or their upload failed) and are not in the
    /// cloud yet; uploaded once the store is reachable.
    #[serde(default)]
    pub pending_upload: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SaveSyncMode {
    /// Get newer saves from the cloud; never upload.
    Download,
    /// Send newer local saves to the cloud; never download.
    Upload,
    /// Replace the local saves with the cloud ones.
    ForceDownload,
    /// Replace the cloud saves with the local ones.
    ForceUpload,
}
