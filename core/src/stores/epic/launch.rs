//! What `legendary launch --dry-run --json` works out: the executable from
//! `installed.json` and the arguments the Epic Games Launcher passes a game
//! (`core.py` `get_launch_parameters`): the game's own parameters, then
//! Epic's login (`-AUTH_PASSWORD=<exchange code>`), the ownership token file
//! for games that check it, the account and locale.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::api::Epic;
use super::{metadata_path, read_json};
use crate::error::{Error, Result};
use crate::stores::legendary::library::read_installed;
use crate::types::LaunchSpec;

/// `offline`: no tokens, nothing asked from Epic (the game starts without a
/// login; not every game accepts that, see `can_run_offline`).
pub async fn spec(
    epic: &Epic,
    app_name: &str,
    offline: bool,
    language: &str,
) -> Result<LaunchSpec> {
    let dir = &epic.config_dir;
    let installed = read_installed(dir)
        .remove(app_name)
        .ok_or_else(|| Error::Other(format!("{app_name} is not installed")))?;
    let game = read_json(&metadata_path(dir, app_name))
        .ok_or_else(|| Error::GameNotFound(app_name.to_string()))?;
    let attr = |k: &str| {
        game.pointer(&format!("/metadata/customAttributes/{k}/value"))
            .and_then(Value::as_str)
    };
    let (namespace, catalog_id) = super::library::catalog_ids(&game);

    // installed.json is a plain file: the executable must stay inside the game folder
    let exe_rel = installed.executable.replace('\\', "/");
    let exe_path = super::installed::locate_checked(
        Path::new(&installed.install_path),
        exe_rel.trim_start_matches('/'),
        false,
    )?;
    let working_dir = exe_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(&installed.install_path));

    let mut args = split_args_keep_quotes(&installed.launch_parameters);
    if let Some(extra) = attr("AdditionalCommandLine") {
        args.extend(split_args_keep_quotes(extra.trim()));
    }

    let (account_id, display_name) = epic.profile()?;
    let game_token = if offline {
        String::new()
    } else {
        epic.exchange_code().await?.to_string()
    };
    if offline && !installed.can_run_offline {
        tracing::warn!(
            app = app_name,
            "game is not approved for offline use and may not work"
        );
    }
    args.extend([
        "-AUTH_LOGIN=unused".to_string(),
        format!("-AUTH_PASSWORD={game_token}"),
        "-AUTH_TYPE=exchangecode".to_string(),
        format!("-epicapp={app_name}"),
        "-epicenv=Prod".to_string(),
    ]);
    if installed.requires_ot && !offline {
        let token = epic
            .ownership_token(&account_id, &namespace, &catalog_id)
            .await?;
        let tmp = dir.join("tmp");
        crate::secrets::create_private_dir(&tmp)?;
        // tokens of earlier launches go: the game only reads the file at start
        remove_ownership_tokens(&tmp);
        let path = tmp.join(format!("{namespace}{catalog_id}.ovt"));
        crate::secrets::write_private(&path, &token)?;
        args.push(format!("-epicovt={}", path.display()));
    }
    args.extend([
        "-EpicPortal".to_string(),
        format!("-epicusername={display_name}"),
        format!("-epicuserid={account_id}"),
        format!("-epiclocale={language}"),
        format!("-epicsandboxid={namespace}"),
    ]);
    if let Some(deployment) = game
        .pointer("/sidecar/config/deploymentId")
        .and_then(Value::as_str)
    {
        args.push(format!("-epicdeploymentid={deployment}"));
    }
    Ok(LaunchSpec {
        executable: exe_path.to_string_lossy().into_owned(),
        args,
        working_dir: working_dir.to_string_lossy().into_owned(),
        env: Vec::new(),
    })
}

/// Remove the ownership tokens (`tmp/*.ovt`) earlier launches wrote: a game
/// reads its token once at start, so none needs to outlive the launch. Also
/// run at start, for a launch the app did not survive.
pub fn remove_ownership_tokens(tmp: &Path) {
    let Ok(rd) = std::fs::read_dir(tmp) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "ovt")
            && let Err(err) = crate::secrets::wipe_file(&p)
        {
            tracing::warn!(path = %p.display(), error = %err, "could not remove an ownership token");
        }
    }
}

/// `shlex.split(posix=False)`: split on whitespace, quotes group a token and
/// stay part of it (Windows programs parse their own quotes).
pub(crate) fn split_args_keep_quotes(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_shlex_non_posix() {
        assert_eq!(
            split_args_keep_quotes(r#"-a "b c" d"#),
            ["-a", "\"b c\"", "d"]
        );
        assert_eq!(
            split_args_keep_quotes("  -windowed   -res=1920x1080 "),
            ["-windowed", "-res=1920x1080"]
        );
        assert_eq!(
            split_args_keep_quotes(r#"-path="C:\Games\X""#),
            ["-path=\"C:\\Games\\X\""]
        );
        assert!(split_args_keep_quotes("").is_empty());
    }
}
