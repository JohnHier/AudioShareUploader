//! The part that works on its own: watching sources, scheduled moments,
//! converting, queueing uploads, following them up and sending alerts.

use crate::audioshare::api::{Client, Program};
use crate::audioshare::queue::{Job, JobState, Level, Queue, now};
use crate::audioshare::secure;
use crate::config::{Settings, Task, TaskFile, TaskState, Trigger, data_dir};
use crate::rules;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Running as a Windows service (instead of under the signed-in user).
pub static AS_SERVICE: AtomicBool = AtomicBool::new(false);

/// The single AudioShare link of this uploader (key in Credential Manager / queue).
pub const PROFILE: &str = "studio";

pub struct App {
    pub settings: Mutex<Settings>,
    pub tasks: Mutex<TaskFile>,
    pub queue: Arc<Queue>,
    /// Tasks someone asked to upload now (web interface).
    pub run_now: Mutex<Vec<String>>,
    /// The task being prepared (converting) right now.
    pub busy: Mutex<Option<String>>,
    pub stop: AtomicBool,
    last_programs_refresh: Mutex<i64>,
}

impl App {
    pub fn start() -> Arc<App> {
        let mut settings = Settings::load();
        let queue = Queue::start();
        if settings.linked() {
            match secure::load_token(PROFILE) {
                Some(tok) => {
                    queue.tokens.lock().unwrap().insert(PROFILE.into(), (settings.url.clone(), tok));
                }
                // E.g. linked as a service before: that key belongs to another Windows account.
                None => {
                    settings.needs_relink = true;
                    settings.save();
                }
            }
        }
        let app = Arc::new(App {
            settings: Mutex::new(settings),
            tasks: Mutex::new(TaskFile::load()),
            queue,
            run_now: Mutex::new(Vec::new()),
            busy: Mutex::new(None),
            stop: AtomicBool::new(false),
            last_programs_refresh: Mutex::new(0),
        });
        app.auto_link();
        let a = app.clone();
        std::thread::Builder::new().name("engine".into()).spawn(move || a.run()).expect("engine thread");
        app
    }

    /// The zip from AudioShare (Recorders → Studio's → Uploader downloaden) holds
    /// the studio's .audioshare file next to the exe: link with it by itself.
    fn auto_link(&self) {
        if self.settings.lock().unwrap().linked() {
            return;
        }
        let Some(f) = crate::audioshare::api::link_files_next_to_exe().into_iter().next() else { return };
        match crate::audioshare::api::read_link_file(&f) {
            Ok(l) => match self.link(&l.url, &l.claim_code, &crate::web::computer_name()) {
                Ok(m) => {
                    crate::audioshare::api::retire_link_file(&f);
                    self.log("koppeling", Level::Ok, format!("Automatisch gekoppeld met {}: {m}", f.display()));
                }
                Err(e) => self.log("koppeling", Level::Error, format!("Koppelen met {} mislukt: {e}", f.display())),
            },
            Err(e) => self.log("koppeling", Level::Error, format!("{}: {e}", f.display())),
        }
    }

    pub fn log(&self, who: &str, level: Level, text: impl Into<String>) {
        let mut d = self.queue.data.lock().unwrap();
        d.add_log(who, level, text);
        // the queue saves on its own changes; save here too so the log survives
        crate::config::write_json(&data_dir().join("uploads.json"), &*d);
    }

    pub fn client(&self) -> Option<Client> {
        let (url, tok) = self.queue.tokens.lock().unwrap().get(PROFILE).cloned()?;
        Some(Client::new(&url, Some(tok)))
    }

    /// Ask AudioShare to mail this (in the background). Always logged.
    pub fn alert(&self, who: &str, level: &str, subject: String, message: String) {
        let lvl = if level == "error" { Level::Error } else { Level::Warn };
        self.log(who, lvl, format!("{subject}{}", if message.is_empty() { String::new() } else { format!(" — {message}") }));
        let Some(c) = self.client() else { return };
        let (level, who) = (level.to_string(), who.to_string());
        let me = self.queue.clone();
        std::thread::spawn(move || {
            if let Err(e) = c.alert(&level, &subject, &message) {
                me.data.lock().unwrap().add_log(&who, Level::Warn, format!("Melding niet verstuurd: {}", e.user_message()));
            }
        });
    }

