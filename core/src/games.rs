//! Running-game tracker: starts the game process from a [`LaunchSpec`],
//! reports `playing` / `done`, records play time, and can kill it.
//!
//! Many games start through a small launcher (`LimboLauncher.exe`, Unity /
//! Unreal bootstrappers) that spawns the real game and exits. Tracking only
//! the process we started would "lose" the game after a few seconds, so a
//! store game counts as running while *any* process whose executable lives
//! inside its install folder is alive, and killing it terminates all of them.
//! A sideloaded game has no install folder of its own (its executable may
//! sit anywhere, `C:\Windows\System32` included), so only the process tree
//! we started is followed there; the same for a store game whose folder
//! turns out to be, hold or lie inside a system folder ([`tracked_dir`]).
//!
//! Before the process exists a store has to prepare the launch (Epic: the
//! session refresh and the launch parameters, a few seconds). That phase is
//! tracked by a [`LaunchTicket`], so a kill during it cancels the launch
//! instead of failing with "not running" and letting the game start anyway.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tokio::process::Command;

use crate::compat::{Compat, Preparing};
use crate::error::{Error, Result};
use crate::events::{CoreEvent, Events};
use crate::playtime::PlayTime;
use crate::process::Killer;
use crate::types::{GameStatus, GameStatusKind, LaunchSpec, PlayStats, Runner};

const POLL: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct Running {
    /// The process we spawned; `None` once it exited (its PID may belong to
    /// anyone from then on).
    root: Option<Killer>,
    /// Lower-cased install folder with a trailing separator.
    dir: Option<String>,
}

#[derive(Default)]
struct Tracked {
    running: HashMap<String, Running>,
    /// Launches being prepared, keyed like `running`; `true` once cancelled.
    pending: HashMap<String, bool>,
    /// Run once the game exited, before it is reported `done` (cloud-save
    /// upload). Keyed like `running`.
    exit_hooks: HashMap<String, ExitHook>,
}

/// Work to do after a game exited; see [`GameRunner::on_exit`].
pub type ExitHook = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

pub struct GameRunner {
    events: Events,
    // A std mutex: it is never held across an await, and `LaunchTicket::drop`
    // has to take it synchronously.
    state: Mutex<Tracked>,
    count: AtomicUsize,
    playtime: PlayTime,
    /// Proton through umu, where Windows games need it (Linux).
    compat: OnceLock<Compat>,
}

/// Reserves a game while its launch is being prepared. Dropping it (launch
/// started, failed or cancelled) releases the reservation.
pub struct LaunchTicket {
    games: Arc<GameRunner>,
    runner: Runner,
    app_name: String,
}

impl LaunchTicket {
    /// True once [`GameRunner::kill`] was called for this game.
    pub fn is_cancelled(&self) -> bool {
        self.games
            .state()
            .pending
            .get(&key(self.runner, &self.app_name))
            .copied()
            .unwrap_or(false)
    }
}

impl Drop for LaunchTicket {
    fn drop(&mut self) {
        self.games
            .state()
            .pending
            .remove(&key(self.runner, &self.app_name));
    }
}

fn key(runner: Runner, app_name: &str) -> String {
    format!("{}:{}", runner.id(), app_name)
}

fn dir_key(dir: &Path) -> Option<String> {
    let s = dir.to_string_lossy().replace('/', "\\").to_lowercase();
    if s.len() < 4 {
        // never treat a drive root (or empty path) as a game folder
        return None;
    }
    Some(if s.ends_with('\\') {
        s
    } else {
        format!("{s}\\")
    })
}

/// Folders of the OS itself: a game folder that is, holds or lies inside
/// one makes the folder heuristic kill every program from there.
fn os_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = [
        "/", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/var", "/boot",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    if let Some(windir) = std::env::var_os("WINDIR") {
        out.push(PathBuf::from(windir));
    }
    out
}

