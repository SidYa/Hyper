//! Games added by hand: any executable, not tied to a store (a game in
//! development, an old installer copy, an emulator…). The list lives in
//! `sideload/games.json`; each game's images (cover, background, the icon
//! taken from its executable) sit in `sideload/<app_name>/`.
//!
//! Removing such a game only forgets it: the game's own files are never
//! touched, it may well be someone's working project.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{GameLog, StoreContext, StoreManager};
use crate::error::{Error, Result};
use crate::events::CoreEvent;
use crate::paths;
use crate::types::{
    GameInfo, GameStatus, GameStatusKind, InstallInfo, InstallOptions, InstallPlatform,
    InstalledInfo, LaunchSpec, Runner, SideloadGame, SideloadInput, StoreStatus,
};

const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

pub struct Sideload {
    ctx: StoreContext,
    dir: PathBuf,
    games: RwLock<Vec<SideloadGame>>,
}

impl Sideload {
    pub fn new(ctx: StoreContext) -> Self {
        Self::with_dir(ctx, paths::sideload_dir())
    }

    fn with_dir(ctx: StoreContext, dir: PathBuf) -> Self {
        let games = load(&dir.join("games.json"));
        Self {
            ctx,
            dir,
            games: RwLock::new(games),
        }
    }

    /// Stored record, for the edit form.
    pub async fn get(&self, app_name: &str) -> Result<SideloadGame> {
        self.games
            .read()
            .await
            .iter()
            .find(|g| g.app_name == app_name)
            .cloned()
            .ok_or_else(|| Error::GameNotFound(app_name.to_string()))
    }

