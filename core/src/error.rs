use serde::Serialize;

/// Error type returned from every command. Serialised to the UI as
/// `{ kind, message }` so the frontend can branch on `kind`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("not logged in to {0}")]
    NotLoggedIn(String),
    /// A program we run on the user's behalf (a setup script, an installer)
    /// failed. The UI knows the kinds `helper_missing` and `helper`; the
    /// former is no longer produced (no helper binaries exist any more).
    #[error("{helper} failed: {message}")]
    Helper { helper: String, message: String },
    #[error("game not found: {0}")]
    GameNotFound(String),
    #[error("{0} is busy")]
    Busy(String),
    #[error("cancelled")]
    Cancelled,
    #[error("secret storage: {0}")]
    Secrets(String),
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Other(format!("{e:#}"))
    }
}

impl Error {
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Io(_) => "io",
            Error::Json(_) => "json",
            Error::NotLoggedIn(_) => "not_logged_in",
            Error::Helper { .. } => "helper",
            Error::GameNotFound(_) => "game_not_found",
            Error::Busy(_) => "busy",
            Error::Cancelled => "cancelled",
            Error::Secrets(_) => "secrets",
            Error::NotImplemented(_) => "not_implemented",
            Error::Other(_) => "other",
        }
    }
}

#[derive(Serialize)]
struct Serialized<'a> {
    kind: &'a str,
    message: String,
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        Serialized {
            kind: self.kind(),
            message: self.to_string(),
        }
        .serialize(s)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
