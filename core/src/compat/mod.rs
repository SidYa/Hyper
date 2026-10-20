//! Windows games on Linux: they run with Proton inside the Steam Linux
//! Runtime, the way Steam and umu start them, with the game's protonfixes
//! (looked up in the umu database by store + game id). The umu launcher is
//! ported to Rust in [`umu`]; no helper binary and no host python3.
//!
//! Only used where `Platform::needs_wine()` (see `GameRunner::enable_compat`);
//! on Windows games start directly and nothing here runs.
//!
//! Always UMU-Proton (umu's default when `PROTONPATH` is unset): one Proton
//! that is fetched and kept up to date, no version picking. The runtime and
//! Proton are downloaded on first use (hundreds of MB, resumable) and every
//! game gets its own prefix; downloads and the prefix creation are reported
//! as the `preparing` status so the UI can show what is going on
//! ([`Preparing`]).

mod umu;

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

pub use umu::{Cancel, Preparing, Progress};

use crate::config::AppSettings;
use crate::error::{Error, Result};
use crate::paths;
use crate::platform;
use crate::process::Killer;
use crate::types::{LaunchSpec, Runner};

const UMU_API: &str = "https://umu.openwinecomponents.org/umu_api.php";
/// umu's id for games without an entry in its database.
const DEFAULT_GAME_ID: &str = "umu-default";

pub struct Compat {
    settings: Arc<RwLock<AppSettings>>,
    http: reqwest::Client,
    umu: umu::Umu,
}

/// A launch wrapped in Proton + runtime, plus the file its output goes to.
pub struct Wrapped {
    pub spec: LaunchSpec,
    pub log: Option<File>,
}

impl Compat {
    pub fn new(settings: Arc<RwLock<AppSettings>>) -> Self {
        // no overall timeout: the runtime and Proton archives take minutes
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            // https only, also after a redirect (GitHub / Valve's CDN redirect
            // to their storage hosts); reqwest's default limit of 10 hops
            .redirect(reqwest::redirect::Policy::custom(|a| {
                if a.previous().len() >= 10 {
                    a.error("too many redirects")
                } else if a.url().scheme() == "https" {
                    a.follow()
                } else {
                    a.stop()
                }
            }))
            .user_agent("Hyper")
            .build()
            .unwrap_or_default();
        Self {
            settings,
            umu: umu::Umu::new(http.clone()),
            http,
        }
    }