    pub async fn add(&self, input: SideloadInput) -> Result<GameInfo> {
        let (title, exe) = validate(&input)?;
        // the pictures are checked before the game's folder exists, so a
        // refused form leaves nothing behind
        check_image(input.cover.as_deref())?;
        check_image(input.background.as_deref())?;
        let app_name = new_app_name(&title);
        let game_dir = self.dir.join(&app_name);
        std::fs::create_dir_all(&game_dir)?;
        let mut game = SideloadGame {
            app_name: app_name.clone(),
            title,
            executable: exe,
            launch_args: input.launch_args.trim().to_string(),
            ..Default::default()
        };
        let images =
            replace_image(&game_dir, "cover", None, input.cover.as_deref()).and_then(|cover| {
                replace_image(&game_dir, "background", None, input.background.as_deref())
                    .map(|bg| (cover, bg))
            });
        let (cover, background) = match images {
            Ok(i) => i,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&game_dir);
                return Err(e);
            }
        };
        game.cover = cover;
        game.background = background;
        game.icon = extract_icon(&game_dir, &game.executable, None);

        let info = self.to_info(&game);
        {
            let mut games = self.games.write().await;
            games.push(game);
            self.save(&games)?;
        }
        tracing::info!(app = %app_name, "sideloaded game added");
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Sideload)));
        Ok(info)
    }

    pub async fn edit(&self, app_name: &str, input: SideloadInput) -> Result<GameInfo> {
        let (title, exe) = validate(&input)?;
        // only a game we know: its name is then one of ours, never a path
        if !self
            .games
            .read()
            .await
            .iter()
            .any(|g| g.app_name == app_name)
        {
            return Err(Error::GameNotFound(app_name.to_string()));
        }
        let game_dir = self.dir.join(app_name);
        std::fs::create_dir_all(&game_dir)?;
        let info = {
            let mut games = self.games.write().await;
            let game = games
                .iter_mut()
                .find(|g| g.app_name == app_name)
                .ok_or_else(|| Error::GameNotFound(app_name.to_string()))?;
            let exe_changed = !game.executable.eq_ignore_ascii_case(&exe);
            game.title = title;
            game.executable = exe;
            game.launch_args = input.launch_args.trim().to_string();
            game.cover = replace_image(
                &game_dir,
                "cover",
                game.cover.as_deref(),
                input.cover.as_deref(),
            )?;
            game.background = replace_image(
                &game_dir,
                "background",
                game.background.as_deref(),
                input.background.as_deref(),
            )?;
            if exe_changed || game.icon.is_none() {
                game.icon = extract_icon(&game_dir, &game.executable, game.icon.as_deref());
            }
            let info = self.to_info(game);
            self.save(&games)?;
            info
        };
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Sideload)));
        Ok(info)
    }

    /// After a move: the executable now lives in `new_path`.
    pub async fn change_install_path(&self, app_name: &str, new_path: &Path) -> Result<()> {
        let mut games = self.games.write().await;
        let g = games
            .iter_mut()
            .find(|g| g.app_name == app_name)
            .ok_or_else(|| Error::GameNotFound(app_name.to_string()))?;
        let file = Path::new(&g.executable)
            .file_name()
            .ok_or_else(|| Error::Other("the executable has no file name".into()))?;
        g.executable = new_path.join(file).to_string_lossy().into_owned();
        self.save(&games)?;
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Sideload)));
        Ok(())
    }

    fn save(&self, games: &[SideloadGame]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("games.json");
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(games)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn to_info(&self, g: &SideloadGame) -> GameInfo {
        let art = |file: &Option<String>| {
            file.as_ref().map(|f| {
                self.dir
                    .join(&g.app_name)
                    .join(f)
                    .to_string_lossy()
                    .into_owned()
            })
        };
        let exe = Path::new(&g.executable);
        GameInfo {
            runner: Some(Runner::Sideload),
            app_name: g.app_name.clone(),
            title: g.title.clone(),
            art_cover: art(&g.cover).unwrap_or_default(),
            art_square: art(&g.cover).unwrap_or_default(),
            art_background: art(&g.background),
            art_icon: art(&g.icon),
            is_installed: true,
            installable: false,
            can_run_offline: true,
            install: Some(InstalledInfo {
                executable: g.executable.clone(),
                install_path: exe
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                platform: InstallPlatform::Windows,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn emit_status(&self, app_name: &str, status: GameStatusKind) {
        self.ctx.events.emit(CoreEvent::GameStatus(GameStatus::new(
            Runner::Sideload,
            app_name,
            status,
        )));
    }
}

#[async_trait]
impl StoreManager for Sideload {
    async fn change_install_path(&self, app_name: &str, new_path: &Path) -> Result<()> {
        Sideload::change_install_path(self, app_name, new_path).await
    }

    fn runner(&self) -> Runner {
        Runner::Sideload
    }

    async fn status(&self) -> StoreStatus {
        StoreStatus {
            runner: Runner::Sideload,
            logged_in: true,
            username: None,
            helper_available: true,
        }
    }

    async fn login(&self, _code: &str) -> Result<StoreStatus> {
        Err(Error::NotImplemented("logging in to sideloaded games"))
    }

    async fn logout(&self) -> Result<()> {
        Ok(())
    }

    async fn library(&self, _force_refresh: bool) -> Result<Vec<GameInfo>> {
        Ok(self
            .games
            .read()
            .await
            .iter()
            .map(|g| self.to_info(g))
            .collect())
    }

    async fn install_info(
        &self,
        _app_name: &str,
        _platform: InstallPlatform,
    ) -> Result<InstallInfo> {
        Err(Error::NotImplemented("installing sideloaded games"))
    }

    async fn install(&self, _app_name: &str, _opts: InstallOptions) -> Result<()> {
        Err(Error::NotImplemented("installing sideloaded games"))
    }

    async fn update(&self, _app_name: &str) -> Result<()> {
        Err(Error::NotImplemented("updating sideloaded games"))
    }

    async fn cancel(&self, _app_name: &str) -> Result<()> {
        Ok(())
    }

    /// Forget the game (and its images). The game's files stay where they are.
    async fn uninstall(&self, app_name: &str) -> Result<()> {
        {
            let mut games = self.games.write().await;
            let before = games.len();
            games.retain(|g| g.app_name != app_name);
            if games.len() == before {
                return Err(Error::GameNotFound(app_name.to_string()));
            }
            self.save(&games)?;
        }
        let _ = std::fs::remove_dir_all(self.dir.join(app_name));
        tracing::info!(app = app_name, "sideloaded game removed from the library");
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Sideload)));
        Ok(())
    }

    async fn launch(&self, app_name: &str) -> Result<()> {
        let game = self.get(app_name).await?;
        let exe = PathBuf::from(&game.executable);
        if !exe.is_file() {
            return Err(Error::Other(format!("{} not found", game.executable)));
        }
        let ticket = self.ctx.games.prepare(Runner::Sideload, app_name)?;
        self.emit_status(app_name, GameStatusKind::Launching);
        let dir = exe.parent().map(Path::to_path_buf);
        let spec = LaunchSpec {
            executable: game.executable.clone(),
            args: crate::config::game::split_args(&game.launch_args),
            working_dir: dir
                .as_ref()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
            env: Vec::new(),
        };
        let log = GameLog::open(Runner::Sideload, app_name, "launch");
        log.write(&format!(
            "{} {}",
            spec.executable,
            crate::games::redact(&spec.args).join(" ")
        ));
        // the exe's folder is no install folder (it may be `System32`): the
        // game runner follows only the started process tree for sideloads
        if let Err(e) = self.ctx.games.launch(ticket, spec, dir).await {
            self.emit_status(app_name, GameStatusKind::Done);
            return Err(e);
        }
        Ok(())
    }
}

/// The list from disk. `games.json` can be edited by hand, and its names
/// become path components (`sideload/<app_name>/<image>`): an entry whose
/// app name or image names are not plain names is dropped.
fn load(path: &Path) -> Vec<SideloadGame> {
    let list: Vec<SideloadGame> = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, path = %path.display(), "sideload list unreadable; starting empty");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    list.into_iter().filter(|g| {
        let plain = super::is_plain_folder_name;
        let ok = plain(&g.app_name) && [&g.cover, &g.background, &g.icon].into_iter().flatten().all(|n| plain(n));
        if !ok {
            tracing::warn!(app = %g.app_name, "dropping a sideload entry whose names are not plain file names");
        }
        ok
    })
    .collect()
}

