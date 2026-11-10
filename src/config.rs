//! Settings and tasks, stored as JSON in the data folder.
//!
//! Windows: `C:\ProgramData\AudioShareUploader\`. Elsewhere (development,
//! tests): `$AUSU_DATA`, or `./data`.

use crate::audioshare::api::Program;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("AUSU_DATA") {
        return PathBuf::from(d);
    }
    #[cfg(windows)]
    {
        let base = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
        PathBuf::from(base).join("AudioShareUploader")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("data")
    }
}

/// Write atomically: a crash never leaves a half file behind.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string_pretty(value) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub name: String,
    /// Argon2id PHC string.
    pub hash: String,
}

/// A network folder the uploader logs on to itself (password in Credential Manager).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Share {
    /// Drive letter people use for it, e.g. "Y:" (optional).
    pub letter: String,
    /// \\server\share
    pub unc: String,
    pub user: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub port: u16,
    /// Allow the web interface from outside the local network.
    pub allow_remote: bool,
    pub accounts: Vec<Account>,
    /// AudioShare link.
    pub url: String,
    pub studio_name: String,
    pub device_name: String,
    pub linked_at: String,
    /// The token was rejected (401): link again.
    pub needs_relink: bool,
    /// The studio's programmes (with norms), refreshed regularly.
    pub programs: Vec<Program>,
    /// Network folders with their own logon.
    pub shares: Vec<Share>,
    /// Desktop mode: start with Windows (for this user).
    pub autostart: bool,
    pub installed_once: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            port: 8470,
            allow_remote: false,
            accounts: Vec::new(),
            url: String::new(),
            studio_name: String::new(),
            device_name: String::new(),
            linked_at: String::new(),
            needs_relink: false,
            programs: Vec::new(),
            shares: Vec::new(),
            autostart: true,
            installed_once: false,
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        read_json(&data_dir().join("settings.json"))
    }
    pub fn save(&self) {
        write_json(&data_dir().join("settings.json"), self);
    }
    pub fn linked(&self) -> bool {
        !self.url.is_empty() && !self.needs_relink
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[default]
    File,
    Folder,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Upload at fixed times (days + time).
    Schedule,
    /// Upload as soon as the source is new or changed.
    #[default]
    Watch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateRule {
    #[default]
    Today,
    Tomorrow,
    /// First of `air_days` from today on (today included).
    NextAirDay,
    /// From the file name: 2026-11-14, 20261114 or 14-11-2026.
    FromName,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub program_id: i64,
    pub source_kind: SourceKind,
    pub source: String,
    pub trigger: Trigger,
    /// Schedule: weekdays 1 (Monday) … 7 (Sunday).
    pub days: Vec<u8>,
    /// Schedule: "HH:MM".
    pub time: String,
    /// Seconds a file must stay unchanged before it is uploaded.
    pub settle_secs: u64,
    pub date_rule: DateRule,
    /// NextAirDay: weekdays 1 … 7.
    pub air_days: Vec<u8>,
    pub title_template: String,
    pub description: String,
}

impl Default for Task {
    fn default() -> Self {
        Task {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            enabled: true,
            program_id: 0,
            source_kind: SourceKind::File,
            source: String::new(),
            trigger: Trigger::Watch,
            days: Vec::new(),
            time: "16:00".into(),
            settle_secs: 60,
            date_rule: DateRule::Today,
            air_days: Vec::new(),
            title_template: "{programma} {datum}".into(),
            description: String::new(),
        }
    }
}

/// What the uploader remembers per task (separate from the settings people edit).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskState {
    /// Fingerprint (size + mtime + path) of the last file sent for upload.
    pub last_fingerprint: String,
    /// SHA-256 of the last uploaded content, per broadcast date.
    pub last_sha_for_date: std::collections::HashMap<String, String>,
    pub last_upload_at: String,
    pub last_upload_date: String,
    pub last_upload_title: String,
    pub last_upload_file: String,
    pub last_error: String,
    pub last_error_at: String,
    /// Upload job in the queue right now.
    pub pending_job: Option<String>,
    /// Converted copy for the pending job (deleted afterwards).
    pub pending_work_file: Option<String>,
    /// Fingerprint of the file the pending job came from.
    pub pending_fingerprint: String,
    /// SHA-256 of the source file of the pending job, and its broadcast date.
    pub pending_src_sha: String,
    pub pending_date: String,
    /// Schedule: the last slot handled, e.g. "2026-11-14 16:00".
    pub last_slot: String,
    /// "Source not found" alert: last day it was sent.
    pub missing_alert_day: String,
    /// Watch: last seen fingerprint and since when it is unchanged.
    pub seen_fingerprint: String,
    pub seen_since: i64,
    /// A temporary problem was alerted for this job already.
    pub slow_alerted_job: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskFile {
    pub tasks: Vec<Task>,
    pub state: std::collections::HashMap<String, TaskState>,
}

impl TaskFile {
    pub fn load() -> TaskFile {
        read_json(&data_dir().join("tasks.json"))
    }
    pub fn save(&self) {
        write_json(&data_dir().join("tasks.json"), self);
    }
}
