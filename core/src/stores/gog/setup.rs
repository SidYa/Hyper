//! What Galaxy does after a Windows game is installed (port of Heroic's
//! `setup.ts`): run the build's setup script through GOG's script
//! interpreter (ISI), or for older builds its temp / support installers,
//! then the installers of the redistributables the game depends on. The
//! programs come from the game's support folder and the shared redist
//! folder (`native::redist`).
//!
//! Windows: every program runs elevated (one UAC prompt each, like Heroic's
//! `Start-Process -Verb RunAs`; `platform::windows::run_elevated`). Linux:
//! inside the game's Wine prefix through Proton (`Compat::run_in_prefix`),
//! with the host paths written as Wine's `Z:` drive.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::installed::split_args;
use super::native::{languages, redist};
use crate::stores::is_confined_rel_path;

/// The installed game the setup is for.
pub struct Game<'a> {
    pub app_name: &'a str,
    pub install_path: &'a Path,
    pub version: &'a str,
    pub build_id: &'a str,
    /// Install language code (`en-US`).
    pub language: &'a str,
    pub dlcs: &'a [String],
}

/// One program to run, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Shown in the UI ("Installing DirectX").
    pub label: String,
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// A host path as the installers see it: as it is on Windows; on Wine's
/// `Z:` drive (the whole host file system) under Proton.
pub fn win_path(p: &Path, wine: bool) -> String {
    if wine {
        format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
    } else {
        p.to_string_lossy().into_owned()
    }
}

/// Heroic uses the English name of the language's primary subtag
/// (`Intl.DisplayNames`): `en-GB` → `English`, `pt-BR` → `Portuguese`.
fn language_name(code: &str) -> String {
    let primary = code.split('-').next().unwrap_or(code);
    languages::parse(primary)
        .or_else(|| languages::parse(code))
        .map(|l| l.name.to_string())
        .unwrap_or_else(|| "English".to_string())
}

/// A program named by a manifest, below `dir` (and below `product`'s
/// folder there): only a confined relative path to an existing file is
/// run, elevated on Windows. Anything else is logged and skipped.
fn program(dir: &Path, product: Option<&str>, rel: &str) -> Option<PathBuf> {
    let product_ok = product.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if !product_ok || !is_confined_rel_path(rel) {
        tracing::warn!(?product, rel, "setup program outside its folder skipped");
        return None;
    }
    let base = match product {
        Some(p) => dir.join(p),
        None => dir.to_path_buf(),
    };
    let exe = base.join(rel.replace('\\', "/"));
    if !exe.starts_with(dir) || !exe.is_file() {
        tracing::warn!(path = %exe.display(), "setup program missing; skipped");
        return None;
    }
    Some(exe)
}

/// Windows Installer: by full path on Windows (a bare name would be looked
/// up in the redist folder first, the working directory); inside the
/// prefix Wine resolves its own.
fn msiexec(wine: bool) -> PathBuf {
    if wine {
        return PathBuf::from("msiexec");
    }
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    root.join("System32").join("msiexec.exe")
}