/// A picture in a supported format, before anything is copied.
fn check_image(change: Option<&str>) -> Result<()> {
    let Some(change) = change.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(());
    };
    let src = Path::new(change);
    let supported = src
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()));
    if !supported {
        return Err(Error::Other(format!("not a supported image: {change}")));
    }
    if !src.is_file() {
        return Err(Error::Other(format!("image not found: {change}")));
    }
    Ok(())
}

fn validate(input: &SideloadInput) -> Result<(String, String)> {
    let exe = input.executable.trim().trim_matches('"').to_string();
    if exe.is_empty() || !Path::new(&exe).is_file() {
        return Err(Error::Other(format!("executable not found: {exe}")));
    }
    let title = match input.title.trim() {
        "" => Path::new(&exe)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Game".into()),
        t => t.to_string(),
    };
    Ok((title, exe))
}

/// `my-game-1a2b3c`: readable, and unique thanks to the time suffix.
fn new_app_name(title: &str) -> String {
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let stamp = chrono::Utc::now().timestamp_millis();
    if slug.is_empty() {
        format!("game-{stamp:x}")
    } else {
        format!("{slug}-{stamp:x}")
    }
}

/// Apply one image field of the form. File names carry a timestamp so the
/// webview never shows a cached old picture after a change.
fn replace_image(
    dir: &Path,
    kind: &str,
    current: Option<&str>,
    change: Option<&str>,
) -> Result<Option<String>> {
    let Some(change) = change.map(str::trim) else {
        return Ok(current.map(str::to_string));
    };
    let current_path = current.map(|c| dir.join(c));
    if current_path
        .as_deref()
        .is_some_and(|p| same_path(p, Path::new(change)))
    {
        return Ok(current.map(str::to_string));
    }
    if !change.is_empty() {
        check_image(Some(change))?;
        let src = Path::new(change);
        let ext = src
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let name = format!("{kind}-{:x}.{ext}", chrono::Utc::now().timestamp_millis());
        std::fs::copy(src, dir.join(&name))?;
        if let Some(old) = current_path {
            let _ = std::fs::remove_file(old);
        }
        return Ok(Some(name));
    }
    if let Some(old) = current_path {
        let _ = std::fs::remove_file(old);
    }
    Ok(None)
}

