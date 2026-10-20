//! umu-run in Rust: a port of `Open-Wine-Components/umu-launcher` (GPL-3,
//! listed as an origin in the licences) for the one way Hyper used it,
//! `umu-run <game.exe> [args]` with `WINEPREFIX`, `GAMEID` and `STORE` set
//! and no `PROTONPATH` (always UMU-Proton).
//!
//! What a launch needs, in umu's order (`umu_run.py::umu_run`):
//! 1. UMU-Proton, installed or updated ([`proton`]);
//! 2. the Steam Linux Runtime that Proton's `toolmanifest.vdf` asks for,
//!    installed or updated ([`runtime`]), plus umu's `umu-shim`;
//! 3. the prefix folder laid out the way Proton expects ([`setup_prefix_async`]);
//! 4. the Steam compatibility-tool environment ([`launch`]) and the command
//!    `<runtime>/umu --verb=waitforexitandrun -- <shim> <proton>/proton
//!    waitforexitandrun <exe> <args>`, run by the game runner.
//!
//! Same folders as umu (`~/.local/share/umu`, `~/.cache/umu`,
//! `~/.local/share/Steam/compatibilitytools.d`) and the same `WINEPREFIX`
//! layout, so everything umu set up before keeps working and umu / Steam /
//! Heroic can share the runtime and Proton. umu's file locks are taken too.
//!
//! Not ported: TOML launch configs, winetricks verbs, `PROTONPATH` choices
//! (GE-Proton, `*-Latest` delta patches, passthrough runtimes for native
//! games), `UMU_NO_RUNTIME`, Flatpak + gamescope window tagging, re-entering
//! a running container (`UMU_CONTAINER_NSENTER`), zenity progress windows
//! (the UI shows progress itself).

pub mod fetch;
pub mod fsutil;
pub mod proton;
pub mod runtime;
pub mod vdf;

use std::io;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};

use crate::error::{Error, Result};
use crate::types::DownloadProgress;

/// What the UI shows while a launch is being prepared.
#[derive(Debug, Clone, Default)]
pub struct Preparing {
    /// English, dynamic ("Downloading UMU-Proton-10.0-4").
    pub message: Option<String>,
    pub progress: Option<DownloadProgress>,
}

impl Preparing {
    /// A step without a measurable progress. The empty progress is sent on
    /// purpose: the UI keeps the previous one for a status-only update.
    pub fn message(text: impl Into<String>) -> Self {
        Self {
            message: Some(text.into()),
            progress: Some(DownloadProgress::default()),
        }
    }
}

pub type Progress<'a> = &'a (dyn Fn(Preparing) + Send + Sync);
pub type Cancel<'a> = &'a (dyn Fn() -> bool + Send + Sync);

/// umu's folders (`umu_consts.py`).
#[derive(Debug, Clone)]
pub struct Dirs {
    /// `$XDG_DATA_HOME/umu`: runtimes, the shim, lock files.
    pub local: PathBuf,
    /// `$XDG_CACHE_HOME/umu`: partial downloads.
    pub cache: PathBuf,
    /// Steam's `compatibilitytools.d`: where UMU-Proton is installed.
    pub steam_compat: PathBuf,
    /// `<local>/compatibilitytools`: umu's own tools (`UMU-Latest` etc.).
    pub umu_compat: PathBuf,
}

impl Dirs {
    pub fn detect() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let env_dir = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let data = if std::env::var("container").as_deref() == Ok("flatpak") {
            env_dir("HOST_XDG_DATA_HOME").or_else(|| env_dir("XDG_DATA_HOME"))
        } else {
            env_dir("XDG_DATA_HOME")
        }
        .unwrap_or_else(|| home.join(".local").join("share"));
        let cache = env_dir("XDG_CACHE_HOME")
            .unwrap_or_else(|| home.join(".cache"))
            .join("umu");
        match env_dir("UMU_FOLDERS_PATH") {
            Some(folders) => Self {
                local: folders.join("umu"),
                cache,
                steam_compat: folders.join("Steam").join("compatibilitytools.d"),
                umu_compat: folders.join("umu").join("compatibilitytools"),
            },
            None => Self {
                local: data.join("umu"),
                cache,
                steam_compat: data.join("Steam").join("compatibilitytools.d"),
                umu_compat: data.join("umu").join("compatibilitytools"),
            },
        }
    }
}

