use std::path::{Path, PathBuf};

use super::Platform;

pub struct Windows;

impl Platform for Windows {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn default_install_path(&self) -> PathBuf {
        // Default: %USERPROFILE%\Games\Hyper (original Heroic used Games\Heroic)
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("C:\\"))
            .join("Games")
            .join("Hyper")
    }

    fn webviews_share_browser_process(&self) -> bool {
        true
    }

    fn needs_wine(&self) -> bool {
        false
    }

    fn exe_icon_png(&self, exe: &Path) -> Option<Vec<u8>> {
        let (w, h, rgba) = icon::extract(exe, 256)?;
        icon::encode_png(w, h, &rgba)
    }

    /// Drive letters in use (A: and B:, floppy letters, are skipped: probing
    /// an empty floppy drive can hang).
    fn roots(&self) -> Vec<PathBuf> {
        (b'C'..=b'Z')
            .map(|l| PathBuf::from(format!("{}:\\", l as char)))
            .filter(|p| p.is_dir())
            .collect()
    }

    fn is_hidden(&self, path: &Path) -> bool {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        std::fs::metadata(path).is_ok_and(|m| m.file_attributes() & (HIDDEN | SYSTEM) != 0)
    }
}

mod icon {
    //! First icon of an executable, read through GDI as 32-bit RGBA.

    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW,
    };

    pub fn extract(exe: &Path, size: i32) -> Option<(u32, u32, Vec<u8>)> {
        let mut name = [0u16; 260];
        let wide: Vec<u16> = exe.as_os_str().encode_wide().collect();
        if wide.len() >= name.len() {
            return None;
        }
        name[..wide.len()].copy_from_slice(&wide);

        let mut icons = [HICON::default()];
        let got = unsafe { PrivateExtractIconsW(&name, 0, size, size, Some(&mut icons), None, 0) };
        if got == 0 || got == u32::MAX || icons[0].is_invalid() {
            return None;
        }
        let out = unsafe { pixels(icons[0]) };
        unsafe {
            let _ = DestroyIcon(icons[0]);
        }
        out
    }

    unsafe fn pixels(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
        let mut info = ICONINFO::default();
        unsafe { GetIconInfo(icon, &mut info) }.ok()?;
        let result = unsafe { read_bitmap(info.hbmColor, info.hbmMask) };
        unsafe {
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        }
        result
    }

    unsafe fn read_bitmap(color: HBITMAP, mask: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
        if color.is_invalid() {
            return None;
        }
        let mut bm = BITMAP::default();
        let n = unsafe {
            GetObjectW(
                HGDIOBJ(color.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bm as *mut _ as *mut _),
            )
        };
        if n == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let dc = unsafe { CreateCompatibleDC(None) };
        let read = |bmp: HBITMAP| -> Option<Vec<u8>> {
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let lines = unsafe {
                GetDIBits(
                    dc,
                    bmp,
                    0,
                    h as u32,
                    Some(buf.as_mut_ptr() as *mut _),
                    &mut bi,
                    DIB_RGB_COLORS,
                )
            };
            (lines > 0).then_some(buf)
        };
        let colour = read(color);
        // Old icons carry no alpha: transparency comes from the AND mask.
        let mask_px = if colour
            .as_ref()
            .is_some_and(|c| c.chunks(4).all(|p| p[3] == 0))
            && !mask.is_invalid()
        {
            read(mask)
        } else {
            None
        };
        unsafe {
            let _ = DeleteDC(dc);
        }
        let mut px = colour?;
        let no_alpha = px.chunks(4).all(|p| p[3] == 0);
        for (i, p) in px.chunks_mut(4).enumerate() {
            p.swap(0, 2); // BGRA -> RGBA
            if no_alpha {
                let transparent = mask_px.as_ref().is_some_and(|m| m[i * 4] != 0);
                p[3] = if transparent { 0 } else { 255 };
            }
        }
        Some((w as u32, h as u32, px))
    }

    pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().ok()?;
            writer.write_image_data(rgba).ok()?;
        }
        Some(out)
    }
}

/// Start a program elevated (the UAC prompt: `ShellExecuteEx` with the
/// `runas` verb, what Heroic does with `Start-Process -Verb RunAs`) and wait
/// for it; returns its exit code. For GOG's setup scripts and redistributable
/// installers, which write to Program Files and the registry. `params` is
/// the already quoted command line (`gog::setup::windows_command_line`). A
/// declined prompt is an error.
///
/// `cancelled` is polled while the program runs: once it says so, the
/// program is terminated if Windows lets us (the handle from an elevated
/// start may lack the right) and the wait ends either way with
/// `ErrorKind::Interrupted`, so a hung installer never holds the game in
/// "Installing".
pub fn run_elevated(
    exe: &Path,
    params: &str,
    cwd: &Path,
    cancelled: &dyn Fn() -> bool,
) -> std::io::Result<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, TerminateProcess, WaitForSingleObject,
    };
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let wide =
        |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain(std::iter::once(0)).collect() };
    let verb = wide(std::ffi::OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    let params = wide(std::ffi::OsStr::new(params));
    let dir = wide(cwd.as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        lpDirectory: PCWSTR(dir.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the wide strings outlive the call and `info` is a complete,
    // correctly sized structure
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| {
        std::io::Error::other(format!("could not start {} elevated: {e}", exe.display()))
    })?;
    let process = info.hProcess;
    if process.is_invalid() {
        return Err(std::io::Error::other(format!(
            "{}: no process handle",
            exe.display()
        )));
    }
    // SAFETY: a valid process handle we own; it is closed once below
    let code = unsafe {
        while WaitForSingleObject(process, 500) == WAIT_TIMEOUT {
            if cancelled() {
                if let Err(e) = TerminateProcess(process, 1) {
                    tracing::warn!(exe = %exe.display(), error = %e, "elevated program left running");
                }
                let _ = CloseHandle(process);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    format!("{}: cancelled", exe.display()),
                ));
            }
        }
        let mut code = 0u32;
        let got = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        got.map(|()| code)
    };
    code.map_err(|e| std::io::Error::other(format!("{}: exit code unknown: {e}", exe.display())))
}

/// Open a URI with the program registered for its scheme (`link2ea://`,
/// `uplay://`: the launcher of a third-party Epic title), without waiting.
pub fn open_uri(uri: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let verb = wide("open");
    let file = wide(uri);
    // SAFETY: the wide strings outlive the call; null window and parameters are allowed
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a pseudo-HINSTANCE: values above 32 mean success
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "ShellExecute failed with code {}",
            result.0 as usize
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_explorer_icon_as_png() {
        let exe = Path::new(r"C:\Windows\explorer.exe");
        if !exe.exists() {
            return;
        }
        let png = Windows.exe_icon_png(exe).expect("explorer.exe has an icon");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(
            png.len() > 1000,
            "suspiciously small icon: {} bytes",
            png.len()
        );
    }

    #[test]
    fn missing_file_has_no_icon() {
        assert!(
            Windows
                .exe_icon_png(Path::new(r"C:\definitely\not\here.exe"))
                .is_none()
        );
    }
}