    /// Wrap a Windows program in Proton; anything else (a native Linux
    /// binary or script) starts as it is. Proton and the runtime are
    /// installed / updated first and the prefix is set up; a game whose
    /// prefix is new is created first (see [`Compat::prepare`]). `on_prepare`
    /// reports what is being waited for, `cancelled` stops it.
    pub async fn wrap(
        &self,
        runner: Runner,
        app_name: &str,
        spec: LaunchSpec,
        on_prepare: Progress<'_>,
        cancelled: Cancel<'_>,
    ) -> Result<Wrapped> {
        if !is_windows_program(&spec.executable) {
            return Ok(Wrapped { spec, log: None });
        }
        let settings = self.settings.read().await.clone();
        let prefix = prefix_dir(&settings, runner, app_name);
        std::fs::create_dir_all(&prefix)?;
        let store = umu_store(runner);
        let game_id = match store {
            Some(s) => self.game_id(runner, s, app_name).await,
            None => DEFAULT_GAME_ID.to_string(),
        };
        let store = store.unwrap_or("none");
        tracing::info!(app = app_name, prefix = %prefix.display(), game_id, "starting through Proton");

        // Proton + runtime (downloaded once, updated when a new build is out)
        let setup = self.umu.setup(on_prepare, cancelled).await?;
        if cancelled() {
            return Err(Error::Cancelled);
        }
        umu::setup_prefix_async(&prefix, on_prepare, cancelled).await?;

        let log = open_log(runner, app_name, "proton");
        let mut extra_env = Vec::new();
        // a crashed earlier run may have left its raw log behind
        scrub_verbose_log(runner, app_name);
        if settings.verbose_logs {
            // Proton writes this log itself, the command line with Epic's
            // account and one-time code included: into a private folder,
            // copied with secrets hidden once the game exits
            let raw = raw_proton_log_dir(runner, app_name);
            match create_private_dir(&raw) {
                Ok(()) => {
                    extra_env.push(("PROTON_LOG".to_string(), "1".to_string()));
                    extra_env.push((
                        "PROTON_LOG_DIR".to_string(),
                        raw.to_string_lossy().into_owned(),
                    ));
                }
                Err(e) => tracing::warn!(app = app_name, error = %e, "no verbose Proton log"),
            }
        }
        if !prefix_ready(&prefix) {
            on_prepare(Preparing::message("Creating the game's Wine prefix"));
            let create = self
                .launch(
                    &setup,
                    &prefix,
                    String::new(),
                    Vec::new(),
                    None,
                    &game_id,
                    store,
                    None,
                )
                .await?;
            let mut env = spec.env.clone();
            env.extend(extra_env.iter().cloned());
            env.extend(create.env);
            self.create_prefix(app_name, create.argv, &env, log.as_ref(), cancelled)
                .await?;
        }

        let cwd = (!spec.working_dir.is_empty()).then(|| PathBuf::from(&spec.working_dir));
        let launch = self
            .launch(
                &setup,
                &prefix,
                spec.executable,
                spec.args,
                cwd.as_deref(),
                &game_id,
                store,
                None,
            )
            .await?;
        let mut argv = launch.argv.into_iter();
        let executable = argv
            .next()
            .ok_or_else(|| Error::Other("empty launch command".into()))?;
        let mut env = spec.env;
        env.extend(extra_env);
        env.extend(launch.env);
        let spec = LaunchSpec {
            executable,
            args: argv.collect(),
            working_dir: spec.working_dir,
            env,
        };
        Ok(Wrapped { spec, log })
    }