/// Everything a launch needs besides the game: Proton, its runtime, the shim.
#[derive(Debug, Clone)]
pub struct Setup {
    pub proton: PathBuf,
    /// `None` for a tool that runs on the host (no `require_tool_appid`).
    pub runtime: Option<PathBuf>,
    pub shim: PathBuf,
}

/// Command line and environment of a launch through the runtime.
#[derive(Debug, Clone)]
pub struct Launch {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub struct Umu {
    http: reqwest::Client,
    dirs: Dirs,
}

const PROTON_VERBS: &[&str] = &[
    "waitforexitandrun",
    "run",
    "runinprefix",
    "destroyprefix",
    "getcompatpath",
    "getnativepath",
];
const DEFAULT_VERB: &str = "waitforexitandrun";

/// umu's shim: inside a gamescope session with several Xwaylands the game
/// goes to display `:1`.
const SHIM: &str = r#"#!/bin/sh

if [ "${XDG_CURRENT_DESKTOP}" = "gamescope" ] || [ "${XDG_SESSION_DESKTOP}" = "gamescope" ]; then
    # Check if STEAM_MULTIPLE_XWAYLANDS is set to 1
    if [ "${STEAM_MULTIPLE_XWAYLANDS}" = "1" ]; then
        # Check if DISPLAY is set, if not, set it to ":1"
        if [ -z "${DISPLAY}" ]; then
            export DISPLAY=":1"
        fi
    fi
fi

# Execute the passed command
exec "$@"
"#;

impl Umu {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            dirs: Dirs::detect(),
        }
    }

    /// Install or update UMU-Proton and its runtime (network permitting),
    /// write the shim. `progress` is only called when there is something
    /// to wait for.
    pub async fn setup(&self, progress: Progress<'_>, cancelled: Cancel<'_>) -> Result<Setup> {
        tokio::fs::create_dir_all(&self.dirs.local).await?;
        let proton = proton::ensure(&self.http, &self.dirs, progress, cancelled).await?;
        let runtime = match runtime::required_by(&proton)? {
            Some(rt) => {
                Some(runtime::ensure(&self.http, &self.dirs, rt, progress, cancelled).await?)
            }
            None => None,
        };
        let shim = self.dirs.local.join("umu-shim");
        if !shim.is_file() {
            write_shim(&shim)?;
        }
        Ok(Setup {
            proton,
            runtime,
            shim,
        })
    }
}

fn write_shim(path: &Path) -> io::Result<()> {
    std::fs::write(path, SHIM)?;
    fsutil::set_mode(path, 0o700)
}

/// What the UI shows while another launcher (umu, Steam, Heroic) holds one
/// of umu's locks.
pub const WAITING_FOR_LOCK: &str = "Waiting for another launcher to finish";

/// Take one of umu's lock files, polling it (`LOCK_EX | LOCK_NB`) every
/// [`fsutil::LOCK_RETRY`] so a cancel is honoured while another launcher
/// holds it; the wait is reported once through `progress`.
pub async fn lock(
    path: PathBuf,
    progress: Progress<'_>,
    cancelled: Cancel<'_>,
) -> Result<fsutil::FileLock> {
    let mut file = tokio::task::spawn_blocking(move || fsutil::open_lock_file(&path))
        .await
        .map_err(|e| Error::Other(format!("lock: {e}")))??;
    let mut reported = false;
    loop {
        // non-blocking: fine on the async thread
        file = match fsutil::try_lock(file)? {
            Ok(lock) => return Ok(lock),
            Err(file) => file,
        };
        if cancelled() {
            return Err(Error::Cancelled);
        }
        if !reported {
            reported = true;
            tracing::info!("another launcher holds the lock; waiting");
            progress(Preparing::message(WAITING_FOR_LOCK));
        }
        tokio::time::sleep(fsutil::LOCK_RETRY).await;
    }
}

