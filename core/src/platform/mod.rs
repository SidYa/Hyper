//! OS-specific behaviour: the only place that branches on the OS. Windows
//! runs games natively; Linux runs Windows games through Proton (see
//! `compat`), which `needs_wine` tells the rest of the crate.

use std::path::{Path, PathBuf};

pub mod child_env;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{open_uri, run_elevated};

pub trait Platform: Send + Sync {
    /// Short identifier used in logs and sent to the frontend.
    fn name(&self) -> &'static str;
    /// Where games are installed when the user has not chosen a folder.
    fn default_install_path(&self) -> PathBuf;
    /// Whether launching Windows games requires a compatibility layer.
    fn needs_wine(&self) -> bool;
    /// The executable's icon as a PNG (largest size available), for the
    /// generated card of a sideloaded game.
    fn exe_icon_png(&self, _exe: &Path) -> Option<Vec<u8>> {
        None
    }
    /// Top-level folders to browse from (drives on Windows).
    fn roots(&self) -> Vec<PathBuf>;
    /// Hidden or system file / folder.
    fn is_hidden(&self, _path: &Path) -> bool {
        false
    }
    /// Process-wide setup that must happen before the webview starts.
    fn init_process(&self) {}
    /// Webviews on one browser profile share one browser process that quits
    /// with its last webview (WebView2), so a store's login window must not
    /// close before its Store tab webview exists.
    fn webviews_share_browser_process(&self) -> bool {
        false
    }
}

pub fn current() -> &'static dyn Platform {
    #[cfg(target_os = "windows")]
    {
        &windows::Windows
    }
    #[cfg(target_os = "linux")]
    {
        &linux::Linux
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        compile_error!("Hyper currently supports only Windows and Linux")
    }
}