/// The programs to run for `game`: `manifest` is its saved build manifest,
/// `redist_manifest` the shared `.gogdl-redist-manifest` (when the
/// redistributables were fetched). `wine`: paths for Proton.
pub fn steps(
    game: &Game,
    manifest: &Value,
    support_dir: &Path,
    redist_dir: &Path,
    redist_manifest: Option<&Value>,
    install_redist: bool,
    wine: bool,
) -> Vec<Step> {
    let mut out = Vec::new();
    let deps = redist::manifest_dependencies(manifest);
    let lang = language_name(game.language);
    let lang_code = if game.language.is_empty() {
        "en-US"
    } else {
        game.language
    };
    // ScummVM games get the host paths as they are (Heroic's exception)
    let convert = wine && !deps.iter().any(|d| d.eq_ignore_ascii_case("scummvm"));
    let game_dir = win_path(game.install_path, convert);
    let support_win = win_path(support_dir, convert);
    let is_installed = |pid: &str| pid == game.app_name || game.dlcs.iter().any(|d| d == pid);

    if manifest.get("version").and_then(Value::as_u64) == Some(1) {
        if support_dir.is_dir() {
            let timestamp = manifest
                .pointer("/product/timestamp")
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_default();
            for cmd in manifest
                .pointer("/product/support_commands")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let languages: Vec<&str> = cmd
                    .get("languages")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                if !languages
                    .iter()
                    .any(|l| *l == lang || *l == game.language || *l == "Neutral")
                {
                    continue;
                }
                let (Some(product), Some(exe)) = (
                    cmd.get("gameID").and_then(Value::as_str),
                    cmd.get("executable").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let Some(exe) = program(support_dir, Some(product), exe) else {
                    continue;
                };
                let lower = lang.to_lowercase();
                out.push(Step {
                    label: format!("Running the setup of {product}"),
                    exe,
                    args: vec![
                        "/VERYSILENT".into(),
                        format!("/DIR={game_dir}"),
                        format!("/Language={lower}"),
                        format!("/LANG={lower}"),
                        format!("/ProductId={product}"),
                        "/galaxyclient".into(),
                        format!("/buildId={timestamp}"),
                        format!("/versionName={}", game.version),
                        "/nodesktopshorctut".into(), // GOG's own typo, expected by the installers
                        "/nodesktopshortcut".into(),
                    ],
                    cwd: game.install_path.to_path_buf(),
                });
            }
        }
    } else if manifest.get("scriptInterpreter").and_then(Value::as_bool) == Some(true) {
        let isi = redist_dir
            .join("__redist")
            .join(redist::ISI)
            .join("scriptinterpreter.exe");
        if !isi.is_file() {
            tracing::error!(path = %isi.display(), "GOG script interpreter missing; the game's setup script was not run");
        } else {
            for product in manifest
                .get("products")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(pid) = product.get("productId").and_then(Value::as_str) else {
                    continue;
                };
                if !is_installed(pid) {
                    continue;
                }
                out.push(Step {
                    label: format!("Running the setup script of {pid}"),
                    exe: isi.clone(),
                    args: vec![
                        "/VERYSILENT".into(),
                        format!("/DIR={game_dir}"),
                        format!("/Language={lang}"),
                        format!("/LANG={lang}"),
                        format!("/ProductId={pid}"),
                        "/galaxyclient".into(),
                        format!("/buildId={}", game.build_id),
                        format!("/versionName={}", game.version),
                        format!("/lang-code={lang_code}"),
                        format!("/supportDir={support_win}"),
                        "/nodesktopshorctut".into(),
                        "/nodesktopshortcut".into(),
                    ],
                    cwd: game.install_path.to_path_buf(),
                });
            }
        }
    } else {
        for product in manifest
            .get("products")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(pid) = product.get("productId").and_then(Value::as_str) else {
                continue;
            };
            let temp = product
                .get("temp_executable")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !is_installed(pid) || temp.is_empty() {
                continue;
            }
            let Some(exe) = program(support_dir, Some(pid), temp) else {
                continue;
            };
            let lower = lang.to_lowercase();
            out.push(Step {
                label: format!("Running the setup of {pid}"),
                exe,
                args: vec![
                    "/VERYSILENT".into(),
                    format!("/DIR={game_dir}"),
                    format!("/Language={lower}"),
                    format!("/LANG={lower}"),
                    format!("/lang-code={lang_code}"),
                    format!("/ProductId={pid}"),
                    "/galaxyclient".into(),
                    format!("/buildId={}", game.build_id),
                    format!("/versionName={}", game.version),
                    "/nodesktopshorctut".into(),
                    "/nodesktopshortcut".into(),
                ],
                cwd: game.install_path.to_path_buf(),
            });
        }
    }

    if let (true, Some(redist_manifest)) = (install_redist, redist_manifest) {
        for dep in &deps {
            let Some(entry) = redist::entry(redist_manifest, dep) else {
                tracing::warn!(dep, "no redistributable data; skipped");
                continue;
            };
            if entry.exe_path.is_empty() {
                continue;
            }
            let Some(exe) = program(redist_dir, None, &entry.exe_path) else {
                continue;
            };
            let mut args = split_args(&entry.exe_args);
            let label = format!(
                "Installing {}",
                if entry.readable_name.is_empty() {
                    dep.clone()
                } else {
                    entry.readable_name.clone()
                }
            );
            // Heroic's hack: the legacy PhysX package is an MSI with no silent mode of its own
            let (exe, args) = if dep == "PHYSXLEGACY" {
                let mut a = vec!["/i".to_string(), win_path(&exe, convert)];
                a.append(&mut args);
                a.push("/qb".into());
                (msiexec(wine), a)
            } else {
                (exe, args)
            };
            out.push(Step {
                label,
                exe,
                args,
                cwd: redist_dir.to_path_buf(),
            });
        }
    }
    out
}

