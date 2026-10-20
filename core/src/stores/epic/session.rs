//! `user.json` as legendary keeps it: Epic's OAuth token response
//! (`access_token`, `refresh_token`, `expires_at`, `account_id`,
//! `displayName`, …). The two tokens live in zeroed strings; everything else
//! stays JSON so unknown fields survive a round trip.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

pub struct Session {
    pub access_token: Zeroizing<String>,
    pub refresh_token: Zeroizing<String>,
    rest: Map<String, Value>,
}

/// A session with less than this left is refreshed before use (legendary's
/// margin as well).
const MIN_LEFT: Duration = Duration::from_secs(600);

impl Session {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut rest: Map<String, Value> = serde_json::from_slice(bytes)?;
        // the string is moved out of the parsed value, not copied, so the
        // only heap copy of the token is the zeroized one
        let take = |rest: &mut Map<String, Value>, key: &str| {
            Zeroizing::new(match rest.remove(key) {
                Some(Value::String(s)) => s,
                _ => String::new(),
            })
        };
        let access_token = take(&mut rest, "access_token");
        let refresh_token = take(&mut rest, "refresh_token");
        if refresh_token.is_empty() {
            return Err(Error::NotLoggedIn("Epic Games".into()));
        }
        Ok(Self {
            access_token,
            refresh_token,
            rest,
        })
    }

    /// The file as legendary expects it.
    pub fn to_bytes(&self) -> Result<Zeroizing<Vec<u8>>> {
        let mut map = self.rest.clone();
        map.insert(
            "access_token".into(),
            Value::String(self.access_token.to_string()),
        );
        map.insert(
            "refresh_token".into(),
            Value::String(self.refresh_token.to_string()),
        );
        let bytes = Zeroizing::new(serde_json::to_vec(&Value::Object(map))?);
        Ok(bytes)
    }

    /// A fresh token response (or `verify` answer) replaces what it names.
    pub fn merge(&mut self, response: Map<String, Value>) {
        for (k, v) in response {
            match k.as_str() {
                "access_token" => {
                    self.access_token = Zeroizing::new(v.as_str().unwrap_or_default().to_owned())
                }
                "refresh_token" => {
                    self.refresh_token = Zeroizing::new(v.as_str().unwrap_or_default().to_owned())
                }
                _ => {
                    self.rest.insert(k, v);
                }
            }
        }
    }

    fn field(&self, key: &str) -> Option<&str> {
        self.rest.get(key).and_then(Value::as_str)
    }

    pub fn account_id(&self) -> Option<&str> {
        self.field("account_id")
    }

    pub fn display_name(&self) -> Option<&str> {
        self.field("displayName")
    }

    /// The access token is good for a while yet (`expires_at`, ISO 8601 UTC).
    pub fn access_token_usable(&self, now: DateTime<Utc>) -> bool {
        if self.access_token.is_empty() {
            return false;
        }
        let Some(exp) = self
            .field("expires_at")
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        else {
            return false;
        };
        exp.signed_duration_since(now)
            .to_std()
            .is_ok_and(|left| left > MIN_LEFT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = r#"{"access_token":"eg1~acc","expires_in":28800,"expires_at":"2030-01-01T10:00:00.000Z","token_type":"bearer","refresh_token":"eg1~ref","refresh_expires":1987200,"refresh_expires_at":"2030-01-24T02:00:00.000Z","account_id":"abc123","client_id":"34a02cf8f4414e29b15921876da36f9a","internal_client":true,"client_service":"launcher","displayName":"Player","app":"launcher","in_app_id":"abc123"}"#;

    #[test]
    fn round_trips_and_reads_fields() {
        let s = Session::parse(USER.as_bytes()).unwrap();
        assert_eq!(s.access_token.as_str(), "eg1~acc");
        assert_eq!(s.account_id(), Some("abc123"));
        assert_eq!(s.display_name(), Some("Player"));
        let back: Value = serde_json::from_slice(&s.to_bytes().unwrap()).unwrap();
        assert_eq!(back, serde_json::from_str::<Value>(USER).unwrap());
    }

    #[test]
    fn expiry_margin() {
        let s = Session::parse(USER.as_bytes()).unwrap();
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        assert!(s.access_token_usable(t("2030-01-01T09:00:00Z")));
        assert!(
            !s.access_token_usable(t("2030-01-01T09:55:00Z")),
            "less than 10 minutes left"
        );
        assert!(!s.access_token_usable(t("2030-01-02T00:00:00Z")));
    }

    #[test]
    fn merge_replaces_tokens() {
        let mut s = Session::parse(USER.as_bytes()).unwrap();
        s.merge(serde_json::from_str(r#"{"access_token":"eg1~new","refresh_token":"eg1~ref2","expires_at":"2031-01-01T00:00:00.000Z"}"#).unwrap());
        assert_eq!(s.access_token.as_str(), "eg1~new");
        assert_eq!(s.refresh_token.as_str(), "eg1~ref2");
        assert_eq!(s.field("expires_at"), Some("2031-01-01T00:00:00.000Z"));
        assert!(
            Session::parse(br#"{"account_id":"x"}"#).is_err(),
            "no refresh token = not logged in"
        );
    }
}
