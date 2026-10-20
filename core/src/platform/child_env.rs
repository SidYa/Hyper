//! The environment programs started by the launcher get.
//!
//! Started from an AppImage, the launcher's own environment points into the
//! image (`/tmp/.mount_*`): GTK / GIO / GdkPixbuf modules, `PYTHONHOME`,
//! `XDG_DATA_DIRS`, sometimes `PATH` / `LD_LIBRARY_PATH`. The launcher needs
//! them for its own GTK and WebKit, but a child must see the host's: Proton
//! is a Python script that dies at once with the image's `PYTHONHOME`, and the
//! Steam Linux Runtime's tools load GLib modules built for another system.
//! Outside an AppImage nothing changes.

use std::ffi::OsString;
use std::sync::OnceLock;

/// What to change in a child's environment.
#[derive(Debug, Default, PartialEq)]
pub struct ChildEnv {
    /// Variables to drop.
    pub remove: Vec<OsString>,
    /// Variables to set (path lists with the image's entries cut out).
    pub set: Vec<(OsString, OsString)>,
}

/// Set by the AppImage runtime / its AppRun only; always dropped.
const APPIMAGE_ONLY: &[&str] = &[
    "APPIMAGE",
    "APPDIR",
    "ARGV0",
    "OWD",
    "GTK_THEME",
    "APPIMAGE_GTK_THEME",
];

/// The changes for the current process, worked out once.
pub fn child_env() -> &'static ChildEnv {
    static ENV: OnceLock<ChildEnv> = OnceLock::new();
    ENV.get_or_init(|| {
        let appdir = std::env::var_os("APPDIR").filter(|_| std::env::var_os("APPIMAGE").is_some());
        match appdir {
            Some(dir) => compute(&dir.to_string_lossy(), std::env::vars_os()),
            None => ChildEnv::default(),
        }
    })
}

fn compute(appdir: &str, vars: impl Iterator<Item = (OsString, OsString)>) -> ChildEnv {
    let appdir = appdir.trim_end_matches('/');
    let mut out = ChildEnv::default();
    if appdir.is_empty() {
        return out;
    }
    let inside = |entry: &str| {
        // `$APPDIR//usr/lib` as the GTK hook writes it counts too
        entry == appdir
            || entry
                .strip_prefix(appdir)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    for (key, value) in vars {
        let name = key.to_string_lossy();
        if APPIMAGE_ONLY.contains(&name.as_ref()) {
            out.remove.push(key);
            continue;
        }
        let text = value.to_string_lossy();
        if !text.contains(appdir) {
            continue;
        }
        let entries: Vec<&str> = text.split(':').filter(|e| !e.is_empty()).collect();
        if !entries.iter().any(|e| inside(e)) {
            continue;
        }
        // a single value pointing into the image (PYTHONHOME, GIO_MODULE_DIR…)
        // or a list made only of such entries goes; a list keeps the rest
        let kept: Vec<&str> = entries.into_iter().filter(|e| !inside(e)).collect();
        if kept.is_empty() {
            out.remove.push(key);
        } else {
            out.set.push((key, OsString::from(kept.join(":"))));
        }
    }
    out
}

/// Apply [`child_env`] to a `std` command.
pub fn apply_std(cmd: &mut std::process::Command) {
    let env = child_env();
    for k in &env.remove {
        cmd.env_remove(k);
    }
    for (k, v) in &env.set {
        cmd.env(k, v);
    }
}

/// Apply [`child_env`] to a tokio command.
pub fn apply(cmd: &mut tokio::process::Command) {
    let env = child_env();
    for k in &env.remove {
        cmd.env_remove(k);
    }
    for (k, v) in &env.set {
        cmd.env(k, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(list: &[(&str, &str)]) -> impl Iterator<Item = (OsString, OsString)> {
        list.iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn the_image_is_cut_out_of_the_child_environment() {
        let d = "/tmp/.mount_hyper.abc";
        let env = compute(
            d,
            vars(&[
                ("APPIMAGE", "/home/u/Hyper.AppImage"),
                ("APPDIR", d),
                ("GTK_THEME", "Adwaita:dark"),
                ("PYTHONHOME", "/tmp/.mount_hyper.abc/usr/"),
                ("PYTHONPATH", "/tmp/.mount_hyper.abc/usr/share/pyshared/:"),
                (
                    "GIO_MODULE_DIR",
                    "/tmp/.mount_hyper.abc//usr/lib/gio/modules",
                ),
                (
                    "XDG_DATA_DIRS",
                    "/tmp/.mount_hyper.abc/usr/share:/usr/share:/usr/local/share",
                ),
                ("PATH", "/tmp/.mount_hyper.abc/usr/bin:/usr/bin:/bin"),
                ("HOME", "/home/u"),
                ("OTHER", "/tmp/.mount_hyper.abcdef/x"),
            ]),
        );
        let removed: Vec<String> = env
            .remove
            .iter()
            .map(|k| k.to_string_lossy().into_owned())
            .collect();
        for k in [
            "APPIMAGE",
            "APPDIR",
            "GTK_THEME",
            "PYTHONHOME",
            "PYTHONPATH",
            "GIO_MODULE_DIR",
        ] {
            assert!(removed.contains(&k.to_string()), "{k} removed");
        }
        let set: Vec<(String, String)> = env
            .set
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert!(set.contains(&("XDG_DATA_DIRS".into(), "/usr/share:/usr/local/share".into())));
        assert!(set.contains(&("PATH".into(), "/usr/bin:/bin".into())));
        // not the image: a folder that only starts with the same characters
        assert!(!removed.contains(&"OTHER".to_string()) && !set.iter().any(|(k, _)| k == "OTHER"));
        assert!(!removed.contains(&"HOME".to_string()));
    }

    #[test]
    fn nothing_changes_outside_an_appimage() {
        assert_eq!(
            compute("", vars(&[("PYTHONHOME", "/usr")])),
            ChildEnv::default()
        );
    }
}
