//! Linux: Windows games run through Proton inside the Steam Linux Runtime
//! (`compat::umu`), so `needs_wine` is true. Folder browsing starts at `/`
//! and the home folder.

use std::path::PathBuf;

use super::Platform;

pub struct Linux;

impl Platform for Linux {
    fn name(&self) -> &'static str {
        "linux"
    }

    fn default_install_path(&self) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join("Games")
            .join("Hyper")
    }

    fn needs_wine(&self) -> bool {
        true
    }

    fn roots(&self) -> Vec<PathBuf> {
        let mut out = vec![PathBuf::from("/")];
        out.extend(dirs::home_dir());
        out
    }

    fn init_process(&self) {
        // WebKitGTK's DMA-BUF renderer shows a blank window under gamescope
        // (Steam Deck game mode) and on some GPU drivers.
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            // SAFETY: called first thing in `run()`, before any other thread exists
            unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
        }
    }
}