    /// Run a Windows program inside a game's prefix through Proton and wait
    /// for it (GOG's setup scripts and redistributable installers). Proton,
    /// the runtime and the prefix are set up first when the game never ran;
    /// `on_prepare` reports that. Output → `logs/games/<store>-<app>-setup.log`.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_in_prefix(
        &self,
        runner: Runner,
        app_name: &str,
        exe: &Path,
        args: &[String],
        cwd: Option<&Path>,
        on_prepare: Progress<'_>,
        cancelled: Cancel<'_>,
    ) -> Result<std::process::ExitStatus> {
        let settings = self.settings.read().await.clone();
        let prefix = prefix_dir(&settings, runner, app_name);
        std::fs::create_dir_all(&prefix)?;
        let store = umu_store(runner);
        let game_id = match store {
            Some(s) => self.game_id(runner, s, app_name).await,
            None => DEFAULT_GAME_ID.to_string(),
        };
        let store = store.unwrap_or("none");
        let setup = self.umu.setup(on_prepare, cancelled).await?;
        umu::setup_prefix_async(&prefix, on_prepare, cancelled).await?;
        let log = open_log(runner, app_name, "setup");
        if !prefix_ready(&prefix) {
            on_prepare(Preparing::message("Creating the game's Wine prefix"));
            let create = self
                .launch(
                    &setup,
                    &prefix,
                    String::new(),
                    Vec::new(),
                    None,
                    &game_id,
                    store,
                    None,
                )
                .await?;
            self.create_prefix(app_name, create.argv, &create.env, log.as_ref(), cancelled)
                .await?;
        }
        let exe = exe.to_string_lossy().into_owned();
        tracing::info!(app = app_name, exe, "running in the prefix");
        let run = self
            .launch(
                &setup,
                &prefix,
                exe,
                args.to_vec(),
                cwd,
                &game_id,
                store,
                Some("run"),
            )
            .await?;
        self.run_and_wait(app_name, run.argv, &run.env, cwd, log.as_ref(), cancelled)
            .await
    }

    /// Command + environment of a launch (`ldconfig` and file checks: off
    /// the async threads).
    #[allow(clippy::too_many_arguments)]
    async fn launch(
        &self,
        setup: &umu::Setup,
        prefix: &Path,
        exe: String,
        args: Vec<String>,
        cwd: Option<&Path>,
        game_id: &str,
        store: &str,
        verb: Option<&'static str>,
    ) -> Result<umu::Launch> {
        let setup = setup.clone();
        let prefix = prefix.to_path_buf();
        let cwd = cwd.map(Path::to_path_buf);
        let (game_id, store) = (game_id.to_string(), store.to_string());
        tokio::task::spawn_blocking(move || {
            umu::launch(
                &setup,
                &prefix,
                &exe,
                &args,
                cwd.as_deref(),
                &game_id,
                &store,
                verb,
            )
        })
        .await
        .map_err(|e| Error::Other(format!("launch setup failed: {e}")))?
    }

    /// Proton with an empty program (`umu-run ""`): creates the prefix, the
    /// slow part of a game's first start. Done apart from the launch so the
    /// UI can say "Preparing" instead of a launch that seems to hang. A
    /// failure is only logged: the launch itself retries and reports.
    async fn create_prefix(
        &self,
        app_name: &str,
        argv: Vec<String>,
        env: &[(String, String)],
        log: Option<&File>,
        cancelled: Cancel<'_>,
    ) -> Result<()> {
        tracing::info!(app = app_name, "creating the Wine prefix");
        let started = std::time::Instant::now();
        match self
            .run_and_wait(app_name, argv, env, None, log, cancelled)
            .await
        {
            Ok(s) if s.success() => {
                tracing::info!(
                    app = app_name,
                    secs = started.elapsed().as_secs(),
                    "prefix created"
                )
            }
            Ok(s) => {
                tracing::warn!(app = app_name, code = ?s.code(), "prefix creation failed; launching anyway")
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                tracing::warn!(app = app_name, error = %e, "prefix creation failed; launching anyway")
            }
        }
        Ok(())
    }

    /// Start `argv` with `env` and wait for it, killing the whole process
    /// group on cancel; its output goes to `log` line by line with secrets
    /// hidden (the arguments can carry Epic's exchange code, `start
    /// link2ea://…`, and Wine / Proton echo them).
    async fn run_and_wait(
        &self,
        app_name: &str,
        argv: Vec<String>,
        env: &[(String, String)],
        cwd: Option<&Path>,
        log: Option<&File>,
        cancelled: Cancel<'_>,
    ) -> Result<std::process::ExitStatus> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| Error::Other("empty command".into()))?;
        let mut cmd = tokio::process::Command::new(program);
        crate::platform::child_env::apply(&mut cmd);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        if log.is_some() {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        // runtime → Proton → wineserver: one group, so a cancel reaches them all
        #[cfg(unix)]
        cmd.process_group(0);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Other(format!("failed to start {program}: {e}")))?;
        if let Some(log) = log {
            crate::games::log_filtered(child.stdout.take(), log);
            crate::games::log_filtered(child.stderr.take(), log);
        }
        let killer = Killer::from_pid(child.id().unwrap_or(0));
        loop {
            tokio::select! {
                status = child.wait() => {
                    return status.map_err(|e| Error::Other(format!("waiting for {program}: {e}")));
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => {
                    if cancelled() {
                        tracing::info!(app = app_name, program, "cancelled");
                        killer.kill().await;
                        let _ = child.wait().await;
                        return Err(Error::Cancelled);
                    }
                }
            }
        }
    }

    /// umu id of a store game. A found id is cached on disk for good; "not
    /// in the database" is cached too and asked again after
    /// [`NEGATIVE_TTL`]. Offline (the store's API not reachable) nothing is
    /// asked. File work runs on the blocking pool.
    async fn game_id(&self, runner: Runner, store: &str, codename: &str) -> String {
        let key = format!("{store}:{codename}");
        let file = paths::cache_dir().join("umu-ids.json");
        let read = file.clone();
        let mut cache: HashMap<String, CachedId> = tokio::task::spawn_blocking(move || {
            std::fs::read(&read)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        let now = unix_now();
        match cache.get(&key) {
            Some(CachedId::Found(id)) => return id.clone(),
            Some(CachedId::Missing { id, checked_at })
                if now.saturating_sub(*checked_at) < NEGATIVE_TTL.as_secs() =>
            {
                return id.clone();
            }
            _ => {}
        }
        if !crate::net::reachable(runner).await {
            return DEFAULT_GAME_ID.to_string();
        }
        let (id, entry) = match self.lookup(store, codename).await {
            Ok(Some(id)) => (id.clone(), CachedId::Found(id)),
            Ok(None) => (
                DEFAULT_GAME_ID.to_string(),
                CachedId::Missing {
                    id: DEFAULT_GAME_ID.to_string(),
                    checked_at: now,
                },
            ),
            Err(e) => {
                // a network error is not an answer: ask again next time
                tracing::warn!(store, codename, error = %e, "umu database lookup failed");
                return DEFAULT_GAME_ID.to_string();
            }
        };
        cache.insert(key, entry);
        if let Ok(bytes) = serde_json::to_vec_pretty(&cache) {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = std::fs::create_dir_all(paths::cache_dir());
                let _ = std::fs::write(&file, bytes);
            })
            .await;
        }
        id
    }

    async fn lookup(&self, store: &str, codename: &str) -> reqwest::Result<Option<String>> {
        #[derive(Deserialize)]
        struct Entry {
            umu_id: Option<String>,
        }
        let Ok(url) =
            reqwest::Url::parse_with_params(UMU_API, &[("store", store), ("codename", codename)])
        else {
            return Ok(None);
        };
        let list: Vec<Entry> = self
            .http
            .get(url)
            .timeout(Duration::from_secs(5))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(list.into_iter().find_map(|e| e.umu_id))
    }
}

