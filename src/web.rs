//! The web interface: a small JSON API plus three static files.
//!
//! Security: only the local network unless switched on; login with a
//! password (Argon2id); session cookie HttpOnly + SameSite=Strict; every
//! change needs the X-AUSU header (a cross-site page can't send that);
//! 5 wrong passwords = 5 minutes wait; the first account can only be made
//! on the server itself.

use crate::audioshare::queue::{JobState, Level};
use crate::config::{Account, SourceKind, Task, Trigger};
use crate::engine::App;
use crate::rules;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");
const SESSION_SECS: i64 = 12 * 3600;

struct Web {
    app: Arc<App>,
    sessions: Mutex<HashMap<String, (String, i64)>>,
    fails: Mutex<HashMap<IpAddr, (u32, i64)>>,
}

fn now() -> i64 {
    crate::audioshare::queue::now()
}

pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])),
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return is_local(IpAddr::V4(m));
            }
            v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback() || v.to_ipv4_mapped().is_some_and(|m| m.is_loopback()),
    }
}

// ---------------------------------------------------------------------------
// Passwords
// ---------------------------------------------------------------------------

fn hash_password(pw: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|e| e.to_string())?;
    Ok(format!("{}${}", hex::encode(salt), hex::encode(derive(pw, &salt)?)))
}

