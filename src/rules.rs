//! The rules of a task, as pure functions: which file, which broadcast date,
//! which title, and whether a scheduled moment has come. Easy to test.

use crate::config::{DateRule, SourceKind, Task};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use std::path::{Path, PathBuf};

pub const AUDIO_EXT: [&str; 4] = ["mp3", "wav", "flac", "ogg"];

pub fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| AUDIO_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

/// The file a task would upload now: the file itself, or the newest audio
/// file in the folder (by modification time; partial downloads are skipped).
pub fn resolve_source(task: &Task) -> Result<PathBuf, String> {
    if task.source.trim().is_empty() {
        return Err("Geen bron ingesteld".into());
    }
    // Y:\ → \\NAS\share, and log on to the share when needed
    let p = crate::shares::prepare(&task.source)?;
    match task.source_kind {
        SourceKind::File => {
            if p.is_file() {
                Ok(p)
            } else {
                Err(format!("Bestand niet gevonden: {}", p.display()))
            }
        }
        SourceKind::Folder => {
            if !p.is_dir() {
                return Err(format!("Map niet gevonden: {}", p.display()));
            }
            let newest = std::fs::read_dir(&p)
                .map_err(|e| format!("Map niet leesbaar: {e}"))?
                .flatten()
                .map(|e| e.path())
                .filter(|f| f.is_file() && is_audio(f))
                .filter_map(|f| std::fs::metadata(&f).and_then(|m| m.modified()).ok().map(|t| (t, f)))
                .max_by_key(|(t, _)| *t);
            newest.map(|(_, f)| f).ok_or_else(|| format!("Geen audiobestand in {}", p.display()))
        }
    }
}

/// Size + modification time + path: changes when the file is replaced or rewritten.
pub fn fingerprint(p: &Path) -> Option<String> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    Some(format!("{}|{}|{}", p.display(), m.len(), t))
}

/// Weekday as 1 (Monday) … 7 (Sunday).
pub fn wd(d: NaiveDate) -> u8 {
    d.weekday().number_from_monday() as u8
}

/// Find a date in a file name: 2026-11-14, 2026_11_14, 20261114, 14-11-2026.
pub fn date_from_name(name: &str) -> Option<NaiveDate> {
    let digits: Vec<(usize, char)> = name.char_indices().collect();
    let s = name;
    // yyyy-mm-dd / yyyy_mm_dd / yyyy.mm.dd
    for (i, _) in s.char_indices() {
        let rest = &s[i..];
        if rest.len() >= 10 {
            let b = rest.as_bytes();
            let is_d = |k: usize| b[k].is_ascii_digit();
            let sep = |k: usize| matches!(b[k], b'-' | b'_' | b'.');
            if (0..4).all(is_d) && sep(4) && is_d(5) && is_d(6) && sep(7) && is_d(8) && is_d(9) {
                if let Some(d) = ymd(&rest[0..4], &rest[5..7], &rest[8..10]) {
                    return Some(d);
                }
            }
            // dd-mm-yyyy
            if is_d(0) && is_d(1) && sep(2) && is_d(3) && is_d(4) && sep(5) && (6..10).all(is_d) {
                if let Some(d) = ymd(&rest[6..10], &rest[3..5], &rest[0..2]) {
                    return Some(d);
                }
            }
        }
    }
    // yyyymmdd as a run of exactly 8 digits
    let mut run = String::new();
    for (_, c) in digits.iter().chain(std::iter::once(&(0, ' '))) {
        if c.is_ascii_digit() {
            run.push(*c);
        } else {
            if run.len() == 8 {
                if let Some(d) = ymd(&run[0..4], &run[4..6], &run[6..8]) {
                    return Some(d);
                }
            }
            run.clear();
        }
    }
    None
}

fn ymd(y: &str, m: &str, d: &str) -> Option<NaiveDate> {
    let y: i32 = y.parse().ok()?;
    if !(2000..=2100).contains(&y) {
        return None;
    }
    NaiveDate::from_ymd_opt(y, m.parse().ok()?, d.parse().ok()?)
}

/// The broadcast date for a file uploaded `today`.
pub fn broadcast_date(task: &Task, file: &Path, today: NaiveDate) -> Result<NaiveDate, String> {
    match task.date_rule {
        DateRule::Today => Ok(today),
        DateRule::Tomorrow => Ok(today + Duration::days(1)),
        DateRule::NextAirDay => {
            if task.air_days.is_empty() {
                return Err("Kies bij de taak op welke dagen het programma wordt uitgezonden".into());
            }
            (0..7).map(|i| today + Duration::days(i)).find(|d| task.air_days.contains(&wd(*d))).ok_or_else(|| "Geen uitzenddag gevonden".into())
        }
        DateRule::FromName => {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            date_from_name(name).ok_or_else(|| format!("Geen datum gevonden in de bestandsnaam {name} (verwacht bijv. 2026-11-14, 20261114 of 14-11-2026)"))
        }
    }
}

pub fn day_name(d: NaiveDate) -> &'static str {
    match d.weekday() {
        Weekday::Mon => "maandag",
        Weekday::Tue => "dinsdag",
        Weekday::Wed => "woensdag",
        Weekday::Thu => "donderdag",
        Weekday::Fri => "vrijdag",
        Weekday::Sat => "zaterdag",
        Weekday::Sun => "zondag",
    }
}