fn extract_icon(dir: &Path, exe: &str, current: Option<&str>) -> Option<String> {
    let png = crate::platform::current().exe_icon_png(Path::new(exe))?;
    let name = format!("icon-{:x}.png", chrono::Utc::now().timestamp_millis());
    std::fs::write(dir.join(&name), png).ok()?;
    if let Some(old) = current {
        let _ = std::fs::remove_file(dir.join(old));
    }
    Some(name)
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::AppSettings;
    use crate::games::GameRunner;
    use crate::secrets::Vault;

    fn store(dir: &Path) -> Sideload {
        let events: crate::events::Events = Arc::new(crate::events::NullSink);
        let ctx = StoreContext {
            settings: Arc::new(RwLock::new(AppSettings::default())),
            vault: Arc::new(Vault::new(dir.join("vault"))),
            events: events.clone(),
            games: GameRunner::new(events),
        };
        Sideload::with_dir(ctx, dir.join("sideload"))
    }

    #[tokio::test]
    async fn add_edit_remove() {
        let exe = r"C:\Windows\explorer.exe";
        if !Path::new(exe).exists() {
            return;
        }
        let tmp = std::env::temp_dir().join(format!("hyper-sideload-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let picture = tmp.join("cover.png");
        std::fs::write(&picture, b"not really a png").unwrap();

        let s = store(&tmp);
        let input = SideloadInput {
            executable: exe.into(),
            launch_args: " -x ".into(),
            ..Default::default()
        };
        let added = s.add(input).await.unwrap();
        assert_eq!(added.title, "explorer", "title defaults to the file name");
        assert_eq!(added.runner, Some(Runner::Sideload));
        assert!(
            added
                .art_icon
                .as_deref()
                .is_some_and(|p| Path::new(p).is_file()),
            "icon extracted"
        );
        assert!(added.art_square.is_empty(), "no cover yet");

        let edit = SideloadInput {
            title: "My Game".into(),
            executable: exe.into(),
            cover: Some(picture.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let edited = s.edit(&added.app_name, edit).await.unwrap();
        assert_eq!(edited.title, "My Game");
        assert!(Path::new(&edited.art_square).is_file(), "cover copied in");
        assert_eq!(
            s.get(&added.app_name).await.unwrap().launch_args,
            "",
            "args replaced"
        );

        // reloads from disk
        let again = store(&tmp);
        assert_eq!(again.library(false).await.unwrap().len(), 1);

        s.uninstall(&added.app_name).await.unwrap();
        assert!(s.library(false).await.unwrap().is_empty());
        assert!(
            !tmp.join("sideload").join(&added.app_name).exists(),
            "images removed"
        );
        assert!(Path::new(exe).exists(), "the game itself is never touched");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn rejects_missing_executable() {
        let tmp =
            std::env::temp_dir().join(format!("hyper-sideload-missing-{}", std::process::id()));
        let s = store(&tmp);
        let input = SideloadInput {
            executable: r"C:\nope\game.exe".into(),
            ..Default::default()
        };
        assert!(s.add(input).await.is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn a_refused_picture_leaves_no_folder_behind() {
        let exe = r"C:\Windows\explorer.exe";
        if !Path::new(exe).exists() {
            return;
        }
        let tmp =
            std::env::temp_dir().join(format!("hyper-sideload-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let s = store(&tmp);
        let input = SideloadInput {
            executable: exe.into(),
            cover: Some(tmp.join("nope.png").to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert!(s.add(input).await.is_err());
        let input = SideloadInput {
            executable: exe.into(),
            cover: Some(exe.into()),
            ..Default::default()
        };
        assert!(s.add(input).await.is_err(), "not an image");
        let folders = std::fs::read_dir(tmp.join("sideload"))
            .map(|rd| rd.count())
            .unwrap_or(0);
        assert_eq!(folders, 0, "no game folder was made");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn hand_edited_entries_with_paths_as_names_are_dropped() {
        let tmp = std::env::temp_dir().join(format!("hyper-sideload-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("games.json");
        let list = vec![
            SideloadGame {
                app_name: "fine-1".into(),
                title: "A".into(),
                executable: "x.exe".into(),
                cover: Some("cover-1.png".into()),
                ..Default::default()
            },
            SideloadGame {
                app_name: "../escape".into(),
                title: "B".into(),
                executable: "x.exe".into(),
                ..Default::default()
            },
            SideloadGame {
                app_name: "fine-2".into(),
                title: "C".into(),
                executable: "x.exe".into(),
                icon: Some("../../vault/x".into()),
                ..Default::default()
            },
            SideloadGame {
                app_name: "fine-3".into(),
                title: "D".into(),
                executable: "x.exe".into(),
                background: Some(r"C:\Windows\x.png".into()),
                ..Default::default()
            },
            SideloadGame {
                app_name: "..".into(),
                title: "E".into(),
                executable: "x.exe".into(),
                ..Default::default()
            },
        ];
        std::fs::write(&file, serde_json::to_string(&list).unwrap()).unwrap();
        let kept: Vec<String> = load(&file).into_iter().map(|g| g.app_name).collect();
        assert_eq!(kept, ["fine-1"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn app_names_are_slugs() {
        let n = new_app_name("Моя Гра: Director's Cut!");
        assert!(n.starts_with("director-s-cut-"), "{n}");
        assert!(new_app_name("Гра").starts_with("game-"));
    }
}
