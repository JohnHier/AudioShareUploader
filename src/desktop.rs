//! Running as an ordinary program under the signed-in Windows user (since
//! 0.3.0): it then sees mapped drives (Y:) and the NAS exactly like that user,
//! needs no admin rights and installs with a double-click.
//!
//! - Double-click: start in the background, start with Windows from now on
//!   (HKCU\...\Run, only this user), open the web interface.
//! - Double-click while it runs: just open the web interface.
//! - Autostart launches `AudioShareUploader.exe background`.

use std::process::Command;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "AudioShareUploader";

pub fn port_in_use(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_err()
}

pub fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

/// Start a second copy of this program in the background, without a window.
pub fn spawn_background() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut c = Command::new(exe);
    c.arg("background");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0000_0008 | 0x0800_0000); // DETACHED_PROCESS | CREATE_NO_WINDOW
    }
    c.spawn().map(|_| ()).map_err(|e| format!("Starten op de achtergrond mislukt: {e}"))
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Start with Windows (for this user) on or off.
pub fn set_autostart(on: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};
        let (key, val) = (wide(RUN_KEY), wide(RUN_VALUE));
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let cmd = wide(&format!("\"{}\" background", exe.display()));
            let rc = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), REG_SZ, cmd.as_ptr().cast(), (cmd.len() * 2) as u32) };
            if rc != 0 {
                return Err(format!("Automatisch starten instellen mislukt (Windows-fout {rc})"));
            }
        } else {
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr()) };
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = on;
        Ok(())
    }
}

pub fn autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
        let (key, val) = (wide(RUN_KEY), wide(RUN_VALUE));
        let mut size = 0u32;
        let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
        rc == 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn message(title: &str, text: &str, warning: bool) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MessageBoxW};
        let (t, c) = (wide(text), wide(title));
        unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | if warning { MB_ICONWARNING } else { MB_ICONINFORMATION }) };
    }
    #[cfg(not(windows))]
    {
        let _ = warning;
        println!("{title}: {text}");
    }
}

/// For command-line use: write to the console we were started from.
pub fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

/// Is the old service installation (0.1/0.2) still there?
pub fn service_installed() -> bool {
    #[cfg(windows)]
    {
        use windows_service::service::ServiceAccess;
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .ok()
            .and_then(|m| m.open_service("AudioShareUploader", ServiceAccess::QUERY_STATUS).ok())
            .is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
