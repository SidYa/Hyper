//! GOG OAuth tokens, replacing `gogdl auth`. The vault entry keeps gogdl's
//! `auth.json` layout — one object per OAuth client id: the Galaxy client
//! (`CLIENT_ID`, written at login by `Gog::login`) and, for cloud saves, a
//! game's own client — each with `access_token`, `refresh_token`,
//! `expires_in`, `user_id` and gogdl's `loginTime` (seconds since the
//! epoch). A token is refreshed when `loginTime + expires_in` has passed;
//! game-scoped tokens are minted from the Galaxy client's refresh token
//! with `without_new_session=1`, as gogdl does.

use std::sync::Arc;

use serde_json::{Map, Value};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use super::super::CLIENT_ID;
use super::super::api::{CLIENT_SECRET, Credentials};
use super::{net, now_secs as now};
use crate::error::{Error, Result};
use crate::secrets::SecretFile;

pub struct Auth {
    user: Arc<SecretFile>,
    http: reqwest::Client,
    /// One refresh at a time; parallel callers reuse its result.
    refresh: Mutex<()>,
}

fn expired(entry: &Value) -> bool {
    let login = entry
        .get("loginTime")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let ttl = entry
        .get("expires_in")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    now() >= login + ttl
}

fn credentials(entry: &Value) -> Result<Credentials> {
    let s = |k: &str| {
        entry
            .get(k)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    match (s("access_token"), s("user_id")) {
        (Some(access_token), Some(user_id)) => Ok(Credentials {
            access_token,
            user_id,
        }),
        _ => Err(Error::NotLoggedIn("GOG".into())),
    }
}

impl Auth {
    pub fn new(user: Arc<SecretFile>, http: reqwest::Client) -> Self {
        Self {
            user,
            http,
            refresh: Mutex::new(()),
        }
    }

    pub fn is_logged_in(&self) -> bool {
        self.user.exists()
    }

    fn read(&self) -> Result<Map<String, Value>> {
        let raw = self
            .user
            .read()?
            .ok_or_else(|| Error::NotLoggedIn("GOG".into()))?;
        match serde_json::from_slice::<Value>(&raw) {
            Ok(Value::Object(map)) => Ok(map),
            _ => Err(Error::NotLoggedIn("GOG".into())),
        }
    }

    /// The Galaxy client's token (library, builds, downloads).
    pub async fn credentials(&self) -> Result<Credentials> {
        self.credentials_for(CLIENT_ID, CLIENT_SECRET).await
    }

    /// A token for another OAuth client (a game's, for its cloud storage),
    /// refreshed or newly minted when needed.
    pub async fn credentials_for(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Credentials> {
        let _one_at_a_time = self.refresh.lock().await;
        // the generation before the read: a logout meanwhile makes the
        // write-back below fail instead of restoring the login
        let generation = self.user.generation();
        let mut all = self.read()?;
        if let Some(entry) = all.get(client_id)
            && !expired(entry)
        {
            return credentials(entry);
        }
        let refresh_token = all
            .get(CLIENT_ID)
            .and_then(|e| e.get("refresh_token"))
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| Error::NotLoggedIn("GOG".into()))?
            .to_string();
        let game_scope = client_id != CLIENT_ID;
        let mut params = vec![
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
        ];
        if game_scope {
            params.push(("without_new_session", "1"));
        }
        let url = reqwest::Url::parse_with_params("https://auth.gog.com/token", &params)
            .map_err(|e| Error::Other(e.to_string()))?;
        let resp = self.http.get(url).send().await.map_err(net)?;
        let status = resp.status();
        let body = Zeroizing::new(resp.bytes().await.map_err(net)?.to_vec());
        let json: Value = serde_json::from_slice(&body)
            .map_err(|_| Error::Other(format!("GOG token refresh: HTTP {status}")))?;
        if !status.is_success() || json.get("access_token").is_none() {
            let err = json.get("error").and_then(Value::as_str).unwrap_or("");
            if !game_scope && status.is_client_error() && !err.is_empty() {
                // the stored login is gone for good (revoked, expired refresh token)
                tracing::warn!(error = err, "GOG refused the stored login; signed out");
                self.user.revoke().await?;
                return Err(Error::NotLoggedIn("GOG".into()));
            }
            return Err(Error::Other(
                format!("GOG token refresh: HTTP {status} {err}")
                    .trim_end()
                    .to_string(),
            ));
        }
        let mut entry = json;
        entry["loginTime"] = now().into();
        let out = credentials(&entry)?;
        all.insert(client_id.to_string(), entry);
        self.user
            .update(
                generation,
                &Zeroizing::new(serde_json::to_vec(&Value::Object(all))?),
            )
            .await?;
        tracing::debug!(game_scope, "GOG token refreshed");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_follows_login_time() {
        let fresh = serde_json::json!({ "loginTime": now(), "expires_in": 3600 });
        assert!(!expired(&fresh));
        let old = serde_json::json!({ "loginTime": now() - 4000.0, "expires_in": 3600 });
        assert!(expired(&old));
        assert!(expired(&serde_json::json!({})));
    }

    #[test]
    fn credentials_need_token_and_user() {
        assert!(credentials(&serde_json::json!({ "access_token": "t", "user_id": "u" })).is_ok());
        assert!(credentials(&serde_json::json!({ "access_token": "t" })).is_err());
    }
}