/// Fill in the title template: {programma}, {datum}, {dag}, {bestand}.
pub fn title(template: &str, program: &str, date: NaiveDate, file: &Path) -> String {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let t = if template.trim().is_empty() { "{programma} {datum}" } else { template };
    let out = t
        .replace("{programma}", program)
        .replace("{datum}", &date.format("%d-%m-%Y").to_string())
        .replace("{dag}", day_name(date))
        .replace("{bestand}", stem);
    out.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect()
}

pub fn parse_time(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// The most recent scheduled moment at or before `now` (within the last 7
/// days), as "YYYY-MM-DD HH:MM". None when the task has no valid schedule.
pub fn last_slot(task: &Task, now: NaiveDateTime) -> Option<String> {
    let t = parse_time(&task.time)?;
    if task.days.is_empty() {
        return None;
    }
    (0..8)
        .map(|i| now.date() - Duration::days(i))
        .filter(|d| task.days.contains(&wd(*d)))
        .map(|d| d.and_time(t))
        .find(|dt| *dt <= now)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
}

/// The next scheduled moment after `now`.
pub fn next_slot(task: &Task, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let t = parse_time(&task.time)?;
    (0..8).map(|i| now.date() + Duration::days(i)).filter(|d| task.days.contains(&wd(*d))).map(|d| d.and_time(t)).find(|dt| *dt > now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    #[test]
    fn dates_in_names() {
        assert_eq!(date_from_name("Blik 2026-11-14 uur1.mp3"), Some(d(2026, 11, 14)));
        assert_eq!(date_from_name("blik_2026_11_14.wav"), Some(d(2026, 11, 14)));
        assert_eq!(date_from_name("blik-20261114.mp3"), Some(d(2026, 11, 14)));
        assert_eq!(date_from_name("14-11-2026 De Blik.flac"), Some(d(2026, 11, 14)));
        assert_eq!(date_from_name("take 12345678.mp3"), None);
        assert_eq!(date_from_name("De Blik.mp3"), None);
        assert_eq!(date_from_name("2026-13-40.mp3"), None);
    }

    #[test]
    fn broadcast_dates() {
        let mut t = Task::default();
        let f = Path::new("x.mp3");
        let fri = d(2026, 11, 13); // Friday
        t.date_rule = DateRule::Today;
        assert_eq!(broadcast_date(&t, f, fri).unwrap(), fri);
        t.date_rule = DateRule::Tomorrow;
        assert_eq!(broadcast_date(&t, f, fri).unwrap(), d(2026, 11, 14));
        t.date_rule = DateRule::NextAirDay;
        t.air_days = vec![6]; // Saturday
        assert_eq!(broadcast_date(&t, f, fri).unwrap(), d(2026, 11, 14));
        t.air_days = vec![5]; // Friday itself counts
        assert_eq!(broadcast_date(&t, f, fri).unwrap(), fri);
        t.air_days = vec![];
        assert!(broadcast_date(&t, f, fri).is_err());
        t.date_rule = DateRule::FromName;
        assert_eq!(broadcast_date(&t, Path::new("Blik 20261120.mp3"), fri).unwrap(), d(2026, 11, 20));
    }

    #[test]
    fn titles() {
        let dt = d(2026, 11, 14);
        assert_eq!(title("{programma} {datum}", "De Blik", dt, Path::new("a.mp3")), "De Blik 14-11-2026");
        assert_eq!(title("{programma} van {dag}", "De Blik", dt, Path::new("a.mp3")), "De Blik van zaterdag");
        assert_eq!(title("{bestand}", "X", dt, Path::new("/m/Show 1.wav")), "Show 1");
        assert_eq!(title("", "X", dt, Path::new("a.mp3")), "X 14-11-2026");
    }

    #[test]
    fn schedule_slots() {
        let mut t = Task { days: vec![5], time: "16:00".into(), ..Default::default() };
        let fri_15 = d(2026, 11, 13).and_hms_opt(15, 0, 0).unwrap();
        let fri_17 = d(2026, 11, 13).and_hms_opt(17, 0, 0).unwrap();
        assert_eq!(last_slot(&t, fri_17).as_deref(), Some("2026-11-13 16:00"));
        assert_eq!(last_slot(&t, fri_15).as_deref(), Some("2026-11-06 16:00"));
        assert_eq!(next_slot(&t, fri_15).unwrap(), d(2026, 11, 13).and_hms_opt(16, 0, 0).unwrap());
        assert_eq!(next_slot(&t, fri_17).unwrap(), d(2026, 11, 20).and_hms_opt(16, 0, 0).unwrap());
        t.days.clear();
        assert!(last_slot(&t, fri_17).is_none());
    }

    #[test]
    fn newest_in_folder() {
        let dir = std::env::temp_dir().join(format!("ausu-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.mp3"), b"a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("notes.txt"), b"b").unwrap();
        std::fs::write(dir.join("new.wav"), b"c").unwrap();
        let t = Task { source_kind: SourceKind::Folder, source: dir.display().to_string(), ..Default::default() };
        assert_eq!(resolve_source(&t).unwrap().file_name().unwrap(), "new.wav");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
