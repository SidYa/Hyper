//! GOG achievements (Heroic's `GOGGame.getAchievements`):
//! `gameplay.gog.com/clients/<client>/users/<user>/achievements`, the
//! Galaxy client id of the game and the account's user id, in the UI's
//! language (`X-Gog-Lc`).

use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::{Achievement, Achievements};

pub async fn fetch(
    http: &reqwest::Client,
    token: &str,
    user_id: &str,
    client_id: &str,
    lang: &str,
) -> Result<Achievements> {
    let safe = |s: &str| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
    };
    let url = format!(
        "https://gameplay.gog.com/clients/{}/users/{}/achievements",
        safe(client_id),
        safe(user_id)
    );
    let resp = http
        .get(url)
        .bearer_auth(token)
        .header("X-Gog-Lc", lang)
        .send()
        .await
        .map_err(|e| Error::Other(format!("GOG achievements: {}", e.without_url())))?;
    if !resp.status().is_success() {
        return Err(Error::Other(format!(
            "GOG achievements: HTTP {}",
            resp.status()
        )));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| Error::Other(format!("GOG achievements: {}", e.without_url())))?;
    Ok(parse(&v))
}

/// `items[]` → ours; unlocked ones first (newest first), then locked by rarity.
pub fn parse(v: &Value) -> Achievements {
    let mut list: Vec<Achievement> = v
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let s = |k: &str| {
                a.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let unlocked_at = a
                .get("date_unlocked")
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty())
                .map(str::to_owned);
            Some(Achievement {
                id: a.get("achievement_id").and_then(|x| {
                    x.as_str()
                        .map(str::to_owned)
                        .or_else(|| x.as_u64().map(|n| n.to_string()))
                })?,
                name: s("name"),
                description: s("description"),
                icon_unlocked: s("image_url_unlocked"),
                icon_locked: s("image_url_locked"),
                rarity: a.get("rarity").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                rarity_label: s("rarity_level_description"),
                hidden: a.get("visible").and_then(Value::as_bool) == Some(false)
                    && unlocked_at.is_none(),
                unlocked: unlocked_at.is_some(),
                unlocked_at,
            })
        })
        .collect();
    sort(&mut list);
    let unlocked = list.iter().filter(|a| a.unlocked).count();
    Achievements {
        total: list.len(),
        unlocked,
        list,
    }
}

/// Unlocked first, newest first; then locked, the common ones first (Heroic's order).
pub fn sort(list: &mut [Achievement]) {
    list.sort_by(|a, b| match (a.unlocked, b.unlocked) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (true, true) => b.unlocked_at.cmp(&a.unlocked_at),
        (false, false) => b
            .rarity
            .partial_cmp(&a.rarity)
            .unwrap_or(std::cmp::Ordering::Equal),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_orders_gog_achievements() {
        let v = serde_json::json!({ "total_count": 3, "items": [
            { "achievement_id": "1", "achievement_key": "a", "visible": true, "name": "Rare", "description": "d", "image_url_unlocked": "u1", "image_url_locked": "l1", "rarity": 3.5, "date_unlocked": null, "rarity_level_description": "Rare", "rarity_level_slug": "rare" },
            { "achievement_id": "2", "achievement_key": "b", "visible": false, "name": "Secret", "description": "d", "image_url_unlocked": "u2", "image_url_locked": "l2", "rarity": 60.0, "date_unlocked": null, "rarity_level_description": "Common", "rarity_level_slug": "common" },
            { "achievement_id": "3", "achievement_key": "c", "visible": true, "name": "Done", "description": "d", "image_url_unlocked": "u3", "image_url_locked": "l3", "rarity": 80.0, "date_unlocked": "2026-10-01T10:00:00+0300", "rarity_level_description": "Common", "rarity_level_slug": "common" }
        ] });
        let a = parse(&v);
        assert_eq!((a.total, a.unlocked), (3, 1));
        assert_eq!(
            a.list.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["3", "2", "1"]
        );
        assert!(a.list[0].unlocked && a.list[0].unlocked_at.is_some());
        assert!(a.list[1].hidden, "locked and invisible");
        assert!(!a.list[2].hidden);
        assert_eq!(parse(&serde_json::json!({})).total, 0);
    }
}