/// How long "not in the umu database" is believed before asking again.
const NEGATIVE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// An entry of `cache/umu-ids.json`: a found id is a plain string (the
/// file's old layout), a miss `{"id": "umu-default", "checkedAt": <unix s>}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum CachedId {
    Found(String),
    #[serde(rename_all = "camelCase")]
    Missing {
        id: String,
        checked_at: u64,
    },
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// umu's name for the store (`STORE`), used to find the game's fixes.
fn umu_store(runner: Runner) -> Option<&'static str> {
    match runner {
        Runner::Legendary => Some("egs"),
        Runner::Gog => Some("gog"),
        Runner::Sideload => None,
    }
}

fn is_windows_program(exe: &str) -> bool {
    Path::new(exe)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["exe", "bat", "cmd", "msi"].contains(&e.to_ascii_lowercase().as_str()))
}

/// Proton wrote the prefix's registry, so its setup is done. umu points
/// `pfx` back at the prefix itself; older Proton layouts keep it below.
fn prefix_ready(prefix: &Path) -> bool {
    prefix.join("system.reg").is_file() || prefix.join("pfx").join("system.reg").is_file()
}

/// Folder holding the Wine prefixes (one per game); by default next to the
/// games (`<Games Lib>/Prefixes`), so moving the games to an SD card takes
/// the prefixes along.
pub fn prefixes_dir(settings: &AppSettings) -> PathBuf {
    let custom = settings.wine_prefixes_dir.trim();
    if !custom.is_empty() {
        return PathBuf::from(custom);
    }
    let games = settings.default_install_path.trim();
    let games = if games.is_empty() {
        platform::current().default_install_path()
    } else {
        PathBuf::from(games)
    };
    games.join("Prefixes")
}