    // -----------------------------------------------------------------------
    // Link with AudioShare
    // -----------------------------------------------------------------------

    pub fn link(&self, url: &str, code: &str, device: &str) -> Result<String, String> {
        let url = crate::audioshare::api::normalize_url(url);
        let c = Client::new(&url, None);
        let claim = c.claim(code, device).map_err(|e| e.user_message())?;
        let authed = Client::new(&url, Some(claim.token.clone()));
        if !claim.user.account_type.eq_ignore_ascii_case("studio") {
            // The code is used up now; take the link straight back.
            let _ = authed.unlink_self();
            return Err(format!(
                "Deze koppelcode hoort bij de persoon {}, niet bij een studio. Maak in AudioShare onder Recorders → Studio's een studio aan voor deze server en gebruik de koppelcode van die studio.",
                claim.user.display_name
            ));
        }
        let programs = authed.programs().unwrap_or_default();
        secure::store_token(PROFILE, &claim.token)?;
        self.queue.tokens.lock().unwrap().insert(PROFILE.into(), (url.clone(), claim.token));
        let mut s = self.settings.lock().unwrap();
        s.url = url;
        s.studio_name = claim.user.display_name.clone();
        s.device_name = device.to_string();
        s.linked_at = chrono::Local::now().format("%d-%m-%Y %H:%M").to_string();
        s.needs_relink = false;
        s.programs = programs;
        s.save();
        let names: Vec<String> = s.programs.iter().map(|p| p.name.clone()).collect();
        drop(s);
        self.log("koppeling", Level::Ok, format!("Gekoppeld als {}", claim.user.display_name));
        self.queue.wake();
        Ok(if names.is_empty() {
            format!("Gekoppeld als {}. AudioShare staat nog geen programma's toe: vink ze aan bij Recorders → Studio's.", claim.user.display_name)
        } else {
            format!("Gekoppeld als {}. Programma's: {}.", claim.user.display_name, names.join(", "))
        })
    }

    pub fn unlink(&self) {
        secure::delete_token(PROFILE);
        self.queue.tokens.lock().unwrap().remove(PROFILE);
        let mut s = self.settings.lock().unwrap();
        s.url.clear();
        s.studio_name.clear();
        s.needs_relink = false;
        s.programs.clear();
        s.save();
        drop(s);
        self.log("koppeling", Level::Warn, "Koppeling met AudioShare verwijderd");
    }

    pub fn refresh_programs(&self) -> Result<Vec<Program>, String> {
        let c = self.client().ok_or("Niet gekoppeld")?;
        let list = c.programs().map_err(|e| e.user_message())?;
        let mut s = self.settings.lock().unwrap();
        if s.programs != list {
            s.programs = list.clone();
            s.save();
        }
        Ok(list)
    }

    // -----------------------------------------------------------------------
    // The loop
    // -----------------------------------------------------------------------

