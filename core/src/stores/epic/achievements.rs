//! Epic achievements (legendary's `achievements` command): two GraphQL
//! queries on the launcher's store endpoint, the game's achievement
//! definitions and the player's records for the game's sandbox
//! (namespace), merged the way `core.get_achievements` does it.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::api::Epic;
use crate::error::Result;
use crate::types::{Achievement, Achievements};

const GAME_QUERY: &str = r#"query Achievement($sandboxId: String!, $locale: String!) {
  Achievement {
    productAchievementsRecordBySandbox(sandboxId: $sandboxId, locale: $locale) {
      sandboxId
      totalAchievements
      totalProductXP
      achievements {
        achievement {
          name
          hidden
          isBase
          unlockedDisplayName
          lockedDisplayName
          unlockedDescription
          lockedDescription
          XP
          unlockedIconLink
          lockedIconLink
          tier { name hexColor min max }
          rarity { percent }
        }
      }
    }
  }
}"#;

const USER_QUERY: &str = r#"query PlayerAchievement($epicAccountId: String!, $sandboxId: String!) {
  PlayerAchievement {
    playerAchievementGameRecordsBySandbox(epicAccountId: $epicAccountId, sandboxId: $sandboxId) {
      records {
        totalXP
        totalUnlocked
        playerAchievements {
          playerAchievement {
            unlocked
            progress
            XP
            unlockDate
            achievementName
          }
        }
      }
    }
  }
}"#;

/// The game's achievements with the account's progress; empty when the
/// game has none.
pub async fn fetch(
    epic: &Epic,
    namespace: &str,
    account_id: &str,
    lang: &str,
) -> Result<Achievements> {
    let game = epic
        .graphql(
            GAME_QUERY,
            json!({ "sandboxId": namespace, "locale": lang }),
        )
        .await?;
    let Some(record) = game
        .pointer("/data/Achievement/productAchievementsRecordBySandbox")
        .filter(|v| !v.is_null())
    else {
        return Ok(Achievements::default());
    };
    if record
        .get("totalAchievements")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        == 0
    {
        return Ok(Achievements::default());
    }
    let user = match epic
        .graphql(
            USER_QUERY,
            json!({ "epicAccountId": account_id, "sandboxId": namespace }),
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(namespace, error = %e, "Epic player achievements unavailable; showing them all locked");
            Value::Null
        }
    };
    Ok(merge(record, &user))
}

/// legendary's `get_achievements`: per achievement the locked or unlocked
/// texts and icon, unlocked ones first.
pub fn merge(record: &Value, user: &Value) -> Achievements {
    let unlocked: HashMap<&str, &Value> = user
        .pointer("/data/PlayerAchievement/playerAchievementGameRecordsBySandbox/records/0/playerAchievements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let pa = p.get("playerAchievement")?;
            Some((pa.get("achievementName")?.as_str()?, pa))
        })
        .collect();
    let mut list: Vec<Achievement> = record
        .get("achievements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let a = item.get("achievement")?;
            let name = a.get("name")?.as_str()?;
            let s = |k: &str| {
                a.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let player = unlocked.get(name);
            let is_unlocked =
                player.is_some_and(|p| p.get("unlocked").and_then(Value::as_bool) == Some(true));
            let unlocked_at = player
                .and_then(|p| p.get("unlockDate").and_then(Value::as_str))
                .filter(|d| !d.is_empty() && *d != "N/A")
                .map(str::to_owned)
                .filter(|_| is_unlocked);
            Some(Achievement {
                id: name.to_string(),
                name: if is_unlocked {
                    s("unlockedDisplayName")
                } else {
                    s("lockedDisplayName")
                },
                description: if is_unlocked {
                    s("unlockedDescription")
                } else {
                    s("lockedDescription")
                },
                icon_unlocked: s("unlockedIconLink"),
                icon_locked: s("lockedIconLink"),
                rarity: a
                    .pointer("/rarity/percent")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32,
                rarity_label: a
                    .pointer("/tier/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                hidden: !is_unlocked && a.get("hidden").and_then(Value::as_bool) == Some(true),
                unlocked: is_unlocked,
                unlocked_at,
            })
        })
        .collect();
    crate::stores::gog::achievements::sort(&mut list);
    let count = list.iter().filter(|a| a.unlocked).count();
    Achievements {
        total: list.len(),
        unlocked: count,
        list,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_player_records_into_the_definitions() {
        let record = json!({ "totalAchievements": 2, "achievements": [
            { "achievement": { "name": "A", "hidden": true, "unlockedDisplayName": "Got it", "lockedDisplayName": "???", "unlockedDescription": "u", "lockedDescription": "l",
                               "unlockedIconLink": "ui", "lockedIconLink": "li", "tier": { "name": "Bronze" }, "rarity": { "percent": 40.5 } } },
            { "achievement": { "name": "B", "hidden": true, "unlockedDisplayName": "Secret", "lockedDisplayName": "Hidden", "unlockedDescription": "u", "lockedDescription": "l",
                               "unlockedIconLink": "ui", "lockedIconLink": "li", "tier": { "name": "Gold" }, "rarity": { "percent": 2.0 } } }
        ] });
        let user = json!({ "data": { "PlayerAchievement": { "playerAchievementGameRecordsBySandbox": { "records": [ { "totalUnlocked": 1, "playerAchievements": [
            { "playerAchievement": { "achievementName": "A", "unlocked": true, "progress": 1.0, "unlockDate": "2026-10-01T10:00:00.000Z" } }
        ] } ] } } } });
        let a = merge(&record, &user);
        assert_eq!((a.total, a.unlocked), (2, 1));
        assert_eq!(a.list[0].name, "Got it");
        assert!(a.list[0].unlocked && !a.list[0].hidden);
        assert_eq!(
            a.list[0].unlocked_at.as_deref(),
            Some("2026-10-01T10:00:00.000Z")
        );
        assert_eq!(a.list[1].name, "Hidden");
        assert!(a.list[1].hidden && !a.list[1].unlocked);
        assert_eq!(a.list[1].rarity_label, "Gold");
        let none = merge(&record, &Value::Null);
        assert_eq!(none.unlocked, 0);
    }
}