/// Whether `dir` lies inside one of `roots` (strictly below it).
fn is_inside(dir: &Path, roots: &[PathBuf]) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
            .replace('/', "\\")
    };
    let target = norm(dir);
    // `/` normalises to nothing; it is caught as "no folder name" instead
    roots
        .iter()
        .map(|p| norm(p))
        .filter(|p| !p.is_empty())
        .any(|p| target.starts_with(&format!("{p}\\")))
}

/// The folder whose processes count as the game, from the store's install
/// folder (or the working directory). `None` = follow only the process tree
/// we started: always for a sideloaded game (its "folder" is just where its
/// executable happens to be), and for a folder that is, holds or lies inside
/// an OS folder (`C:\Windows`, `/usr`), or is / holds a user or program
/// folder (home, Documents, Program Files, `/opt`, the games library, our
/// config): "Stop" must never terminate every program running from there.
fn tracked_dir(runner: Runner, install_dir: Option<PathBuf>, working_dir: &str) -> Option<String> {
    if runner == Runner::Sideload {
        return None;
    }
    let dir =
        install_dir.or_else(|| (!working_dir.is_empty()).then(|| PathBuf::from(working_dir)))?;
    if !dir.is_absolute()
        || !dir.components().any(|c| matches!(c, Component::Normal(_)))
        || dir
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return None;
    }
    let os = os_dirs();
    let mut protected = crate::stores::protected_dirs("");
    protected.extend(os.iter().cloned());
    protected.extend(["/opt", "/home"].into_iter().map(PathBuf::from));
    let refused = crate::stores::is_or_contains(&dir, &protected) || is_inside(&dir, &os);
    if refused {
        tracing::warn!(dir = %dir.display(), "not a game's own folder: following only the started process tree");
        return None;
    }
    dir_key(&dir)
}

/// Processes whose executable is inside `dir` (excluding ourselves), with
/// their start times.
fn processes_in(dir: &str) -> Vec<(sysinfo::Pid, u64)> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    let me = sysinfo::get_current_pid().ok();
    sys.processes()
        .iter()
        .filter(|(pid, _)| Some(**pid) != me)
        .filter(|(_, p)| {
            p.exe()
                .map(|e| {
                    e.to_string_lossy()
                        .replace('/', "\\")
                        .to_lowercase()
                        .starts_with(dir)
                })
                .unwrap_or(false)
        })
        .map(|(pid, p)| (*pid, p.start_time()))
        .collect()
}

/// Copy a child's output into `log` line by line, secrets hidden.
pub(crate) fn log_filtered<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    pipe: Option<R>,
    log: &std::fs::File,
) {
    let (Some(pipe), Ok(mut log)) = (pipe, log.try_clone()) else {
        return;
    };
    tokio::spawn(async move {
        use std::io::Write;
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = writeln!(log, "{}", redact_text(&line));
        }
    });
}

/// [`redact`] for free text (a logged command line, `KEY=value` lists):
/// the value after any secret-looking key is hidden.
pub(crate) fn redact_text(line: &str) -> std::borrow::Cow<'_, str> {
    static RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(?i)((?:password|token|auth_|ovt|exchangecode|epicuser|secret)[a-z0-9_]*[=:]\s*['"]?)[^\s'",;&]+"#).ok()
    });
    match re {
        Some(re) => re.replace_all(line, "${1}<redacted>"),
        None => std::borrow::Cow::Borrowed(line),
    }
}

/// Launch and helper arguments carry one-time auth tokens and the Epic
/// account (`-epicuserid=`, `-epicusername=`); keep them out of the logs.
/// `--code X` / `--token X` style flags hide the value that follows.
pub(crate) fn redact(args: &[String]) -> Vec<String> {
    const KEYS: [&str; 7] = [
        "password",
        "token",
        "auth_",
        "ovt",
        "exchangecode",
        "epicuser",
        "secret",
    ];
    const VALUE_FLAGS: [&str; 4] = ["--code", "--token", "--ex-token", "--session-id"];
    let mut hide_next = false;
    args.iter()
        .map(|a| {
            if std::mem::take(&mut hide_next) {
                return "<redacted>".into();
            }
            let lower = a.to_lowercase();
            if VALUE_FLAGS.contains(&lower.as_str()) {
                hide_next = true;
                return a.clone();
            }
            let secret = KEYS.iter().any(|k| lower.contains(k));
            match (secret, a.split_once('=')) {
                (true, Some((k, _))) => format!("{k}=<redacted>"),
                (true, None) => "<redacted>".into(),
                _ => a.clone(),
            }
        })
        .collect()
}