fn derive(pw: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(19 * 1024, 2, 1, Some(32)).map_err(|e| e.to_string())?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(pw.as_bytes(), salt, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn check_password(pw: &str, stored: &str) -> bool {
    let Some((s, h)) = stored.split_once('$') else { return false };
    let (Ok(salt), Ok(want)) = (hex::decode(s), hex::decode(h)) else { return false };
    let Ok(got) = derive(pw, &salt) else { return false };
    want.len() == got.len() && want.iter().zip(got.iter()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

fn valid_new_password(pw: &str) -> Result<(), String> {
    if pw.chars().count() < 10 {
        return Err("Kies een wachtwoord van minimaal 10 tekens.".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

pub fn serve(app: Arc<App>) -> Result<(), String> {
    let port = app.settings.lock().unwrap().port;
    let server = Server::http(("0.0.0.0", port)).map_err(|e| format!("Poort {port} is niet beschikbaar: {e}"))?;
    app.log("systeem", Level::Info, format!("Webinterface gestart op poort {port}"));
    let web = Arc::new(Web { app: app.clone(), sessions: Mutex::new(HashMap::new()), fails: Mutex::new(HashMap::new()) });
    while !app.stop.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(500)) {
            Ok(Some(req)) => {
                let w = web.clone();
                std::thread::spawn(move || w.handle(req));
            }
            Ok(None) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    Ok(())
}

fn hdr(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap()
}

fn security_headers<R: Read>(mut r: Response<R>) -> Response<R> {
    for (k, v) in [
        ("X-Frame-Options", "DENY"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Content-Security-Policy", "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'"),
        ("Cache-Control", "no-store"),
    ] {
        r.add_header(hdr(k, v));
    }
    r
}

fn json_resp(status: u16, v: &Value) -> Response<std::io::Cursor<Vec<u8>>> {
    security_headers(Response::from_string(v.to_string()).with_status_code(status).with_header(hdr("Content-Type", "application/json; charset=utf-8")))
}

fn err(status: u16, msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json_resp(status, &json!({ "error": msg }))
}

fn cookie(req: &Request) -> Option<String> {
    req.headers().iter().find(|h| h.field.equiv("Cookie")).and_then(|h| {
        h.value.as_str().split(';').map(str::trim).find_map(|c| c.strip_prefix("ausu=").map(str::to_string))
    })
}

impl Web {
    fn handle(&self, mut req: Request) {
        let ip = req.remote_addr().map(SocketAddr::ip).unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let allow_remote = self.app.settings.lock().unwrap().allow_remote;
        if !allow_remote && !is_local(ip) {
            let _ = req.respond(err(403, "Alleen bereikbaar vanaf het lokale netwerk"));
            return;
        }
        let url = req.url().split('?').next().unwrap_or("/").to_string();
        let method = req.method().clone();
        // static files
        if method == Method::Get {
            let (body, ctype) = match url.as_str() {
                "/" | "/index.html" => (INDEX, "text/html; charset=utf-8"),
                "/app.js" => (APP_JS, "application/javascript; charset=utf-8"),
                "/app.css" => (APP_CSS, "text/css; charset=utf-8"),
                _ => ("", ""),
            };
            if !body.is_empty() {
                let _ = req.respond(security_headers(Response::from_string(body).with_header(hdr("Content-Type", ctype))));
                return;
            }
        }
        if !url.starts_with("/api/") {
            let _ = req.respond(err(404, "Niet gevonden"));
            return;
        }
        // Changes need the custom header (blocks cross-site requests) and JSON.
        if method != Method::Get {
            let ok_hdr = req.headers().iter().any(|h| h.field.equiv("X-AUSU") && h.value.as_str() == "1");
            if !ok_hdr {
                let _ = req.respond(err(403, "Ongeldig verzoek"));
                return;
            }
        }
        let mut body = String::new();
        let _ = req.as_reader().take(2 * 1024 * 1024).read_to_string(&mut body);
        let input: Value = if body.trim().is_empty() { json!({}) } else { serde_json::from_str(&body).unwrap_or(Value::Null) };
        if input.is_null() {
            let _ = req.respond(err(400, "Ongeldige JSON"));
            return;
        }
        let user = cookie(&req).and_then(|c| {
            let mut s = self.sessions.lock().unwrap();
            s.retain(|_, (_, exp)| *exp > now());
            s.get(&c).map(|(u, _)| u.clone())
        });
        let resp = self.route(&method, &url, &input, user, ip, cookie(&req));
        let _ = req.respond(resp);
    }

    fn route(&self, method: &Method, url: &str, input: &Value, user: Option<String>, ip: IpAddr, sess: Option<String>) -> Response<std::io::Cursor<Vec<u8>>> {
        let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match (method, url) {
            (Method::Get, "/api/state") => {
                let setup = self.app.settings.lock().unwrap().accounts.is_empty();
                json_resp(200, &json!({ "setup_needed": setup, "setup_allowed": setup && is_loopback(ip), "user": user, "version": env!("CARGO_PKG_VERSION") }))
            }
            (Method::Post, "/api/setup") => {
                let mut st = self.app.settings.lock().unwrap();
                if !st.accounts.is_empty() {
                    return err(400, "Er is al een account");
                }
                if !is_loopback(ip) {
                    return err(403, "Het eerste account maak je op de server zelf aan, via http://localhost");
                }
                let name = s("name").trim().to_string();
                if name.is_empty() {
                    return err(400, "Vul een gebruikersnaam in");
                }
                if let Err(e) = valid_new_password(&s("password")) {
                    return err(400, &e);
                }
                let hash = match hash_password(&s("password")) {
                    Ok(h) => h,
                    Err(e) => return err(500, &e),
                };
                st.accounts.push(Account { name: name.clone(), hash });
                st.save();
                drop(st);
                self.app.log("systeem", Level::Ok, format!("Beheeraccount {name} aangemaakt"));
                self.login_response(&name)
            }
            (Method::Post, "/api/login") => {
                {
                    let f = self.fails.lock().unwrap();
                    if let Some((n, until)) = f.get(&ip) {
                        if *n >= 5 && *until > now() {
                            return err(429, &format!("Te veel foute pogingen. Probeer het over {} minuten opnieuw.", (*until - now()) / 60 + 1));
                        }
                    }
                }
                let acc = self.app.settings.lock().unwrap().accounts.iter().find(|a| a.name.eq_ignore_ascii_case(s("name").trim())).cloned();
                if let Some(a) = acc.filter(|a| check_password(&s("password"), &a.hash)) {
                    self.fails.lock().unwrap().remove(&ip);
                    return self.login_response(&a.name);
                }
                let mut f = self.fails.lock().unwrap();
                let e = f.entry(ip).or_insert((0, 0));
                if e.1 <= now() && e.0 >= 5 {
                    *e = (0, 0);
                }
                e.0 += 1;
                if e.0 >= 5 {
                    e.1 = now() + 300;
                }
                drop(f);
                self.app.log("systeem", Level::Warn, format!("Mislukte inlogpoging vanaf {ip}"));
                err(401, "Onjuiste gebruikersnaam of wachtwoord")
            }
            (Method::Post, "/api/logout") => {
                if let Some(c) = sess {
                    self.sessions.lock().unwrap().remove(&c);
                }
                json_resp(200, &json!({ "ok": true })).with_header(hdr("Set-Cookie", "ausu=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict"))
            }
            _ => match user {
                None => err(401, "Log eerst in"),
                Some(u) => self.authed(method, url, input, &u),
            },
        }
    }

    fn login_response(&self, name: &str) -> Response<std::io::Cursor<Vec<u8>>> {
        let mut b = [0u8; 32];
        let _ = getrandom::getrandom(&mut b);
        let tok = hex::encode(b);
        self.sessions.lock().unwrap().insert(tok.clone(), (name.to_string(), now() + SESSION_SECS));
        json_resp(200, &json!({ "ok": true, "user": name }))
            .with_header(hdr("Set-Cookie", &format!("ausu={tok}; Path=/; Max-Age={SESSION_SECS}; HttpOnly; SameSite=Strict")))
    }

    fn authed(&self, method: &Method, url: &str, input: &Value, user: &str) -> Response<std::io::Cursor<Vec<u8>>> {
        let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let parts: Vec<&str> = url.trim_start_matches("/api/").split('/').collect();
        match (method, parts.as_slice()) {
            (Method::Get, ["overview"]) => json_resp(200, &self.overview()),
            (Method::Get, ["log"]) => {
                let d = self.app.queue.data.lock().unwrap();
                let log: Vec<Value> = d.log.iter().rev().take(400).map(|l| json!({ "time": l.time, "who": l.profile, "level": format!("{:?}", l.level).to_lowercase(), "text": l.text })).collect();
                json_resp(200, &json!({ "log": log }))
            }
            (Method::Post, ["tasks"]) => {
                let mut t: Task = match serde_json::from_value(input.clone()) {
                    Ok(t) => t,
                    Err(e) => return err(400, &format!("Ongeldige taak: {e}")),
                };
                if let Err(e) = self.validate(&mut t) {
                    return err(400, &e);
                }
                let mut tf = self.app.tasks.lock().unwrap();
                let old = tf.tasks.iter().find(|x| x.id == t.id).cloned();
                // New task or other source: the file that is there now counts as already sent.
                let rebase = old.as_ref().is_none_or(|o| o.source != t.source || o.source_kind != t.source_kind);
                if rebase {
                    let fp = rules::resolve_source(&t).ok().and_then(|f| rules::fingerprint(&f)).unwrap_or_default();
                    let st = tf.state.entry(t.id.clone()).or_default();
                    st.last_fingerprint = fp.clone();
                    st.seen_fingerprint = fp;
                    st.seen_since = now();
                    st.last_slot.clear();
                }
                match tf.tasks.iter_mut().find(|x| x.id == t.id) {
                    Some(slot) => *slot = t.clone(),
                    None => tf.tasks.push(t.clone()),
                }
                tf.save();
                drop(tf);
                self.app.log(&t.name, Level::Info, format!("Taak {} door {user}", if old.is_some() { "gewijzigd" } else { "aangemaakt" }));
                json_resp(200, &json!({ "ok": true, "id": t.id }))
            }
            (Method::Delete, ["tasks", id]) => {
                let mut tf = self.app.tasks.lock().unwrap();
                let name = tf.tasks.iter().find(|t| t.id == *id).map(|t| t.name.clone()).unwrap_or_default();
                tf.tasks.retain(|t| t.id != *id);
                tf.state.remove(*id);
                tf.save();
                drop(tf);
                self.app.log(&name, Level::Info, format!("Taak verwijderd door {user}"));
                json_resp(200, &json!({ "ok": true }))
            }
            (Method::Post, ["tasks", id, "run"]) => {
                if !self.app.tasks.lock().unwrap().tasks.iter().any(|t| t.id == *id) {
                    return err(404, "Taak niet gevonden");
                }
                self.app.run_now.lock().unwrap().push(id.to_string());
                json_resp(200, &json!({ "ok": true }))
            }
            (Method::Post, ["preview"]) => {
                let t: Task = match serde_json::from_value(input.clone()) {
                    Ok(t) => t,
                    Err(e) => return err(400, &format!("Ongeldige taak: {e}")),
                };
                json_resp(200, &self.preview(&t))
            }
            (Method::Post, ["link"]) => {
                let (mut url, mut code) = (s("url"), s("code"));
                if !s("file").is_empty() {
                    let f: Value = serde_json::from_str(s("file").trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
                    if f["type"] != "audioshare-maker-link" {
                        return err(400, "Dit is geen geldig koppelbestand");
                    }
                    url = f["url"].as_str().unwrap_or("").to_string();
                    code = f["claim_code"].as_str().unwrap_or("").to_string();
                }
                if url.trim().is_empty() || code.trim().is_empty() {
                    return err(400, "Vul het adres van AudioShare en de koppelcode in, of kies het koppelbestand");
                }
                let device = if s("device_name").trim().is_empty() { computer_name() } else { s("device_name").trim().to_string() };
                match self.app.link(&url, &code, &device) {
                    Ok(m) => json_resp(200, &json!({ "ok": true, "message": m })),
                    Err(e) => err(400, &e),
                }
            }
            (Method::Post, ["unlink"]) => {
                self.app.unlink();
                json_resp(200, &json!({ "ok": true }))
            }
            (Method::Post, ["test"]) => {
                let Some(c) = self.app.client() else { return err(400, "Niet gekoppeld") };
                match c.me() {
                    Ok(me) => {
                        let progs = self.app.refresh_programs().unwrap_or_default();
                        json_resp(200, &json!({ "ok": true, "message": format!("Verbinding in orde: {} met {} programma's", me.user.display_name, progs.len()) }))
                    }
                    Err(e) => err(400, &e.user_message()),
                }
            }
            (Method::Post, ["password"]) => {
                let mut st = self.app.settings.lock().unwrap();
                let Some(a) = st.accounts.iter_mut().find(|a| a.name == user) else { return err(400, "Account niet gevonden") };
                if !check_password(&s("old"), &a.hash) {
                    return err(400, "Het huidige wachtwoord klopt niet");
                }
                if let Err(e) = valid_new_password(&s("new")) {
                    return err(400, &e);
                }
                a.hash = match hash_password(&s("new")) {
                    Ok(h) => h,
                    Err(e) => return err(500, &e),
                };
                st.save();
                json_resp(200, &json!({ "ok": true }))
            }
            (Method::Post, ["accounts"]) => {
                let mut st = self.app.settings.lock().unwrap();
                let name = s("name").trim().to_string();
                if name.is_empty() || st.accounts.iter().any(|a| a.name.eq_ignore_ascii_case(&name)) {
                    return err(400, "Vul een nieuwe, nog niet gebruikte naam in");
                }
                if let Err(e) = valid_new_password(&s("password")) {
                    return err(400, &e);
                }
                let hash = match hash_password(&s("password")) {
                    Ok(h) => h,
                    Err(e) => return err(500, &e),
                };
                st.accounts.push(Account { name: name.clone(), hash });
                st.save();
                drop(st);
                self.app.log("systeem", Level::Info, format!("Account {name} aangemaakt door {user}"));
                json_resp(200, &json!({ "ok": true }))
            }
            (Method::Delete, ["accounts", name]) => {
                let mut st = self.app.settings.lock().unwrap();
                if *name == user {
                    return err(400, "Je kunt je eigen account niet verwijderen");
                }
                st.accounts.retain(|a| a.name != *name);
                st.save();
                json_resp(200, &json!({ "ok": true }))
            }
            (Method::Post, ["shutdown"]) => {
                if crate::engine::AS_SERVICE.load(Ordering::Relaxed) {
                    return err(400, "De service stop je via Windows (Services)");
                }
                self.app.log("systeem", Level::Info, format!("Afgesloten door {user}"));
                self.app.stop.store(true, Ordering::Relaxed);
                json_resp(200, &json!({ "ok": true }))
            }
            (Method::Post, ["shares"]) => {
                let unc = crate::shares::normalize_unc(&s("unc"));
                let letter = crate::shares::normalize_letter(&s("letter"));
                let user_name = s("user").trim().to_string();
                if unc.trim_start_matches('\\').split('\\').filter(|x| !x.is_empty()).count() < 2 {
                    return err(400, "Vul het netwerkpad in als \\\\NAS\\gedeelde-map, bijv. \\\\NASBR6\\Mediafiles_Playout");
                }
                let share = crate::config::Share { letter: letter.clone(), unc: unc.clone(), user: user_name.clone() };
                if !s("password").is_empty() {
                    if let Err(e) = crate::shares::save_password(&unc, &s("password")) {
                        return err(500, &e);
                    }
                }
                // Test right away, so a typo shows now and not at 20:00.
                let test = crate::shares::connect(&share, true).and_then(|_| {
                    std::fs::read_dir(&unc).map(|d| d.count()).map_err(|e| format!("Ingelogd, maar de map is niet leesbaar: {e}"))
                });
                let mut st = self.app.settings.lock().unwrap();
                st.shares.retain(|x| !x.unc.eq_ignore_ascii_case(&unc) && (letter.is_empty() || x.letter != letter));
                st.shares.push(share);
                st.shares.sort_by(|a, b| a.letter.cmp(&b.letter).then(a.unc.cmp(&b.unc)));
                st.save();
                drop(st);
                self.app.log("netwerk", Level::Info, format!("Netwerkmap {}{} ingesteld door {user}", if letter.is_empty() { String::new() } else { format!("{letter} = ") }, unc));
                match test {
                    Ok(n) => json_resp(200, &json!({ "ok": true, "message": format!("Opgeslagen. Verbinding met {unc} in orde ({n} items in de map).") })),
                    Err(e) => json_resp(200, &json!({ "ok": false, "message": format!("Opgeslagen, maar: {e}") })),
                }
            }
            (Method::Post, ["shares", "test"]) => {
                let unc = crate::shares::normalize_unc(&s("unc"));
                let share = self.app.settings.lock().unwrap().shares.iter().find(|x| x.unc.eq_ignore_ascii_case(&unc)).cloned();
                let Some(share) = share else { return err(404, "Netwerkmap niet gevonden") };
                match crate::shares::connect(&share, true).and_then(|_| std::fs::read_dir(&share.unc).map(|d| d.count()).map_err(|e| format!("Ingelogd, maar de map is niet leesbaar: {e}"))) {
                    Ok(n) => json_resp(200, &json!({ "ok": true, "message": format!("Verbinding met {} in orde ({n} items).", share.unc) })),
                    Err(e) => err(400, &e),
                }
            }
            (Method::Post, ["shares", "delete"]) => {
                let unc = crate::shares::normalize_unc(&s("unc"));
                let mut st = self.app.settings.lock().unwrap();
                st.shares.retain(|x| !x.unc.eq_ignore_ascii_case(&unc));
                st.save();
                drop(st);
                crate::shares::forget(&unc);
                json_resp(200, &json!({ "ok": true }))
            }
            (Method::Post, ["settings"]) => {
                let mut st = self.app.settings.lock().unwrap();
                if let Some(p) = input.get("port").and_then(Value::as_u64) {
                    if !(1024..=65535).contains(&p) {
                        return err(400, "Kies een poort tussen 1024 en 65535");
                    }
                    st.port = p as u16;
                }
                if let Some(b) = input.get("allow_remote").and_then(Value::as_bool) {
                    st.allow_remote = b;
                }
                if let Some(b) = input.get("autostart").and_then(Value::as_bool) {
                    if !crate::engine::AS_SERVICE.load(Ordering::Relaxed) {
                        if let Err(e) = crate::desktop::set_autostart(b) {
                            return err(500, &e);
                        }
                    }
                    st.autostart = b;
                }
                st.save();
                json_resp(200, &json!({ "ok": true, "message": "Opgeslagen. Een andere poort werkt na het herstarten van de service." }))
            }
            _ => err(404, "Niet gevonden"),
        }
    }

    fn validate(&self, t: &mut Task) -> Result<(), String> {
        t.name = t.name.trim().chars().take(60).collect();
        t.source = t.source.trim().trim_matches('"').to_string();
        if t.id.trim().is_empty() {
            t.id = uuid::Uuid::new_v4().to_string();
        }
        if t.name.is_empty() {
            return Err("Geef de taak een naam".into());
        }
        if !self.app.settings.lock().unwrap().programs.iter().any(|p| p.id == t.program_id) {
            return Err("Kies een programma (AudioShare moet het voor deze studio toestaan)".into());
        }
        if t.source.is_empty() {
            return Err("Vul de bron in (bestand of map)".into());
        }
        if t.trigger == Trigger::Schedule {
            if t.days.is_empty() {
                return Err("Kies op welke dagen er geüpload wordt".into());
            }
            if rules::parse_time(&t.time).is_none() {
                return Err("Vul een tijd in als UU:MM, bijv. 16:00".into());
            }
        }
        if t.date_rule == crate::config::DateRule::NextAirDay && t.air_days.is_empty() {
            return Err("Kies de uitzenddagen".into());
        }
        t.days.retain(|d| (1..=7).contains(d));
        t.air_days.retain(|d| (1..=7).contains(d));
        t.settle_secs = t.settle_secs.clamp(10, 3600);
        t.title_template = t.title_template.chars().take(200).collect();
        t.description = t.description.chars().take(5000).collect();
        Ok(())
    }

    /// What the task would do right now (for the form).
    fn preview(&self, t: &Task) -> Value {
        let program = self.app.settings.lock().unwrap().programs.iter().find(|p| p.id == t.program_id).map(|p| p.name.clone());
        let file = rules::resolve_source(t);
        let today = chrono::Local::now().date_naive();
        let mut out = json!({});
        match &file {
            Ok(f) => {
                out["file"] = json!(f.display().to_string());
                let m = std::fs::metadata(f).ok();
                out["size_mb"] = json!(m.as_ref().map(|m| (m.len() as f64 / 1_048_576.0 * 10.0).round() / 10.0));
                out["modified"] = json!(m.and_then(|m| m.modified().ok()).map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%d-%m-%Y %H:%M").to_string()));
                match rules::broadcast_date(t, f, today) {
                    Ok(d) => {
                        out["date"] = json!(format!("{} {}", rules::day_name(d), d.format("%d-%m-%Y")));
                        out["title"] = json!(rules::title(&t.title_template, program.as_deref().unwrap_or("(programma)"), d, f));
                    }
                    Err(e) => out["date_error"] = json!(e),
                }
            }
            Err(e) => out["file_error"] = json!(e),
        }
        if t.trigger == Trigger::Schedule {
            if let Some(n) = rules::next_slot(t, chrono::Local::now().naive_local()) {
                out["next"] = json!(format!("{} {}", rules::day_name(n.date()), n.format("%d-%m-%Y %H:%M")));
            }
        }
        out
    }

    fn overview(&self) -> Value {
        let st = self.app.settings.lock().unwrap().clone();
        let tf = self.app.tasks.lock().unwrap().clone();
        let busy = self.app.busy.lock().unwrap().clone();
        let d = self.app.queue.data.lock().unwrap();
        let tasks: Vec<Value> = tf
            .tasks
            .iter()
            .map(|t| {
                let s = tf.state.get(&t.id).cloned().unwrap_or_default();
                let job = s.pending_job.as_ref().and_then(|id| d.jobs.iter().find(|j| &j.id == id));
                let status = if busy.as_deref() == Some(&t.id) {
                    "Omzetten…".to_string()
                } else if let Some(j) = job {
                    match j.state {
                        JobState::Waiting | JobState::Creating => "In de wachtrij".into(),
                        JobState::Uploading => "Uploaden…".into(),
                        JobState::Finalizing => "Vrijgeven…".into(),
                        _ => j.message.clone(),
                    }
                } else if !t.enabled {
                    "Uit".into()
                } else if !s.last_error.is_empty() && newer(&s.last_error_at, &s.last_upload_at) {
                    "Fout".into()
                } else {
                    "Wacht".into()
                };
                let next = (t.trigger == Trigger::Schedule && t.enabled)
                    .then(|| rules::next_slot(t, chrono::Local::now().naive_local()))
                    .flatten()
                    .map(|n| format!("{} {}", &rules::day_name(n.date())[..2], n.format("%d-%m %H:%M")));
                json!({
                    "task": t,
                    "program": st.programs.iter().find(|p| p.id == t.program_id).map(|p| p.name.clone()),
                    "status": status,
                    "job_message": job.map(|j| j.message.clone()),
                    "next": next,
                    "last_upload_at": s.last_upload_at, "last_upload_date": s.last_upload_date, "last_upload_title": s.last_upload_title,
                    "last_error": s.last_error, "last_error_at": s.last_error_at,
                    "source_kind": match t.source_kind { SourceKind::File => "bestand", SourceKind::Folder => "map" },
                })
            })
            .collect();
        json!({
            "link": {
                "linked": st.linked(), "needs_relink": st.needs_relink, "url": st.url, "studio": st.studio_name,
                "device": st.device_name, "linked_at": st.linked_at,
            },
            "programs": st.programs.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
            "tasks": tasks,
            "settings": { "port": st.port, "allow_remote": st.allow_remote, "autostart": crate::desktop::autostart_enabled() || (cfg!(not(windows)) && st.autostart),
                "service": crate::engine::AS_SERVICE.load(Ordering::Relaxed), "accounts": st.accounts.iter().map(|a| a.name.clone()).collect::<Vec<_>>() },
            "shares": st.shares.iter().map(|x| json!({ "letter": x.letter, "unc": x.unc, "user": x.user })).collect::<Vec<_>>(),
            "computer": computer_name(),
            "version": env!("CARGO_PKG_VERSION"),
        })
    }
}

/// "dd-mm-YYYY HH:MM" a is later than b (or b is empty).
fn newer(a: &str, b: &str) -> bool {
    let p = |x: &str| chrono::NaiveDateTime::parse_from_str(x, "%d-%m-%Y %H:%M").ok();
    match (p(a), p(b)) {
        (Some(x), Some(y)) => x >= y,
        (Some(_), None) => true,
        _ => false,
    }
}

pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "SERVER".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords() {
        let h = hash_password("geheim-wachtwoord").unwrap();
        assert!(check_password("geheim-wachtwoord", &h));
        assert!(!check_password("geheim-wachtwoorD", &h));
        assert!(!check_password("x", "rommel"));
        assert!(valid_new_password("kort").is_err());
    }

    #[test]
    fn local_addresses() {
        for a in ["127.0.0.1", "10.1.2.3", "192.168.1.20", "172.16.0.5", "::1", "fd00::1", "fe80::1", "::ffff:192.168.1.2"] {
            assert!(is_local(a.parse().unwrap()), "{a}");
        }
        for a in ["8.8.8.8", "2a00:1450::1", "172.32.0.1"] {
            assert!(!is_local(a.parse().unwrap()), "{a}");
        }
    }
}