/// Lay the prefix folder out the way Proton (and umu) expect
/// (`umu_run.py::setup_pfx`), under umu's `pfx.lock`: `pfx` points back at
/// the folder itself, `shadercache` / `gstreamer-1.0` exist, and the Unix
/// user's `drive_c/users/<name>` is the same folder as `steamuser`.
/// Blocking; `cancelled` ends the wait for a `pfx.lock` another launcher
/// holds (`Error::Cancelled`).
#[cfg(all(test, unix))]
pub fn setup_prefix(prefix: &Path, cancelled: &dyn Fn() -> bool) -> Result<()> {
    std::fs::create_dir_all(prefix)?;
    let path = std::fs::canonicalize(prefix)?;
    let waiting =
        || tracing::info!(prefix = %path.display(), "another launcher holds pfx.lock; waiting");
    let _lock =
        fsutil::lock(&path.join("pfx.lock"), cancelled, &waiting)?.ok_or(Error::Cancelled)?;
    layout_prefix(&path)
}

/// Prefix setup for async callers (`umu_run.py::setup_pfx`, see
/// `layout_prefix`): `pfx.lock` is polled on the async
/// side (a wait is shown through `progress`, a cancel ends it), the file
/// work runs on the blocking pool.
pub async fn setup_prefix_async(
    prefix: &Path,
    progress: Progress<'_>,
    cancelled: Cancel<'_>,
) -> Result<()> {
    let prefix = prefix.to_path_buf();
    let path = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        std::fs::create_dir_all(&prefix)?;
        Ok(std::fs::canonicalize(&prefix)?)
    })
    .await
    .map_err(|e| Error::Other(format!("prefix setup failed: {e}")))??;
    let lock = lock(path.join("pfx.lock"), progress, cancelled).await?;
    tokio::task::spawn_blocking(move || {
        let _lock = lock;
        layout_prefix(&path)
    })
    .await
    .map_err(|e| Error::Other(format!("prefix setup failed: {e}")))?
}

/// The prefix layout itself; `path` is canonical and `pfx.lock` is held.
fn layout_prefix(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path.join("shadercache"))?;
    std::fs::create_dir_all(path.join("gstreamer-1.0"))?;

    let pfx = path.join("pfx");
    if let Ok(meta) = std::fs::symlink_metadata(&pfx)
        && meta.file_type().is_symlink()
    {
        std::fs::remove_file(&pfx)?;
    }
    if std::fs::symlink_metadata(&pfx).is_err() {
        fsutil::symlink(".", &pfx)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.join("tracked_files"))?;

    let users = path.join("drive_c").join("users");
    let steamuser = users.join("steamuser");
    let unixuser = fsutil::unix_user_name();
    let wineuser = users.join(&unixuser);
    let present = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    if unixuser == "steamuser" {
        std::fs::create_dir_all(&steamuser)?;
    } else if !present(&wineuser) && !present(&steamuser) {
        // a new prefix: user -> steamuser
        std::fs::create_dir_all(&steamuser)?;
        fsutil::symlink("steamuser", &wineuser)?;
    } else if wineuser.is_dir() && !present(&steamuser) {
        // a prefix made by plain Wine: steamuser -> user
        fsutil::symlink(&unixuser, &steamuser)?;
    } else if !present(&wineuser) && steamuser.is_dir() {
        fsutil::symlink("steamuser", &wineuser)?;
    }
    Ok(())
}