/// The game's own prefix: `<prefixes>/<store>/<game id>`.
pub fn prefix_dir(settings: &AppSettings, runner: Runner, app_name: &str) -> PathBuf {
    prefixes_dir(settings)
        .join(runner.id())
        .join(safe_name(app_name))
}

pub(crate) fn safe_name(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_. ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.trim_matches(['.', ' ']).is_empty() {
        "_".into()
    } else {
        out
    }
}

/// Proton / runtime output of the last launch (`proton`) or setup run
/// (`setup`): `logs/games/<store>-<game>-<kind>.log`.
fn open_log(runner: Runner, app_name: &str, kind: &str) -> Option<File> {
    let dir = paths::game_logs_dir();
    std::fs::create_dir_all(&dir).ok()?;
    File::create(dir.join(format!(
        "{}-{}-{kind}.log",
        runner.id(),
        safe_name(app_name)
    )))
    .ok()
}

/// Where Proton writes its verbose log (`PROTON_LOG`) during a launch:
/// `logs/games/.proton-raw/<store>-<game>`, private to the user.
fn raw_proton_log_dir(runner: Runner, app_name: &str) -> PathBuf {
    paths::game_logs_dir()
        .join(".proton-raw")
        .join(format!("{}-{}", runner.id(), safe_name(app_name)))
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// After a launch with verbose logs: Proton's own log (raw, see
/// [`raw_proton_log_dir`]) is copied line by line through
/// [`crate::games::redact_text`] into
/// `logs/games/<store>-<game>-proton-verbose.log`, then the raw folder is
/// deleted. Nothing to do when the folder does not exist. Blocking.
pub fn scrub_verbose_log(runner: Runner, app_name: &str) {
    use std::io::{BufRead, BufReader, BufWriter, Write};

    let raw = raw_proton_log_dir(runner, app_name);
    let Ok(entries) = std::fs::read_dir(&raw) else {
        return;
    };
    let target = paths::game_logs_dir().join(format!(
        "{}-{}-proton-verbose.log",
        runner.id(),
        safe_name(app_name)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut out = BufWriter::new(File::create(&target)?);
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for file in files {
            let mut reader = BufReader::new(File::open(&file)?);
            let mut line = Vec::new();
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches(['\r', '\n']);
                writeln!(out, "{}", crate::games::redact_text(text))?;
            }
        }
        out.flush()
    })();
    if let Err(e) = result {
        tracing::warn!(app = app_name, error = %e, "verbose Proton log not copied");
        let _ = std::fs::remove_file(&target);
    }
    if let Err(e) = std::fs::remove_dir_all(&raw) {
        tracing::warn!(app = app_name, error = %e, "raw Proton log not removed");
    }
}

/// Delete a game's prefix after it was uninstalled (Linux only; nothing to
/// do where games start natively). The prefix holds the game's local saves
/// too; the uninstall dialog says so. Never fails the uninstall.
///
/// Only a folder that looks like a Wine prefix (umu's `pfx` link,
/// `tracked_files` or `drive_c`) and lies inside the prefixes folder (both
/// canonicalised) is deleted. `remove_dir_all` does not follow symlinks, so
/// the prefix's `dosdevices/z:` → `/` link is removed, not walked.
pub async fn remove_prefix(settings: &RwLock<AppSettings>, runner: Runner, app_name: &str) {
    if !platform::current().needs_wine() {
        return;
    }
    let (root, dir) = {
        let settings = settings.read().await;
        (
            prefixes_dir(&settings),
            prefix_dir(&settings, runner, app_name),
        )
    };
    let checked = tokio::task::spawn_blocking(move || removable_prefix(&root, &dir)).await;
    let dir = match checked {
        Ok(Ok(Some(dir))) => dir,
        Ok(Ok(None)) => return,
        Ok(Err(why)) => {
            tracing::warn!(app = app_name, why, "prefix left in place");
            return;
        }
        Err(e) => {
            tracing::warn!(app = app_name, error = %e, "prefix check failed");
            return;
        }
    };
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => tracing::info!(dir = %dir.display(), "prefix removed"),
        Err(e) => tracing::warn!(dir = %dir.display(), error = %e, "could not remove the prefix"),
    }
}

