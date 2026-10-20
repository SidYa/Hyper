//! `hyper-core` — everything that does not need a window.
//!
//! - `types`     — data shapes shared with the UI (`app/src/types/index.ts`)
//! - `config`    — application settings persisted as JSON
//! - `paths`     — well-known directories
//! - `platform`  — OS-specific behaviour (Windows native, Linux through Proton)
//! - `process`   — terminating a process tree we started
//! - `compat`    — Proton through the Steam Linux Runtime (our umu port; Linux)
//! - `secrets`   — encrypted vault for store credentials (DPAPI on Windows, AES-GCM on Linux)
//! - `events`    — events the core pushes to whoever hosts it (the Tauri app)
//! - `stores`    — store managers: Epic and GOG (native ports of legendary / gogdl), sideloaded games
//! - `downloads` — sequential install/update queue
//! - `games`     — running-game tracker (launch, kill)
//! - `net`       — whether a store is reachable (offline launch / saves)
//! - `browse`    — folder listing for the console-mode folder picker
//! - `playtime`  — play time per game (`playtime.json`)
//! - `saves`     — the checks around a cloud-save sync
//! - `error`     — the single error type returned to the UI

pub mod browse;
pub mod compat;
pub mod config;
pub mod downloads;
pub mod error;
pub mod events;
pub mod games;
pub mod net;
pub mod paths;
pub mod platform;
pub mod playtime;
pub mod process;
pub mod saves;
pub mod secrets;
pub mod stores;
pub mod types;

pub use error::{Error, Result};