/// One Windows command line from arguments, quoted the way
/// `CommandLineToArgvW` reads it back (`ShellExecuteEx` takes a string).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_command_line(args: &[String]) -> String {
    let mut out = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '"']) {
            out.push_str(arg);
            continue;
        }
        out.push('"');
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                c => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    out.push(c);
                    backslashes = 0;
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redist_manifest() -> Value {
        serde_json::json!({ "depots": [
            { "dependencyId": "ISI", "readableName": "Script interpreter", "executable": { "path": "__redist/ISI/scriptinterpreter.exe", "arguments": "" } },
            { "dependencyId": "DirectX", "readableName": "DirectX", "executable": { "path": "__redist/DirectX/DXSETUP.exe", "arguments": "/silent /x" } },
            { "dependencyId": "PHYSXLEGACY", "readableName": "PhysX legacy", "executable": { "path": "__redist/PhysX/legacy.msi", "arguments": "" } },
            { "dependencyId": "nothing", "readableName": "Nothing", "executable": { "path": "", "arguments": "" } }
        ] })
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hyper-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    #[test]
    fn v2_runs_the_script_interpreter_per_installed_product_then_the_redists() {
        let base = temp("v2");
        let redist_dir = base.join("redist");
        std::fs::create_dir_all(redist_dir.join("__redist/ISI")).expect("isi dir");
        std::fs::write(redist_dir.join("__redist/ISI/scriptinterpreter.exe"), "MZ").expect("isi");
        for f in ["__redist/DirectX/DXSETUP.exe", "__redist/PhysX/legacy.msi"] {
            let p = redist_dir.join(f);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("redist dir");
            std::fs::write(p, "MZ").expect("redist");
        }
        let manifest = serde_json::json!({
            "version": 2, "scriptInterpreter": true, "dependencies": ["DirectX", "PHYSXLEGACY", "nothing", "unknown"],
            "products": [ { "productId": "100" }, { "productId": "200" }, { "productId": "300" } ]
        });
        let dlcs = vec!["200".to_string()];
        let game = Game {
            app_name: "100",
            install_path: Path::new("/games/Witcher"),
            version: "1.2",
            build_id: "b9",
            language: "en-GB",
            dlcs: &dlcs,
        };
        let steps = steps(
            &game,
            &manifest,
            Path::new("/support/100"),
            &redist_dir,
            Some(&redist_manifest()),
            true,
            true,
        );
        assert_eq!(steps.len(), 4, "{steps:#?}");
        assert_eq!(
            steps[0].exe,
            redist_dir.join("__redist/ISI/scriptinterpreter.exe")
        );
        assert!(steps[0].args.contains(&"/ProductId=100".to_string()));
        assert!(
            steps[0]
                .args
                .contains(&"/DIR=Z:\\games\\Witcher".to_string())
        );
        assert!(steps[0].args.contains(&"/Language=English".to_string()));
        assert!(steps[0].args.contains(&"/lang-code=en-GB".to_string()));
        assert!(
            steps[0]
                .args
                .contains(&"/supportDir=Z:\\support\\100".to_string())
        );
        assert!(steps[0].args.contains(&"/buildId=b9".to_string()));
        assert_eq!(steps[0].cwd, PathBuf::from("/games/Witcher"));
        assert!(
            steps[1].args.contains(&"/ProductId=200".to_string()),
            "installed DLC"
        );
        assert_eq!(steps[2].label, "Installing DirectX");
        assert_eq!(
            steps[2].exe,
            redist_dir.join("__redist/DirectX/DXSETUP.exe")
        );
        assert_eq!(steps[2].args, ["/silent", "/x"]);
        assert_eq!(steps[2].cwd, redist_dir);
        assert_eq!(steps[3].exe, PathBuf::from("msiexec"), "Wine finds its own");
        assert_eq!(steps[3].args[0], "/i");
        assert!(steps[3].args[1].starts_with("Z:"));
        assert_eq!(steps[3].args.last().map(String::as_str), Some("/qb"));
        // no redist pass on an update
        let only_scripts = super::steps(
            &game,
            &manifest,
            Path::new("/support/100"),
            &redist_dir,
            Some(&redist_manifest()),
            false,
            true,
        );
        assert_eq!(only_scripts.len(), 2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn v2_without_the_interpreter_uses_temp_executables_and_windows_paths() {
        let base = temp("v2-temp");
        let support = base.join("support");
        std::fs::create_dir_all(support.join("100")).expect("support");
        std::fs::write(support.join("100").join("setup_game.exe"), "MZ").expect("exe");
        let manifest = serde_json::json!({
            "version": 2, "dependencies": [],
            "products": [ { "productId": "100", "temp_executable": "setup_game.exe" }, { "productId": "200", "temp_executable": "" } ]
        });
        let game = Game {
            app_name: "100",
            install_path: Path::new("D:\\Games\\X"),
            version: "1",
            build_id: "b",
            language: "de-DE",
            dlcs: &[],
        };
        let steps = steps(
            &game,
            &manifest,
            &support,
            Path::new("C:\\redist"),
            None,
            true,
            false,
        );
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].exe, support.join("100").join("setup_game.exe"));
        assert!(steps[0].args.contains(&"/DIR=D:\\Games\\X".to_string()));
        assert!(steps[0].args.contains(&"/Language=german".to_string()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn programs_outside_their_folder_or_missing_are_skipped() {
        let base = temp("confined");
        let support = base.join("support");
        let redist_dir = base.join("redist");
        std::fs::create_dir_all(support.join("100")).expect("support");
        std::fs::create_dir_all(&redist_dir).expect("redist");
        std::fs::write(base.join("evil.exe"), "MZ").expect("evil");
        std::fs::write(support.join("100").join("ok.exe"), "MZ").expect("ok");
        assert!(program(&support, Some("100"), "ok.exe").is_some());
        assert!(program(&support, Some("100"), "..\\..\\evil.exe").is_none());
        assert!(program(&support, Some("100"), "../../evil.exe").is_none());
        assert!(program(&support, Some(".."), "evil.exe").is_none());
        assert!(program(&support, Some("100"), "missing.exe").is_none());
        assert!(program(&support, Some("100"), "C:\\Windows\\System32\\cmd.exe").is_none());
        assert!(program(&redist_dir, None, "../evil.exe").is_none());
        let manifest = serde_json::json!({
            "version": 2, "dependencies": ["Evil"],
            "products": [ { "productId": "100", "temp_executable": "../../evil.exe" } ]
        });
        let repo = serde_json::json!({ "depots": [
            { "dependencyId": "Evil", "executable": { "path": "..\\evil.exe", "arguments": "" } }
        ] });
        let game = Game {
            app_name: "100",
            install_path: &base,
            version: "1",
            build_id: "b",
            language: "en-US",
            dlcs: &[],
        };
        assert!(
            steps(
                &game,
                &manifest,
                &support,
                &redist_dir,
                Some(&repo),
                true,
                false
            )
            .is_empty()
        );
        #[cfg(windows)]
        assert!(msiexec(false).ends_with("System32\\msiexec.exe") && msiexec(false).is_absolute());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn v1_support_commands_match_the_language() {
        let base = temp("v1");
        let support = base.join("support").join("100");
        std::fs::create_dir_all(support.join("100")).expect("support");
        for exe in ["setup_en.exe", "setup_pl.exe", "setup_all.exe"] {
            std::fs::write(support.join("100").join(exe), "MZ").expect("exe");
        }
        let manifest = serde_json::json!({ "version": 1, "product": { "timestamp": 1700000000, "depots": [ { "redist": "DirectX" } ],
            "support_commands": [
                { "gameID": "100", "executable": "setup_en.exe", "languages": ["English"] },
                { "gameID": "100", "executable": "setup_pl.exe", "languages": ["Polish"] },
                { "gameID": "100", "executable": "setup_all.exe", "languages": ["Neutral"] }
            ] } });
        let game = Game {
            app_name: "100",
            install_path: Path::new("/g"),
            version: "1",
            build_id: "",
            language: "en-US",
            dlcs: &[],
        };
        let steps = steps(
            &game,
            &manifest,
            &support,
            Path::new("/redist"),
            None,
            true,
            true,
        );
        let exes: Vec<_> = steps
            .iter()
            .map(|s| {
                s.exe
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(exes, ["setup_en.exe", "setup_all.exe"]);
        assert!(steps[0].args.contains(&"/buildId=1700000000".to_string()));
        // no support folder: nothing to run
        assert!(
            super::steps(
                &game,
                &manifest,
                &base.join("nope"),
                Path::new("/redist"),
                None,
                true,
                true
            )
            .is_empty()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn command_lines_quote_like_windows() {
        let args = [
            "/VERYSILENT".to_string(),
            "/DIR=C:\\Program Files\\Game".to_string(),
            "".to_string(),
            "say \"hi\"".to_string(),
            "trail\\".to_string(),
        ];
        assert_eq!(
            windows_command_line(&args),
            r#"/VERYSILENT "/DIR=C:\Program Files\Game" "" "say \"hi\"" trail\"#
        );
        assert_eq!(windows_command_line(&["a b\\".to_string()]), r#""a b\\""#);
    }

    #[test]
    fn language_names_follow_the_primary_subtag() {
        assert_eq!(language_name("en-GB"), "English");
        assert_eq!(language_name("en-US"), "English");
        assert_eq!(language_name("de-DE"), "German");
        assert_eq!(language_name(""), "English");
        assert_eq!(
            win_path(Path::new("/home/deck/Games/X"), true),
            "Z:\\home\\deck\\Games\\X"
        );
        assert_eq!(win_path(Path::new("D:\\Games\\X"), false), "D:\\Games\\X");
    }
}