/// The canonical prefix folder to delete; `Ok(None)` when there is none,
/// `Err` when the folder is not one to delete. Blocking.
fn removable_prefix(root: &Path, dir: &Path) -> std::result::Result<Option<PathBuf>, String> {
    // the prefix itself must be a real folder, not a link to somewhere else
    match std::fs::symlink_metadata(dir) {
        Err(_) => return Ok(None),
        Ok(m) if !m.is_dir() => return Err(format!("{} is not a folder", dir.display())),
        Ok(_) => {}
    }
    let looks_like_prefix = std::fs::symlink_metadata(dir.join("pfx")).is_ok()
        || dir.join("tracked_files").is_file()
        || dir.join("drive_c").is_dir();
    if !looks_like_prefix {
        return Err(format!(
            "{} does not look like a Wine prefix",
            dir.display()
        ));
    }
    let root = std::fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let real = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if real == root || !real.starts_with(&root) {
        return Err(format!(
            "{} is outside the prefixes folder {}",
            real.display(),
            root.display()
        ));
    }
    Ok(Some(real))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_windows_programs_are_wrapped() {
        assert!(is_windows_program("/games/x/Game.EXE"));
        assert!(is_windows_program("setup.msi"));
        assert!(!is_windows_program("/games/x/start.sh"));
        assert!(!is_windows_program("/games/x/game"));
    }

    #[test]
    fn umu_id_cache_reads_old_and_new_entries() {
        let json = r#"{"egs:a":"umu-123","gog:b":{"id":"umu-default","checkedAt":42}}"#;
        let cache: HashMap<String, CachedId> = serde_json::from_str(json).expect("cache");
        assert_eq!(cache["egs:a"], CachedId::Found("umu-123".into()));
        assert_eq!(
            cache["gog:b"],
            CachedId::Missing {
                id: "umu-default".into(),
                checked_at: 42
            }
        );
        let back = serde_json::to_string(&cache["gog:b"]).expect("json");
        assert!(back.contains("\"checkedAt\":42"));
    }

    #[test]
    fn only_prefixes_inside_the_folder_are_removed() {
        let base = std::env::temp_dir().join(format!("hyper-rmpfx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("Prefixes");
        let game = root.join("legendary").join("Game");
        std::fs::create_dir_all(game.join("drive_c")).expect("prefix");
        let plain = root.join("legendary").join("NotAPrefix");
        std::fs::create_dir_all(&plain).expect("plain");
        std::fs::write(plain.join("save.dat"), "x").expect("file");

        let real = removable_prefix(&root, &game).expect("ok").expect("some");
        assert_eq!(real, std::fs::canonicalize(&game).expect("canon"));
        assert!(removable_prefix(&root, &plain).is_err(), "no prefix marker");
        assert_eq!(
            removable_prefix(&root, &root.join("legendary").join("Gone")),
            Ok(None)
        );
        // a prefix-looking folder outside the prefixes folder stays
        let outside = base.join("elsewhere");
        std::fs::create_dir_all(outside.join("drive_c")).expect("outside");
        assert!(removable_prefix(&root, &outside).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prefix_names_stay_inside_the_folder() {
        assert_eq!(safe_name("Catnip"), "Catnip");
        assert_eq!(safe_name("../etc"), ".._etc");
        assert_eq!(safe_name(".."), "_");
        assert_eq!(safe_name("a/b\\c"), "a_b_c");
    }
}
