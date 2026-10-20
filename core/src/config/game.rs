//! Per-game launch settings (`GamesConfig/<store>-<game>.json`): the part
//! of Heroic's game settings that is about starting the game (extra
//! arguments, another executable, a wrapper command, environment
//! variables; legendary's `--override-exe`, `--wrapper`, `start_params` and
//! env config) plus the password of a private GOG branch. A file exists
//! only while something is set.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::types::{LaunchSpec, Runner};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GameSettings {
    /// Appended to the game's command line (split like a command line).
    pub launch_args: String,
    /// Another program to start instead of the build's: absolute, or
    /// relative to the game folder.
    pub target_exe: String,
    /// A program (with arguments) the whole launch command is appended to
    /// (`mangohud`, `gamemoderun`); on Linux it wraps Proton itself.
    pub wrapper: String,
    pub env: Vec<EnvVar>,
    /// Password of a private GOG branch (beta builds).
    pub gog_branch_password: String,
}

impl GameSettings {
    pub fn path(runner: Runner, app_name: &str) -> PathBuf {
        paths::game_configs_dir().join(format!(
            "{}-{}.json",
            runner.id(),
            crate::compat::safe_name(app_name)
        ))
    }

    /// A file that does not parse is moved aside (`<name>.broken-<time>`)
    /// and reported, never silently overwritten by the next save.
    pub fn load(runner: Runner, app_name: &str) -> Self {
        let path = Self::path(runner, app_name);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "game settings unreadable; using defaults");
                super::set_aside_broken(&path);
                Self::default()
            }
        }
    }

    /// Write the settings; a default (empty) set removes the file.
    pub fn save(&self, runner: Runner, app_name: &str) -> crate::error::Result<()> {
        let path = Self::path(runner, app_name);
        if *self == Self::default() {
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            return Ok(());
        }
        std::fs::create_dir_all(paths::game_configs_dir())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Executable, arguments and environment of a launch. `install_dir`
    /// resolves a relative `target_exe`. The wrapper is added later
    /// ([`Self::apply_wrapper`]), outside any Proton wrapping.
    pub fn apply_launch(&self, spec: &mut LaunchSpec, install_dir: Option<&Path>) {
        let target = self.target_exe.trim();
        if !target.is_empty() {
            let p = Path::new(target);
            let base = install_dir
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(&spec.working_dir));
            let exe = if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.join(p)
            };
            spec.executable = exe.to_string_lossy().into_owned();
        }
        if !self.launch_args.trim().is_empty() {
            spec.args.extend(split_args(self.launch_args.trim()));
        }
        for v in &self.env {
            let key = v.key.trim();
            if !key.is_empty() {
                spec.env.push((key.to_string(), v.value.clone()));
            }
        }
    }

    /// `wrapper args… <command>`: the wrapper becomes the program to start.
    pub fn apply_wrapper(&self, spec: &mut LaunchSpec) {
        let parts = split_args(self.wrapper.trim());
        let Some((wrapper, rest)) = parts.split_first() else {
            return;
        };
        let mut args: Vec<String> = rest.to_vec();
        args.push(std::mem::take(&mut spec.executable));
        args.append(&mut spec.args);
        spec.executable = wrapper.clone();
        spec.args = args;
    }
}

/// Split a command line typed by the user into arguments: whitespace
/// separates them, double or single quotes group them and are dropped
/// (`--map "Big Hall"` → `--map`, `Big Hall`). No backslash escapes: the
/// arguments are passed to the program as they are, and Windows paths keep
/// their backslashes. A quoted empty string is an empty argument.
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in line.chars() {
        match (c, quote) {
            (q, Some(open)) if q == open => quote = None,
            ('"' | '\'', None) => {
                quote = Some(c);
                has = true;
            }
            (c, None) if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (c, _) => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LaunchSpec {
        LaunchSpec {
            executable: "/g/Game.exe".into(),
            args: vec!["-a".into()],
            working_dir: "/g".into(),
            env: Vec::new(),
        }
    }

    #[test]
    fn applies_target_args_and_env() {
        let s = GameSettings {
            launch_args: "-windowed \"-res 1920 1080\"".into(),
            target_exe: "Bin/Other.exe".into(),
            env: vec![
                EnvVar {
                    key: "DXVK_HUD".into(),
                    value: "fps".into(),
                },
                EnvVar {
                    key: " ".into(),
                    value: "x".into(),
                },
            ],
            ..Default::default()
        };
        let mut l = spec();
        s.apply_launch(&mut l, Some(Path::new("/g")));
        assert_eq!(
            l.executable,
            Path::new("/g").join("Bin/Other.exe").to_string_lossy()
        );
        assert_eq!(
            l.args,
            ["-a", "-windowed", "-res 1920 1080"],
            "quotes group and are dropped"
        );
        assert_eq!(l.env, [("DXVK_HUD".to_string(), "fps".to_string())]);
        s.apply_wrapper(&mut l);
        assert_eq!(
            l.executable,
            Path::new("/g").join("Bin/Other.exe").to_string_lossy(),
            "no wrapper set"
        );
    }

    #[test]
    fn wrapper_takes_the_command() {
        let s = GameSettings {
            wrapper: "mangohud --dlsym".into(),
            ..Default::default()
        };
        let mut l = spec();
        s.apply_wrapper(&mut l);
        assert_eq!(l.executable, "mangohud");
        assert_eq!(l.args, ["--dlsym", "/g/Game.exe", "-a"]);

        let s = GameSettings {
            wrapper: r#""C:\Tools\my wrapper.exe" --flag"#.into(),
            ..Default::default()
        };
        let mut l = spec();
        s.apply_wrapper(&mut l);
        assert_eq!(
            l.executable, r"C:\Tools\my wrapper.exe",
            "the quoted program is one name without the quotes"
        );
        assert_eq!(l.args, ["--flag", "/g/Game.exe", "-a"]);
    }

    #[test]
    fn splits_a_command_line() {
        assert_eq!(
            split_args(r#"-windowed  --map "Big Hall" """#),
            ["-windowed", "--map", "Big Hall", ""]
        );
        assert_eq!(
            split_args(r#"-conf "a b.conf" -x 'c d' plain"#),
            ["-conf", "a b.conf", "-x", "c d", "plain"]
        );
        assert_eq!(
            split_args(r#"--path="C:\My Games\x""#),
            [r"--path=C:\My Games\x"]
        );
        assert_eq!(
            split_args("it's"),
            ["its"],
            "an unmatched quote opens a group"
        );
        assert_eq!(split_args(r#""it's""#), ["it's"]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn save_and_load_round_trip_and_default_removes_the_file() {
        let s = GameSettings {
            launch_args: "-x".into(),
            ..Default::default()
        };
        s.save(Runner::Sideload, "test game").expect("save");
        assert_eq!(GameSettings::load(Runner::Sideload, "test game"), s);
        GameSettings::default()
            .save(Runner::Sideload, "test game")
            .expect("clear");
        assert!(!GameSettings::path(Runner::Sideload, "test game").exists());
        assert_eq!(
            GameSettings::load(Runner::Sideload, "test game"),
            GameSettings::default()
        );
    }
}
