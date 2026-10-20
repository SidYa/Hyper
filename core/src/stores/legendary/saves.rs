//! Where an Epic game keeps its saves on this PC: the game's
//! `CloudSaveFolder` attribute with its variables filled in, the same way
//! `legendary`'s `get_save_path` does on Windows (legendary would also ask on
//! the console when it cannot resolve one, which we cannot answer).
//!
//! The variables resolve against [`Roots`]: the user's folders on Windows,
//! the game's Wine prefix on Linux.

use std::path::PathBuf;

use regex::Regex;

use crate::saves::Roots;

/// `CloudSaveFolder` → absolute folder, or `None` when a variable is unknown.
pub fn resolve(
    folder: &str,
    install_path: &str,
    epic_id: Option<&str>,
    roots: &Roots,
) -> Option<String> {
    resolve_with(folder, &|var: &str| -> Option<PathBuf> {
        match var.to_ascii_lowercase().as_str() {
            "installdir" => Some(PathBuf::from(install_path)),
            "epicid" => epic_id.filter(|s| !s.is_empty()).map(PathBuf::from),
            "appdata" => Some(roots.local_app_data.clone()),
            "userdir" => Some(roots.documents.clone()),
            "userprofile" => Some(roots.user_profile.clone()),
            "usersavedgames" => Some(roots.saved_games.clone()),
            _ => None,
        }
    })
}

fn resolve_with(folder: &str, lookup: &dyn Fn(&str) -> Option<PathBuf>) -> Option<String> {
    let re = Regex::new(r"\{(\w+)\}").ok()?;
    let mut unknown = false;
    let replaced = re.replace_all(folder.trim(), |c: &regex::Captures| match lookup(&c[1]) {
        Some(p) => p.to_string_lossy().into_owned(),
        None => {
            unknown = true;
            String::new()
        }
    });
    if unknown || replaced.contains('%') || replaced.is_empty() {
        return None;
    }
    // "{AppData}/../LocalLow/…" is common: fold the "..", or the save-folder
    // check would refuse it
    crate::saves::normalize(&replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn lookup(var: &str) -> Option<PathBuf> {
        match var.to_ascii_lowercase().as_str() {
            "installdir" => Some(PathBuf::from("E:\\Games\\Game")),
            "appdata" => Some(PathBuf::from("C:\\Users\\a\\AppData\\Local")),
            "epicid" => Some(PathBuf::from("abc123")),
            _ => None,
        }
    }

    #[cfg(windows)]
    #[test]
    fn fills_in_variables_case_insensitively() {
        assert_eq!(
            resolve_with("{AppData}/Studio/Game/Saved/SaveGames/{EpicId}/", &lookup).as_deref(),
            Some("C:\\Users\\a\\AppData\\Local\\Studio\\Game\\Saved\\SaveGames\\abc123")
        );
        assert_eq!(
            resolve_with("{installdir}\\\\saves", &lookup).as_deref(),
            Some("E:\\Games\\Game\\saves")
        );
        // Disco Elysium
        assert_eq!(
            resolve_with(
                "{appdata}/../locallow/ZAUM Studio/Disco Elysium/SaveGames/",
                &lookup
            )
            .as_deref(),
            Some("C:\\Users\\a\\AppData\\locallow\\ZAUM Studio\\Disco Elysium\\SaveGames")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolves_into_the_wine_prefix() {
        let roots = Roots::prefix(std::path::Path::new(
            "/home/deck/Games/Hyper/Prefixes/legendary/Game",
        ));
        let user = "/home/deck/Games/Hyper/Prefixes/legendary/Game/drive_c/users/steamuser";
        assert_eq!(
            resolve(
                "{AppData}/Studio/Game/Saved/SaveGames/{EpicId}/",
                "/home/deck/Games/Hyper/Game",
                Some("abc123"),
                &roots
            )
            .as_deref(),
            Some(format!("{user}/AppData/Local/Studio/Game/Saved/SaveGames/abc123").as_str())
        );
        assert_eq!(
            resolve(
                "{installdir}\\\\saves",
                "/home/deck/Games/Hyper/Game",
                None,
                &roots
            )
            .as_deref(),
            Some("/home/deck/Games/Hyper/Game/saves")
        );
        assert_eq!(
            resolve(
                "{appdata}/../locallow/ZAUM Studio/Disco Elysium/SaveGames/",
                "/g",
                None,
                &roots
            )
            .as_deref(),
            Some(format!("{user}/AppData/locallow/ZAUM Studio/Disco Elysium/SaveGames").as_str())
        );
        assert_eq!(
            resolve("{UserSavedGames}\\Studio\\Game", "/g", None, &roots).as_deref(),
            Some(format!("{user}/Saved Games/Studio/Game").as_str())
        );
    }

    #[test]
    fn unknown_variables_give_no_path() {
        let roots = Roots::prefix(std::path::Path::new("/p"));
        assert_eq!(resolve("{SomethingElse}/Saves", "/g", None, &roots), None);
        assert_eq!(resolve("%APPDATA%/Saves", "/g", None, &roots), None);
        assert_eq!(resolve("Saves", "/g", None, &roots), None, "relative");
        assert_eq!(
            resolve("{AppData}/{EpicId}", "/g", None, &roots),
            None,
            "no Epic id known"
        );
    }
}