/// Build the command and environment of a launch (`umu_run.py::set_env` +
/// `build_command`). An empty `exe` creates the prefix only (`umu-run ""`).
/// A relative `exe` with a folder in it is resolved against `cwd` (the
/// launch's working folder; umu uses its own); a bare name (`start`, `reg`)
/// or a Windows path is passed on as it is, it only exists inside the prefix.
/// `verb`: the Proton verb (`run` for a helper program in the prefix);
/// `None` = `PROTON_VERB` of the host, else `waitforexitandrun`.
/// Blocking: it runs `ldconfig` and touches the file system.
#[allow(clippy::too_many_arguments)]
pub fn launch(
    setup: &Setup,
    prefix: &Path,
    exe: &str,
    args: &[String],
    cwd: Option<&Path>,
    game_id: &str,
    store: &str,
    verb: Option<&str>,
) -> Result<Launch> {
    let host = |name: &str| std::env::var(name).unwrap_or_default();
    let pfx = std::fs::canonicalize(prefix).unwrap_or_else(|_| prefix.to_path_buf());
    let protonpath = std::fs::canonicalize(&setup.proton)?;
    let pfx_s = pfx.to_string_lossy().into_owned();
    let proton_s = protonpath.to_string_lossy().into_owned();
    let runtime_path = setup
        .runtime
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut env: Vec<(String, String)> = Vec::new();
    let mut set = |k: &str, v: String| env.push((k.to_string(), v));

    let host_verb = verb
        .map(str::to_string)
        .unwrap_or_else(|| host("PROTON_VERB"));
    let verb = if PROTON_VERBS.contains(&host_verb.as_str()) {
        host_verb
    } else {
        DEFAULT_VERB.to_string()
    };

    // EXE + STEAM_COMPAT_INSTALL_PATH
    let mut install_path = host("STEAM_COMPAT_INSTALL_PATH");
    let exe_env = if exe.is_empty() {
        install_path.clear();
        String::new()
    } else if !is_host_path(exe) {
        // a bare program name (`start`, `reg`) or a Windows path: Wine
        // resolves it inside the prefix, there is nothing to find here
        exe.to_string()
    } else {
        let path = Path::new(exe);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok())
                .map(|d| d.join(path))
                .unwrap_or_else(|| path.to_path_buf())
        };
        if abs.exists() {
            if install_path.is_empty() {
                install_path = abs
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
            abs.to_string_lossy().into_owned()
        } else {
            // may exist only inside the prefix or the container
            tracing::warn!(exe, "executable not found");
            exe.to_string()
        }
    };

    // Steam app id: the number of a `umu-<id>`, else 0
    let steam_app_id = match game_id.strip_prefix("umu-") {
        Some(rest)
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
        {
            rest.to_string()
        }
        _ => "0".to_string(),
    };

    set("WINEPREFIX", pfx_s.clone());
    set("GAMEID", game_id.to_string());
    set("STORE", store.to_string());
    set(
        "PROTON_CRASH_REPORT_DIR",
        std::env::temp_dir()
            .join("umu_crashreports")
            .to_string_lossy()
            .into_owned(),
    );
    set("PROTONPATH", proton_s.clone());
    set("PROTON_VERB", verb);
    set("EXE", exe_env);
    set("STEAM_COMPAT_INSTALL_PATH", install_path.clone());
    set("STEAM_COMPAT_CLIENT_INSTALL_PATH", String::new());
    set("STEAM_COMPAT_DATA_PATH", pfx_s.clone());
    set(
        "STEAM_COMPAT_SHADER_PATH",
        pfx.join("shadercache").to_string_lossy().into_owned(),
    );
    set("STEAM_COMPAT_LAUNCHER_SERVICE", String::new());
    set("FONTCONFIG_PATH", String::new());
    set("UMU_ID", game_id.to_string());
    set("UMU_STEAM_GAME_ID", host("SteamGameId"));
    set("SteamAppId", steam_app_id.clone());
    set("SteamGameId", steam_app_id);
    set("UMU_INVOCATION_ID", random_hex());
    let tool_paths = if runtime_path.is_empty() {
        proton_s.clone()
    } else {
        format!("{proton_s}:{runtime_path}")
    };
    set("STEAM_COMPAT_TOOL_PATHS", tool_paths.clone());
    set("STEAM_COMPAT_MOUNTS", tool_paths);
    // the launcher service keys the container by this
    set(
        "STEAM_COMPAT_APP_ID",
        fsutil::hex(&Md5::digest(pfx_s.as_bytes())),
    );
    // parity only: neither re-entering a container nor zenity is implemented
    set("UMU_CONTAINER_NSENTER", host("UMU_CONTAINER_NSENTER"));
    set("UMU_ZENITY", host("UMU_ZENITY"));

    // Steam game drive: the mount the game lives on, and the host's libraries
    let mut library_paths = host("STEAM_COMPAT_LIBRARY_PATHS");
    if !install_path.is_empty()
        && let Some(mount) = Path::new(&install_path)
            .ancestors()
            .skip(1)
            .find(|p| *p != Path::new("/") && fsutil::is_mount_point(p))
    {
        let mount = mount.to_string_lossy();
        library_paths = if library_paths.is_empty() {
            mount.into_owned()
        } else {
            format!("{library_paths}:{mount}")
        };
    }
    set("STEAM_COMPAT_LIBRARY_PATHS", library_paths);
    let mut paths: std::collections::BTreeSet<String> = host("LD_LIBRARY_PATH")
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if !install_path.is_empty() {
        paths.insert(install_path.clone());
    }
    paths.extend(fsutil::library_paths());
    set(
        "STEAM_RUNTIME_LIBRARY_PATH",
        paths.into_iter().collect::<Vec<_>>().join(":"),
    );

    // Forwarded from the host for parity with umu's environment only (so
    // protonfixes and Proton see what they would under umu); Hyper does
    // not implement what they switch (see the module notes: `UMU_NO_RUNTIME`,
    // `UMU_NO_PROTON`, `UMU_RUNTIME_UPDATE`, like `UMU_CONTAINER_NSENTER` and
    // `UMU_ZENITY` above, change nothing here).
    set("UMU_NO_RUNTIME", host("UMU_NO_RUNTIME"));
    set("UMU_RUNTIME_UPDATE", host("UMU_RUNTIME_UPDATE"));
    set("UMU_NO_PROTON", host("UMU_NO_PROTON"));
    set("RUNTIMEPATH", runtime_path);

    let verb = env
        .iter()
        .find(|(k, _)| k == "PROTON_VERB")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| DEFAULT_VERB.to_string());
    let exe_env = env
        .iter()
        .find(|(k, _)| k == "EXE")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let mut argv = command(setup, &protonpath, &verb)?;
    argv.push(exe_env);
    argv.extend(args.iter().cloned());
    Ok(Launch { argv, env })
}