impl GameRunner {
    pub fn new(events: Events) -> Arc<Self> {
        Arc::new(Self {
            events,
            state: Mutex::new(Tracked::default()),
            count: AtomicUsize::new(0),
            playtime: PlayTime::new(crate::paths::config_dir().join("playtime.json")),
            compat: OnceLock::new(),
        })
    }

    /// Start Windows games through umu + Proton from now on (Linux only;
    /// see `Platform::needs_wine`).
    pub fn enable_compat(&self, compat: Compat) {
        let _ = self.compat.set(compat);
    }

    /// The Proton wrapper (Linux), for running helper programs in a game's prefix.
    pub fn compat(&self) -> Option<&Compat> {
        self.compat.get()
    }

    fn state(&self) -> MutexGuard<'_, Tracked> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_running(&self, runner: Runner, app_name: &str) -> bool {
        self.state().running.contains_key(&key(runner, app_name))
    }

    /// Cheap, lock-free check used by the controller watcher.
    pub fn any_running(&self) -> bool {
        self.count.load(Ordering::Relaxed) > 0
    }

    pub fn running_keys(&self) -> Vec<String> {
        self.state().running.keys().cloned().collect()
    }

    pub fn play_stats(&self, runner: Runner, app_name: &str) -> PlayStats {
        self.playtime.get(&key(runner, app_name))
    }

    /// Running, or its launch is being prepared.
    pub fn is_active(&self, runner: Runner, app_name: &str) -> bool {
        let k = key(runner, app_name);
        let st = self.state();
        st.running.contains_key(&k) || st.pending.contains_key(&k)
    }

    /// Run `hook` after the next launch of the game exits, while the game
    /// still counts as running (nothing else touches its files meanwhile).
    /// Replaced by a later call; dropped by [`GameRunner::clear_exit_hook`]
    /// when the launch did not happen.
    pub fn on_exit(&self, runner: Runner, app_name: &str, hook: ExitHook) {
        self.state().exit_hooks.insert(key(runner, app_name), hook);
    }

    pub fn clear_exit_hook(&self, runner: Runner, app_name: &str) {
        self.state().exit_hooks.remove(&key(runner, app_name));
    }

    /// Start preparing a launch. Fails if the game is already running or
    /// being launched.
    pub fn prepare(self: &Arc<Self>, runner: Runner, app_name: &str) -> Result<LaunchTicket> {
        let k = key(runner, app_name);
        let mut st = self.state();
        if st.running.contains_key(&k) || st.pending.contains_key(&k) {
            return Err(Error::Busy(app_name.to_string()));
        }
        st.pending.insert(k, false);
        Ok(LaunchTicket {
            games: self.clone(),
            runner,
            app_name: app_name.to_string(),
        })
    }

    /// Spawn the game and return once it started. Exit is reported through
    /// the event sink. `install_dir` is the folder used to follow child
    /// processes (falls back to the working directory; see [`tracked_dir`]
    /// for when it is not used). Returns [`Error::Cancelled`] if the launch
    /// was cancelled while being prepared.
    pub async fn launch(
        self: &Arc<Self>,
        ticket: LaunchTicket,
        spec: LaunchSpec,
        install_dir: Option<PathBuf>,
    ) -> Result<()> {
        let runner = ticket.runner;
        let k = key(runner, &ticket.app_name);
        // the player's launch options: arguments, another exe, env now; the
        // wrapper after Proton, so it wraps the whole command
        let game_settings = crate::config::game::GameSettings::load(runner, &ticket.app_name);
        let mut spec = spec;
        game_settings.apply_launch(&mut spec, install_dir.as_deref());
        let (spec, output) = match self.compat.get() {
            Some(compat) => {
                let app = ticket.app_name.clone();
                let on_prepare = |p: Preparing| {
                    let mut status = GameStatus::new(runner, &app, GameStatusKind::Preparing);
                    status.message = p.message;
                    status.progress = p.progress;
                    self.events.emit(CoreEvent::GameStatus(status))
                };
                let w = compat
                    .wrap(runner, &ticket.app_name, spec, &on_prepare, &|| {
                        ticket.is_cancelled()
                    })
                    .await?;
                (w.spec, w.log)
            }
            None => (spec, None),
        };
        let spec = {
            let mut s = spec;
            game_settings.apply_wrapper(&mut s);
            s
        };
        // the command as it is really run (the store logged its own part
        // when it opened the file), secrets hidden
        crate::stores::GameLog::append(runner, &ticket.app_name, "launch").write(&format!(
            "launching: {} {} (cwd: {})",
            spec.executable,
            redact(&spec.args).join(" "),
            spec.working_dir
        ));
        let (mut child, dir) = {
            // Check for a cancel and register the process under one lock, so
            // a concurrent kill either stops the spawn or finds the process.
            let mut st = self.state();
            if st.pending.get(&k).copied().unwrap_or(false) {
                tracing::info!(app = %ticket.app_name, "launch cancelled before start");
                return Err(Error::Cancelled);
            }

            tracing::info!(exe = %spec.executable, cwd = %spec.working_dir, args = ?redact(&spec.args), "launching game");
            let mut cmd = Command::new(&spec.executable);
            crate::platform::child_env::apply(&mut cmd);
            cmd.args(&spec.args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if !spec.working_dir.is_empty() {
                cmd.current_dir(&spec.working_dir);
            }
            // Proton logs the whole command line and environment, Epic's
            // one-time login included: the output goes through
            // `redact_text` on its way to the log
            if output.is_some() {
                cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            }
            // runtime → Proton → Wine is a tree of processes: give it its own
            // group so a force-close reaches all of them (`Killer::kill`)
            #[cfg(unix)]
            cmd.process_group(0);
            for (k, v) in &spec.env {
                cmd.env(k, v);
            }
            let mut child = cmd
                .spawn()
                .map_err(|e| Error::Other(format!("failed to start {}: {e}", spec.executable)))?;
            if let Some(log) = output {
                log_filtered(child.stdout.take(), &log);
                log_filtered(child.stderr.take(), &log);
            }
            let dir = tracked_dir(runner, install_dir, &spec.working_dir);
            let pid = child.id().unwrap_or(0);
            st.running.insert(
                k.clone(),
                Running {
                    root: Some(Killer::from_pid(pid)),
                    dir: dir.clone(),
                },
            );
            st.pending.remove(&k);
            (child, dir)
        };
        let app = ticket.app_name.clone();
        drop(ticket);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.playtime.started(&k);
        self.events.emit(CoreEvent::GameStatus(GameStatus::new(
            runner,
            &app,
            GameStatusKind::Playing,
        )));

        let me = self.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let status = child.wait().await;
            tracing::debug!(app = %app, ?status, "launch process exited");
            // its PID may be reused any time now: never kill by it again
            if let Some(r) = me.state().running.get_mut(&k) {
                r.root = None;
            }
            // The launcher may have handed over to the real game: keep the
            // game "playing" while anything from its folder is alive.
            if let Some(dir) = dir {
                loop {
                    let d = dir.clone();
                    let alive = tokio::task::spawn_blocking(move || !processes_in(&d).is_empty())
                        .await
                        .unwrap_or(false);
                    if !alive {
                        break;
                    }
                    tokio::time::sleep(POLL).await;
                }
            }
            tracing::info!(app = %app, secs = started.elapsed().as_secs(), "game exited");
            if me.compat.get().is_some() {
                let a = app.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::compat::scrub_verbose_log(runner, &a)
                })
                .await;
            }
            me.playtime.add(&k, started.elapsed().as_secs());
            // the hook (cloud-save upload) runs while the game still counts
            // as running, so no sync or re-launch gets in between
            let hook = me.state().exit_hooks.remove(&k);
            if let Some(hook) = hook {
                hook().await;
            }
            let removed = me.state().running.remove(&k).is_some();
            if removed {
                me.count.fetch_sub(1, Ordering::Relaxed);
            }
            me.events.emit(CoreEvent::GameStatus(GameStatus::new(
                runner,
                &app,
                GameStatusKind::Done,
            )));
        });
        Ok(())
    }

    /// Force-close the game: the process tree we started plus every process
    /// running from the game's folder. While the launch is still being
    /// prepared this cancels it instead.
    pub async fn kill(&self, runner: Runner, app_name: &str) -> Result<()> {
        let k = key(runner, app_name);
        let entry = {
            let mut st = self.state();
            if let Some(cancelled) = st.pending.get_mut(&k) {
                tracing::info!(app = app_name, "cancelling launch");
                *cancelled = true;
                return Ok(());
            }
            st.running.get(&k).cloned()
        };
        let Some(entry) = entry else {
            return Err(Error::Other(format!("{app_name} is not running")));
        };
        tracing::info!(app = app_name, "force-closing game");
        if let Some(root) = entry.root {
            root.kill().await;
        }
        if let Some(dir) = entry.dir {
            let found = tokio::task::spawn_blocking(move || processes_in(&dir))
                .await
                .unwrap_or_default();
            for (pid, start) in found {
                Killer::with_start_time(pid.as_u32(), start).kill().await;
            }
        }
        Ok(())
    }

    /// Force-close every running game and cancel pending launches
    /// (controller shortcut).
    pub async fn kill_all(&self) {
        let keys: Vec<(Runner, String)> = {
            let st = self.state();
            st.running
                .keys()
                .chain(st.pending.keys())
                .filter_map(|k| {
                    let (r, app) = k.split_once(':')?;
                    let runner = Runner::all().into_iter().find(|x| x.id() == r)?;
                    Some((runner, app.to_string()))
                })
                .collect()
        };
        for (runner, app) in keys {
            if let Err(e) = self.kill(runner, &app).await {
                tracing::warn!(app, error = %e, "kill failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_auth_arguments() {
        let args = vec![
            "-AUTH_LOGIN=unused".to_string(),
            "-AUTH_PASSWORD=c173e58e76b24d".to_string(),
            "-epicovt=abc".to_string(),
            "-windowed".to_string(),
        ];
        let r = redact(&args);
        assert_eq!(r[1], "-AUTH_PASSWORD=<redacted>");
        assert_eq!(r[2], "-epicovt=<redacted>");
        assert_eq!(r[3], "-windowed");
        assert!(!r.join(" ").contains("c173e58e76b24d"));

        let args: Vec<String> = [
            "-epicusername=Player",
            "-epicuserid=abc123",
            "auth",
            "--code",
            "XYZ",
            "list",
        ]
        .map(String::from)
        .to_vec();
        let r = redact(&args);
        assert_eq!(
            r,
            [
                "-epicusername=<redacted>",
                "-epicuserid=<redacted>",
                "auth",
                "--code",
                "<redacted>",
                "list"
            ]
        );
    }

    #[test]
    fn redacts_secrets_in_logged_text() {
        let line = "umu: Command: ['Game.exe', '-AUTH_LOGIN=unused', '-AUTH_PASSWORD=c173e58e76b24d', '-epicusername=Player', '-epicuserid=abc123', '-windowed']";
        let r = redact_text(line);
        assert!(
            !r.contains("c173e58e76b24d") && !r.contains("Player") && !r.contains("abc123"),
            "{r}"
        );
        assert!(
            r.contains("-windowed") && r.contains("-AUTH_PASSWORD=<redacted>"),
            "{r}"
        );
        assert_eq!(
            redact_text("STORE=egs GAMEID=umu-123"),
            "STORE=egs GAMEID=umu-123"
        );
    }

    fn runner() -> Arc<GameRunner> {
        GameRunner::new(Arc::new(crate::events::NullSink))
    }

    fn spec() -> LaunchSpec {
        LaunchSpec {
            executable: "hyper-test-does-not-exist.exe".into(),
            args: vec![],
            working_dir: String::new(),
            env: Default::default(),
        }
    }

    #[tokio::test]
    async fn kill_while_preparing_cancels_the_launch() {
        let games = runner();
        let ticket = games.prepare(Runner::Legendary, "Game").unwrap();
        assert!(
            games.prepare(Runner::Legendary, "Game").is_err(),
            "second launch must be refused"
        );
        games.kill(Runner::Legendary, "Game").await.unwrap();
        assert!(ticket.is_cancelled());
        let r = games.launch(ticket, spec(), None).await;
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(games.running_keys().is_empty());
        // the reservation is released, so the game can be launched again
        assert!(games.prepare(Runner::Legendary, "Game").is_ok());
    }

    #[tokio::test]
    async fn dropped_ticket_releases_the_game() {
        let games = runner();
        drop(games.prepare(Runner::Legendary, "Game").unwrap());
        assert!(games.kill(Runner::Legendary, "Game").await.is_err());
        assert!(games.prepare(Runner::Legendary, "Game").is_ok());
    }

    #[test]
    fn never_tracks_a_drive_root() {
        assert_eq!(dir_key(Path::new("C:\\")), None);
        assert_eq!(
            dir_key(Path::new("E:\\Heroic\\Game")).as_deref(),
            Some("e:\\heroic\\game\\")
        );
    }

    #[test]
    fn system_and_user_folders_are_never_followed() {
        let tmp = std::env::temp_dir().join(format!("hyper-tracked-{}", std::process::id()));
        let game = tmp.join("Games").join("Limbo");
        let game_s = game.to_string_lossy().into_owned();
        // a store game's own folder is followed; the same folder for a
        // sideloaded game is not (it is just where the exe happens to be)
        assert_eq!(
            tracked_dir(Runner::Gog, Some(game.clone()), ""),
            dir_key(&game)
        );
        assert_eq!(
            tracked_dir(Runner::Legendary, None, &game_s),
            dir_key(&game)
        );
        assert_eq!(
            tracked_dir(Runner::Sideload, Some(game.clone()), &game_s),
            None
        );
        assert_eq!(tracked_dir(Runner::Gog, None, ""), None);
        assert_eq!(
            tracked_dir(Runner::Gog, Some(game.join("..")), ""),
            None,
            "no .."
        );
        if let Some(home) = dirs::home_dir() {
            assert_eq!(
                tracked_dir(Runner::Gog, Some(home.clone()), ""),
                None,
                "home"
            );
            assert_eq!(
                tracked_dir(Runner::Gog, Some(home.parent().unwrap().to_path_buf()), ""),
                None,
                "holds home"
            );
        }
        #[cfg(windows)]
        {
            let windir =
                PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into()));
            assert_eq!(
                tracked_dir(Runner::Gog, Some(windir.clone()), ""),
                None,
                "Windows"
            );
            assert_eq!(
                tracked_dir(Runner::Gog, Some(windir.join("System32")), ""),
                None,
                "inside Windows"
            );
            assert_eq!(
                tracked_dir(Runner::Gog, Some(PathBuf::from(r"C:\")), ""),
                None,
                "drive root"
            );
            if let Some(pf) = std::env::var_os("ProgramFiles") {
                assert_eq!(
                    tracked_dir(Runner::Gog, Some(PathBuf::from(pf)), ""),
                    None,
                    "Program Files"
                );
            }
        }
        #[cfg(unix)]
        {
            for d in ["/", "/usr", "/usr/bin", "/bin", "/opt", "/home", "/etc/x"] {
                assert_eq!(
                    tracked_dir(Runner::Gog, Some(PathBuf::from(d)), ""),
                    None,
                    "{d}"
                );
            }
            assert!(tracked_dir(Runner::Gog, Some(PathBuf::from("/opt/games/x")), "").is_some());
        }
    }
}
