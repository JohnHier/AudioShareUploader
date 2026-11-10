//! Network folders (NAS). The service runs as Local System, which can't see a
//! user's mapped drives (Y:) nor log on to a NAS by itself. Here the admin
//! enters a share once (\\NASBR6\Mediafiles_Playout, user, password, and
//! optionally the drive letter people know it by). The uploader then:
//! - translates Y:\... into \\NASBR6\Mediafiles_Playout\...
//! - logs on to the share with those credentials before reading.
//! The password is kept in Windows Credential Manager (encrypted by Windows).

use crate::config::{Settings, Share};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

static CONNECTED: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);

fn key(unc: &str) -> String {
    format!("share:{}", unc.to_ascii_lowercase())
}

pub fn normalize_unc(s: &str) -> String {
    let s = s.trim().trim_matches('"').replace('/', "\\");
    let s = s.trim_end_matches('\\').to_string();
    if s.starts_with("\\\\") { s } else { format!("\\\\{}", s.trim_start_matches('\\')) }
}

pub fn normalize_letter(s: &str) -> String {
    let c = s.trim().chars().next().map(|c| c.to_ascii_uppercase());
    match c {
        Some(c) if c.is_ascii_alphabetic() => format!("{c}:"),
        _ => String::new(),
    }
}

pub fn save_password(unc: &str, pw: &str) -> Result<(), String> {
    crate::audioshare::secure::store_token(&key(unc), pw)
}

pub fn forget(unc: &str) {
    crate::audioshare::secure::delete_token(&key(unc));
    if let Some(m) = CONNECTED.lock().unwrap().as_mut() {
        m.remove(&unc.to_ascii_lowercase());
    }
}

/// The share a UNC path lies on (longest match).
fn share_for<'a>(shares: &'a [Share], unc_path: &str) -> Option<&'a Share> {
    let p = unc_path.to_ascii_lowercase();
    shares
        .iter()
        .filter(|s| {
            let u = s.unc.to_ascii_lowercase();
            p == u || p.starts_with(&format!("{u}\\"))
        })
        .max_by_key(|s| s.unc.len())
}

/// Y:\map\file → \\server\share\map\file when Y: is a known share.
pub fn translate(path: &str, shares: &[Share]) -> String {
    let p = path.trim().trim_matches('"');
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        let letter = normalize_letter(&p[..1]);
        if let Some(s) = shares.iter().find(|s| s.letter == letter) {
            let rest = p[2..].trim_start_matches(['\\', '/']);
            return if rest.is_empty() { s.unc.clone() } else { format!("{}\\{}", s.unc, rest.replace('/', "\\")) };
        }
    }
    p.to_string()
}

/// Translate the path and make sure its share is logged on. Returns the path to read.
pub fn prepare(path: &str) -> Result<PathBuf, String> {
    let shares = Settings::load().shares;
    let p = translate(path, &shares);
    if p.starts_with("\\\\") {
        if let Some(s) = share_for(&shares, &p) {
            connect(s, false)?;
        }
    } else {
        let b = p.as_bytes();
        if cfg!(windows) && b.len() >= 2 && b[1] == b':' && !std::path::Path::new(&p[..2]).join("\\").exists() {
            let l = normalize_letter(&p[..1]);
            return Err(format!(
                "Schijf {l} is voor de uploader niet zichtbaar (een netwerkschijf van een gebruiker). Voeg hem toe onder Instellingen → Netwerkmappen, met het netwerkpad erachter (bijv. {l} = \\\\NAS\\share)."
            ));
        }
    }
    Ok(PathBuf::from(p))
}

/// Log on to the share (again at most every 10 minutes, or when `force`).
pub fn connect(s: &Share, force: bool) -> Result<(), String> {
    let now = crate::audioshare::queue::now();
    let k = s.unc.to_ascii_lowercase();
    {
        let mut g = CONNECTED.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        if !force && m.get(&k).is_some_and(|t| now - t < 600) {
            return Ok(());
        }
    }
    let pw = crate::audioshare::secure::load_token(&key(&s.unc));
    if s.user.trim().is_empty() {
        return Ok(()); // no credentials: rely on the service account's own access
    }
    let pw = pw.ok_or_else(|| format!("Geen wachtwoord bewaard voor {}. Vul het opnieuw in onder Instellingen → Netwerkmappen.", s.unc))?;
    os_connect(&s.unc, &s.user, &pw)?;
    CONNECTED.lock().unwrap().get_or_insert_with(HashMap::new).insert(k, now);
    Ok(())
}

#[cfg(windows)]
fn os_connect(unc: &str, user: &str, pw: &str) -> Result<(), String> {
    use windows_sys::Win32::NetworkManagement::WNet::{CONNECT_TEMPORARY, NETRESOURCEW, RESOURCETYPE_DISK, WNetAddConnection2W, WNetCancelConnection2W};
    let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let mut remote = w(unc);
    let (wpw, wuser) = (w(pw), w(user));
    let nr = NETRESOURCEW { dwType: RESOURCETYPE_DISK, lpRemoteName: remote.as_mut_ptr(), ..Default::default() };
    let mut rc = unsafe { WNetAddConnection2W(&nr, wpw.as_ptr(), wuser.as_ptr(), CONNECT_TEMPORARY) };
    if rc == 1219 {
        // Already connected with other credentials: drop that connection and retry.
        unsafe { WNetCancelConnection2W(remote.as_ptr(), 0, 1) };
        rc = unsafe { WNetAddConnection2W(&nr, wpw.as_ptr(), wuser.as_ptr(), CONNECT_TEMPORARY) };
    }
    match rc {
        0 => Ok(()),
        86 | 1326 => Err(format!("Inloggen op {unc} mislukt: onjuiste gebruikersnaam of wachtwoord")),
        53 | 67 => Err(format!("{unc} niet gevonden: controleer de naam van de NAS en van de gedeelde map")),
        5 => Err(format!("Geen toegang tot {unc} met deze gebruiker")),
        1909 => Err(format!("Het account voor {unc} is geblokkeerd op de NAS")),
        e => Err(format!("Verbinden met {unc} mislukt (Windows-fout {e})")),
    }
}

#[cfg(not(windows))]
fn os_connect(_unc: &str, _user: &str, _pw: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_drive_letters() {
        let shares = vec![Share { letter: "Y:".into(), unc: r"\\NASBR6\Mediafiles_Playout".into(), user: "u".into() }];
        assert_eq!(translate(r"Y:\muziek\Bob.mp3", &shares), r"\\NASBR6\Mediafiles_Playout\muziek\Bob.mp3");
        assert_eq!(translate(r"y:/muziek/Bob.mp3", &shares), r"\\NASBR6\Mediafiles_Playout\muziek\Bob.mp3");
        assert_eq!(translate(r"Y:\", &shares), r"\\NASBR6\Mediafiles_Playout");
        assert_eq!(translate(r"D:\x.mp3", &shares), r"D:\x.mp3");
        assert_eq!(translate(r"\\NASBR6\Mediafiles_Playout\a.mp3", &shares), r"\\NASBR6\Mediafiles_Playout\a.mp3");
        assert_eq!(share_for(&shares, r"\\nasbr6\mediafiles_playout\muziek\a.mp3").unwrap().letter, "Y:");
        assert!(share_for(&shares, r"\\nasbr6\mediafiles_playoutX\a.mp3").is_none());
        assert_eq!(normalize_unc("NASBR6/Media/"), r"\\NASBR6\Media");
        assert_eq!(normalize_letter("y"), "Y:");
    }
}