/// Does `exe` name a file on the host (an absolute path or a relative one
/// with a folder in it)? A bare name or a Windows path (`C:\…`, `a\b.exe`)
/// is left to Wine.
fn is_host_path(exe: &str) -> bool {
    let path = Path::new(exe);
    if path.is_absolute() {
        return true;
    }
    exe.contains('/') && !exe.contains('\\') && !is_drive_path(exe)
}

/// `C:\…` / `C:/…`
fn is_drive_path(exe: &str) -> bool {
    let b = exe.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// `umu_runtime.py::CompatLayer.command` for Proton inside the runtime:
/// `<runtime>/umu --verb=<verb> -- <shim> <proton>/proton <verb>`.
fn command(setup: &Setup, protonpath: &Path, verb: &str) -> Result<Vec<String>> {
    let proton_manifest =
        vdf::load_manifest(&protonpath.join("toolmanifest.vdf")).map_err(Error::Other)?;
    if proton_manifest.str("compatmanager_layer_name") != Some("proton") {
        return Err(Error::Other(format!(
            "{} is not a Proton (compatmanager_layer_name)",
            protonpath.display()
        )));
    }
    let proton_cmd = tool_command(
        protonpath,
        proton_manifest
            .str("commandline")
            .unwrap_or("/proton %verb%"),
        verb,
    );
    let shim = setup.shim.to_string_lossy().into_owned();
    let mut argv = Vec::new();
    match &setup.runtime {
        Some(runtime) => {
            let commandline = vdf::load_manifest(&runtime.join("toolmanifest.vdf"))
                .ok()
                .and_then(|m| m.str("commandline").map(str::to_string))
                .unwrap_or_else(|| "/_v2-entry-point --verb=%verb% --".to_string());
            let mut rt_cmd = tool_command(runtime, &commandline, verb);
            // umu's own entry point, when the runtime was installed by it or us
            if runtime.join("umu").is_file()
                && let Some(first) = rt_cmd.first_mut()
            {
                *first = first.replace("_v2-entry-point", "umu");
            }
            argv.extend(rt_cmd);
            argv.push(shim);
            argv.extend(proton_cmd);
        }
        None => {
            argv.push(shim);
            argv.extend(proton_cmd);
        }
    }
    Ok(argv)
}

/// `commandline` of a tool manifest, rooted at the tool's folder, `%verb%`
/// filled in: `/proton %verb%` → `[<tool>/proton, waitforexitandrun]`.
fn tool_command(tool: &Path, commandline: &str, verb: &str) -> Vec<String> {
    let mut words = commandline.split_whitespace();
    let first = words.next().unwrap_or_default();
    let mut out = vec![format!(
        "{}{}",
        tool.to_string_lossy(),
        first.replace("%verb%", verb)
    )];
    out.extend(words.map(|w| w.replace("%verb%", verb)));
    out
}

fn random_hex() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes[..16].copy_from_slice(&nanos.to_le_bytes());
    }
    fsutil::hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_tools(base: &Path) -> Setup {
        let proton = base.join("UMU-Proton-10.0-4");
        let runtime = base.join("steamrt3");
        std::fs::create_dir_all(&proton).expect("proton");
        std::fs::create_dir_all(&runtime).expect("runtime");
        std::fs::write(
            proton.join("toolmanifest.vdf"),
            "\"manifest\"\n{\n  \"commandline\" \"/proton %verb%\"\n  \"require_tool_appid\" \"1628350\"\n  \"compatmanager_layer_name\" \"proton\"\n}\n",
        )
        .expect("manifest");
        std::fs::write(
            runtime.join("toolmanifest.vdf"),
            "// Generated file, do not edit\n\"manifest\"\n{\n  \"commandline\" \"/_v2-entry-point --verb=%verb% --\"\n  \"compatmanager_layer_name\" \"container-runtime\"\n}\n",
        )
        .expect("manifest");
        std::fs::write(runtime.join("umu"), "#!/bin/sh\n").expect("umu");
        let shim = base.join("umu-shim");
        write_shim(&shim).expect("shim");
        Setup {
            proton,
            runtime: Some(runtime),
            shim,
        }
    }

    #[test]
    fn builds_umus_command_line_and_environment() {
        let base = std::env::temp_dir().join(format!("hyper-umu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let setup = fake_tools(&base);
        let prefix = base.join("prefix");
        std::fs::create_dir_all(&prefix).expect("prefix");
        let game_dir = base.join("game");
        std::fs::create_dir_all(&game_dir).expect("game");
        let exe = game_dir.join("Game.exe");
        std::fs::write(&exe, "MZ").expect("exe");

        let l = launch(
            &setup,
            &prefix,
            &exe.to_string_lossy(),
            &["-windowed".to_string()],
            None,
            "umu-1234",
            "egs",
            None,
        )
        .expect("launch");
        let proton = std::fs::canonicalize(&setup.proton).expect("canon");
        let runtime = setup.runtime.clone().expect("rt");
        let rt_s = runtime.to_string_lossy().into_owned();
        assert_eq!(l.argv[0], format!("{rt_s}/umu"));
        assert_eq!(l.argv[1], "--verb=waitforexitandrun");
        assert_eq!(l.argv[2], "--");
        assert_eq!(l.argv[3], setup.shim.to_string_lossy());
        assert_eq!(l.argv[4], format!("{}/proton", proton.to_string_lossy()));
        assert_eq!(l.argv[5], "waitforexitandrun");
        assert_eq!(l.argv[6], exe.to_string_lossy());
        assert_eq!(l.argv[7], "-windowed");
        assert_eq!(l.argv.len(), 8);

        let get = |k: &str| {
            l.env
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
        };
        let pfx = std::fs::canonicalize(&prefix).expect("canon");
        assert_eq!(get("WINEPREFIX"), pfx.to_string_lossy());
        assert_eq!(get("STEAM_COMPAT_DATA_PATH"), pfx.to_string_lossy());
        assert_eq!(
            get("STEAM_COMPAT_SHADER_PATH"),
            pfx.join("shadercache").to_string_lossy()
        );
        assert_eq!(get("GAMEID"), "umu-1234");
        assert_eq!(get("UMU_ID"), "umu-1234");
        assert_eq!(get("SteamAppId"), "1234");
        assert_eq!(get("SteamGameId"), "1234");
        assert_eq!(get("STORE"), "egs");
        assert_eq!(get("PROTON_VERB"), "waitforexitandrun");
        assert_eq!(get("STEAM_COMPAT_INSTALL_PATH"), game_dir.to_string_lossy());
        assert_eq!(
            get("STEAM_COMPAT_TOOL_PATHS"),
            format!("{}:{rt_s}", proton.to_string_lossy())
        );
        assert_eq!(get("STEAM_COMPAT_MOUNTS"), get("STEAM_COMPAT_TOOL_PATHS"));
        assert_eq!(get("RUNTIMEPATH"), rt_s);
        assert_eq!(
            get("STEAM_COMPAT_APP_ID"),
            fsutil::hex(&Md5::digest(pfx.to_string_lossy().as_bytes()))
        );
        assert_eq!(get("UMU_INVOCATION_ID").len(), 32);
        assert_eq!(get("FONTCONFIG_PATH"), "");
        // (not split on ':': Windows paths carry one in the drive letter)
        assert!(get("STEAM_RUNTIME_LIBRARY_PATH").contains(&*game_dir.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_empty_exe_creates_the_prefix_and_default_ids_stay_zero() {
        let base = std::env::temp_dir().join(format!("hyper-umu-pfx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let setup = fake_tools(&base);
        let prefix = base.join("prefix");
        std::fs::create_dir_all(&prefix).expect("prefix");
        let l =
            launch(&setup, &prefix, "", &[], None, "umu-default", "none", None).expect("launch");
        assert_eq!(l.argv.last().map(String::as_str), Some(""));
        let get = |k: &str| {
            l.env
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
        };
        assert_eq!(get("EXE"), "");
        assert_eq!(get("STEAM_COMPAT_INSTALL_PATH"), "");
        // umu derives the Steam id from `umu-<id>` even for `umu-default`
        assert_eq!(get("SteamAppId"), "default");
        let l = launch(&setup, &prefix, "", &[], None, "custom", "none", None).expect("launch");
        let get = |k: &str| {
            l.env
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
        };
        assert_eq!(get("SteamAppId"), "0");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_host_tool_runs_without_the_runtime() {
        let base = std::env::temp_dir().join(format!("hyper-umu-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mut setup = fake_tools(&base);
        setup.runtime = None;
        let proton = std::fs::canonicalize(&setup.proton).expect("canon");
        let argv = command(&setup, &proton, "run").expect("command");
        assert_eq!(
            argv,
            vec![
                setup.shim.to_string_lossy().into_owned(),
                format!("{}/proton", proton.to_string_lossy()),
                "run".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn relative_exes_resolve_against_the_launch_folder_bare_names_pass_through() {
        let base = std::env::temp_dir().join(format!("hyper-umu-cwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let setup = fake_tools(&base);
        let prefix = base.join("prefix");
        std::fs::create_dir_all(&prefix).expect("prefix");
        let game_dir = base.join("game").join("bin");
        std::fs::create_dir_all(&game_dir).expect("game");
        std::fs::write(game_dir.join("Game.exe"), "MZ").expect("exe");
        let get = |l: &Launch, k: &str| {
            l.env
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };

        // relative with a folder: against the launch's working folder, not Hyper's
        let l = launch(
            &setup,
            &prefix,
            "bin/Game.exe",
            &[],
            Some(&base.join("game")),
            "umu-1",
            "egs",
            None,
        )
        .expect("launch");
        // (compared as paths: the separators may be mixed on Windows)
        assert_eq!(PathBuf::from(get(&l, "EXE")), game_dir.join("Game.exe"));
        assert_eq!(
            PathBuf::from(get(&l, "STEAM_COMPAT_INSTALL_PATH")),
            game_dir
        );

        // a bare name is Wine's to find; it stays as it is and sets no install path
        for bare in ["start", "reg", "notepad.exe"] {
            let l = launch(
                &setup,
                &prefix,
                bare,
                &["/?".into()],
                None,
                "umu-1",
                "egs",
                Some("run"),
            )
            .expect("launch");
            assert_eq!(get(&l, "EXE"), bare);
            assert_eq!(get(&l, "STEAM_COMPAT_INSTALL_PATH"), "");
            assert_eq!(l.argv[l.argv.len() - 2], bare);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn host_paths_are_told_from_wine_ones() {
        assert!(!is_host_path("start"));
        assert!(!is_host_path("reg"));
        assert!(!is_host_path("a\\b.exe"));
        assert!(is_host_path("bin/Game.exe"));
        assert!(is_host_path("./Game.exe"));
        assert!(is_host_path("/games/x/Game.exe"));
        #[cfg(unix)]
        {
            assert!(!is_host_path("C:\\windows\\system32\\reg.exe"));
            assert!(!is_host_path("C:/windows/reg.exe"));
        }
    }

    #[test]
    fn tool_commands_are_rooted_at_the_tool() {
        let cmd = tool_command(
            Path::new("/opt/rt"),
            "/_v2-entry-point --verb=%verb% --",
            "run",
        );
        assert_eq!(cmd, ["/opt/rt/_v2-entry-point", "--verb=run", "--"]);
    }

    #[cfg(unix)]
    #[test]
    fn prefix_layout_matches_umu() {
        let base = std::env::temp_dir().join(format!("hyper-umu-prefix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let prefix = base.join("Prefixes").join("legendary").join("Game");
        setup_prefix(&prefix, &|| false).expect("setup");
        let pfx = prefix.join("pfx");
        assert_eq!(
            std::fs::read_link(&pfx).expect("pfx link"),
            PathBuf::from(".")
        );
        assert!(prefix.join("shadercache").is_dir() && prefix.join("gstreamer-1.0").is_dir());
        assert!(prefix.join("tracked_files").is_file());
        let steamuser = prefix.join("drive_c").join("users").join("steamuser");
        assert!(steamuser.is_dir());
        let me = fsutil::unix_user_name();
        if me != "steamuser" {
            assert_eq!(
                std::fs::read_link(prefix.join("drive_c").join("users").join(&me))
                    .expect("user link"),
                PathBuf::from("steamuser")
            );
        }
        // a second run changes nothing and does not fail
        setup_prefix(&prefix, &|| false).expect("setup again");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn folders_follow_xdg() {
        let d = Dirs::detect();
        assert!(d.local.ends_with("umu"));
        assert!(d.cache.ends_with("umu"));
        assert!(
            d.steam_compat.ends_with("Steam/compatibilitytools.d")
                || d.steam_compat.ends_with("Steam\\compatibilitytools.d")
        );
        assert_eq!(d.umu_compat, d.local.join("compatibilitytools"));
    }
}