    fn run(self: Arc<Self>) {
        while !self.stop.load(Ordering::Relaxed) {
            if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.tick())) {
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                self.log("systeem", Level::Error, format!("Interne fout: {msg}"));
            }
            for _ in 0..50 {
                if self.stop.load(Ordering::Relaxed) || !self.run_now.lock().unwrap().is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    fn tick(&self) {
        // Rejected token
        let unl: Vec<String> = std::mem::take(&mut *self.queue.unlinked.lock().unwrap());
        if !unl.is_empty() {
            let mut s = self.settings.lock().unwrap();
            if !s.needs_relink {
                s.needs_relink = true;
                s.save();
                drop(s);
                secure::delete_token(PROFILE);
                self.log("koppeling", Level::Error, "AudioShare heeft de koppeling ingetrokken. Koppel opnieuw met een nieuwe code (Instellingen).");
            }
        }
        // Programme list every 10 minutes
        {
            let mut last = self.last_programs_refresh.lock().unwrap();
            if now() - *last > 600 && self.client().is_some() {
                *last = now();
                drop(last);
                if let Err(e) = self.refresh_programs() {
                    self.log("koppeling", Level::Warn, format!("Programmalijst niet opgehaald: {e}"));
                }
            }
        }
        self.follow_up_jobs();
        let tasks: Vec<Task> = self.tasks.lock().unwrap().tasks.clone();
        let wanted: Vec<String> = std::mem::take(&mut *self.run_now.lock().unwrap());
        for t in tasks {
            let forced = wanted.contains(&t.id);
            if !t.enabled && !forced {
                continue;
            }
            if self.state(&t.id).pending_job.is_some() {
                if forced {
                    self.log(&t.name, Level::Warn, "Er loopt al een upload voor deze taak");
                }
                continue;
            }
            self.check_task(&t, forced);
        }
    }

    fn state(&self, id: &str) -> TaskState {
        self.tasks.lock().unwrap().state.get(id).cloned().unwrap_or_default()
    }

    fn update_state(&self, id: &str, f: impl FnOnce(&mut TaskState)) {
        let mut tf = self.tasks.lock().unwrap();
        f(tf.state.entry(id.to_string()).or_default());
        tf.save();
    }

    /// See how queued uploads ended.
    fn follow_up_jobs(&self) {
        let tasks: Vec<Task> = self.tasks.lock().unwrap().tasks.clone();
        for t in tasks {
            let st = self.state(&t.id);
            let Some(jid) = st.pending_job.clone() else { continue };
            let job = self.queue.data.lock().unwrap().jobs.iter().find(|j| j.id == jid).cloned();
            let Some(job) = job else {
                self.update_state(&t.id, |s| s.pending_job = None);
                continue;
            };
            let clean = |s: &TaskState| {
                if let Some(w) = &s.pending_work_file {
                    let _ = std::fs::remove_file(w);
                }
            };
            match job.state {
                JobState::Done => {
                    clean(&st);
                    let stamp = chrono::Local::now().format("%d-%m-%Y %H:%M").to_string();
                    self.update_state(&t.id, |s| {
                        s.last_fingerprint = s.pending_fingerprint.clone();
                        if !s.pending_date.is_empty() {
                            s.last_sha_for_date.insert(s.pending_date.clone(), s.pending_src_sha.clone());
                        }
                        // keep the date map small
                        if s.last_sha_for_date.len() > 60 {
                            let mut keys: Vec<String> = s.last_sha_for_date.keys().cloned().collect();
                            keys.sort();
                            for k in keys.iter().take(keys.len() - 60) {
                                s.last_sha_for_date.remove(k);
                            }
                        }
                        s.last_upload_at = stamp;
                        s.last_upload_date = job.date.clone();
                        s.last_upload_title = job.title.clone();
                        s.last_error.clear();
                        s.pending_job = None;
                        s.pending_work_file = None;
                    });
                }
                JobState::Failed | JobState::Cancelled => {
                    clean(&st);
                    let msg = job.message.clone();
                    self.update_state(&t.id, |s| {
                        // don't retry the same file in a loop; a new file or "Nu uploaden" does
                        s.last_fingerprint = s.pending_fingerprint.clone();
                        s.last_error = msg.clone();
                        s.last_error_at = chrono::Local::now().format("%d-%m-%Y %H:%M").to_string();
                        s.pending_job = None;
                        s.pending_work_file = None;
                    });
                    if job.state == JobState::Failed {
                        self.alert(&t.name, "error", format!("{}: upload mislukt", t.name), msg);
                    }
                }
                JobState::NeedsConfirm => {
                    // Not expected (auto_replace), but never leave it hanging.
                    self.queue.confirm_replace(&jid);
                }
                _ => {
                    // Still busy: one alert when it keeps failing for an hour.
                    let created = chrono::NaiveDateTime::parse_from_str(&job.created_at, "%Y-%m-%d %H:%M").ok();
                    let old = created.is_some_and(|c| chrono::Local::now().naive_local() - c > chrono::Duration::hours(1));
                    if old && job.attempt > 0 && st.slow_alerted_job != jid {
                        self.update_state(&t.id, |s| s.slow_alerted_job = jid.clone());
                        self.alert(&t.name, "warning", format!("{}: upload lukt al een uur niet", t.name), job.message.clone());
                    }
                }
            }
        }
    }

    fn check_task(&self, t: &Task, forced: bool) {
        let st = self.state(&t.id);
        let today = chrono::Local::now().date_naive();
        let file = match rules::resolve_source(t) {
            Ok(f) => f,
            Err(e) => {
                let day = today.to_string();
                let stamp = chrono::Local::now().format("%d-%m-%Y %H:%M").to_string();
                if st.last_error != e {
                    self.update_state(&t.id, |s| {
                        s.last_error = e.clone();
                        s.last_error_at = stamp;
                    });
                }
                if forced {
                    self.alert(&t.name, "warning", format!("{}: bron niet gevonden", t.name), e);
                } else if st.missing_alert_day != day {
                    self.update_state(&t.id, |s| s.missing_alert_day = day.clone());
                    self.alert(&t.name, "warning", format!("{}: bron niet gevonden", t.name), e);
                }
                return;
            }
        };
        // The source is back: a "not found" error no longer applies.
        if ["Bestand niet gevonden", "Map niet gevonden", "Geen audiobestand", "Map niet leesbaar", "Geen bron"].iter().any(|p| st.last_error.starts_with(p)) {
            self.update_state(&t.id, |s| s.last_error.clear());
        }
        let Some(fp) = rules::fingerprint(&file) else { return };
        // A file that is still being written must stay unchanged for a while.
        let stable = {
            let age = std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).map(|d| d.as_secs()).unwrap_or(0);
            age >= t.settle_secs
        };
        if forced {
            if !stable {
                self.log(&t.name, Level::Info, format!("{} wordt nog geschreven; upload start zodra het bestand {} s niet meer verandert", file.display(), t.settle_secs));
                self.run_now.lock().unwrap().push(t.id.clone());
                return;
            }
            self.trigger(t, &file, &fp, true);
            return;
        }
        match t.trigger {
            Trigger::Watch => {
                if fp != st.seen_fingerprint {
                    let n = now();
                    self.update_state(&t.id, |s| {
                        s.seen_fingerprint = fp.clone();
                        s.seen_since = n;
                    });
                    return;
                }
                if fp != st.last_fingerprint && stable && now() - st.seen_since >= t.settle_secs as i64 {
                    self.trigger(t, &file, &fp, false);
                }
            }
            Trigger::Schedule => {
                let n = chrono::Local::now().naive_local();
                let Some(slot) = rules::last_slot(t, n) else { return };
                if slot == st.last_slot {
                    return;
                }
                // A moment long gone (e.g. the server was off) is only recorded.
                let slot_dt = chrono::NaiveDateTime::parse_from_str(&slot, "%Y-%m-%d %H:%M").ok();
                let late = slot_dt.is_some_and(|d| n - d > chrono::Duration::hours(6));
                if st.last_slot.is_empty() || late {
                    self.update_state(&t.id, |s| s.last_slot = slot.clone());
                    if late && !st.last_slot.is_empty() {
                        self.log(&t.name, Level::Warn, format!("Geplande upload van {slot} overgeslagen (meer dan 6 uur te laat, server stond uit?)"));
                    }
                    return;
                }
                if fp == st.last_fingerprint {
                    self.update_state(&t.id, |s| s.last_slot = slot.clone());
                    self.alert(
                        &t.name,
                        "warning",
                        format!("{}: geen nieuw bestand om {}", t.name, &slot[11..]),
                        format!("Het nieuwste bestand ({}) is al eerder geüpload. Er is niets verstuurd.", file.display()),
                    );
                    return;
                }
                if !stable {
                    return; // try again next tick, until the file is complete
                }
                self.update_state(&t.id, |s| s.last_slot = slot.clone());
                self.trigger(t, &file, &fp, false);
            }
        }
    }

    fn trigger(&self, t: &Task, file: &std::path::Path, fp: &str, forced: bool) {
        let program = self.settings.lock().unwrap().programs.iter().find(|p| p.id == t.program_id).cloned();
        let Some(program) = program else {
            self.update_state(&t.id, |s| s.last_fingerprint = fp.to_string());
            self.alert(&t.name, "error", format!("{}: programma niet toegestaan", t.name), "AudioShare staat dit programma (niet meer) toe voor deze studio. Controleer Recorders → Studio's.".into());
            return;
        };
        let today = chrono::Local::now().date_naive();
        let date = match rules::broadcast_date(t, file, today) {
            Ok(d) => d,
            Err(e) => {
                self.update_state(&t.id, |s| s.last_fingerprint = fp.to_string());
                self.alert(&t.name, "error", format!("{}: geen uitzenddatum", t.name), e);
                return;
            }
        };
        let date_s = date.format("%Y-%m-%d").to_string();
        let src_sha = crate::audioshare::api::sha256_file(file).unwrap_or_default();
        if !forced && self.state(&t.id).last_sha_for_date.get(&date_s) == Some(&src_sha) {
            self.update_state(&t.id, |s| s.last_fingerprint = fp.to_string());
            self.log(&t.name, Level::Info, format!("{} is ongewijzigd ten opzichte van de vorige upload voor {}; niets verstuurd", file.display(), date.format("%d-%m-%Y")));
            return;
        }
        let title = rules::title(&t.title_template, &program.name, date, file);
        *self.busy.lock().unwrap() = Some(t.id.clone());
        self.log(&t.name, Level::Info, format!("Voorbereiden: {} → {} {}", file.display(), program.name, date.format("%d-%m-%Y")));
        let prepared = crate::media::prepare(file, &program.quality, &data_dir().join("werk"), &t.name);
        *self.busy.lock().unwrap() = None;
        let prepared = match prepared {
            Ok(p) => p,
            Err(e) => {
                self.update_state(&t.id, |s| {
                    s.last_fingerprint = fp.to_string();
                    s.last_error = e.clone();
                    s.last_error_at = chrono::Local::now().format("%d-%m-%Y %H:%M").to_string();
                });
                self.alert(&t.name, "error", format!("{}: niet geüpload", t.name), format!("{}: {e}", file.display()));
                return;
            }
        };
        if prepared.converted {
            self.log(&t.name, Level::Info, "Omgezet naar MP3 320 kbps");
        }
        let job = Job {
            profile_id: PROFILE.into(),
            profile_name: t.name.clone(),
            program_id: program.id,
            program_name: program.name.clone(),
            date: date_s.clone(),
            title,
            description: t.description.clone(),
            file: prepared.file.clone(),
            duration_sec: prepared.info.duration_sec,
            sample_rate: prepared.info.sample_rate,
            quality: program.quality.clone(),
            operator: format!("Automatische upload ({})", t.name).chars().take(80).collect(),
            auto_replace: true,
            task_id: t.id.clone(),
            ..Default::default()
        };
        let jid = job.id.clone();
        self.update_state(&t.id, |s| {
            s.pending_job = Some(jid);
            s.pending_work_file = prepared.converted.then(|| prepared.file.display().to_string());
            s.pending_fingerprint = fp.to_string();
            s.pending_src_sha = src_sha;
            s.pending_date = date_s;
            s.last_upload_file = file.display().to_string();
        });
        self.queue.add(job);
    }
}
