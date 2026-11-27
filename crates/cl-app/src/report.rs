//! Opt-in error reports to the developer: one Sentry envelope per report, posted with the HTTP
//! client the app already uses (ureq, rustls). Nothing leaves the machine unless the setting
//! „Fehlerberichte senden“ is on or the user clicks „An Entwickler senden“ on the crash notice.
//! Without a DSN (build time `CUTLINE_SENTRY_DSN`, or the same variable at runtime) nothing of
//! this is visible and no code path sends. Every string is scrubbed before it leaves.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui;
use log::Level;
use serde_json::{json, Map, Value};

use crate::theme::{self, icon};

/// Automatic reports per session (a render loop error must not send thousands)
const MAX_AUTO: usize = 5;
/// Log lines sent along with a report
const TAIL: usize = 60;
/// Upper bound for one send; the panic hook waits at most this long
const TIMEOUT: Duration = Duration::from_secs(2);
/// Log marker: the crash of this session already went out (the next start does not send it again)
pub const SENT: &str = "Fehlerbericht gesendet";

static ENABLED: AtomicBool = AtomicBool::new(false);
/// 0 = unknown, 1 = start page, 2 = editor
static SCREEN: AtomicU8 = AtomicU8::new(0);
static TAIL_LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static SYSTEM: Mutex<Vec<String>> = Mutex::new(Vec::new());
static AUTO: Mutex<Limiter> = Mutex::new(Limiter { seen: None, count: 0 });
/// Crash of the previous session (set by the log thread at startup)
static PREV: Mutex<Option<Report>> = Mutex::new(None);
static CRASH: Mutex<CrashSend> = Mutex::new(CrashSend { busy: false, sent: false, auto_tried: false });

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------- DSN and transport ----------

pub struct Dsn {
    endpoint: String,
    auth: String,
}

/// `https://<key>@<host>[/<prefix>]/<project>` → envelope endpoint and auth header
pub fn parse_dsn(s: &str) -> Option<Dsn> {
    let (scheme, rest) = s.trim().split_once("://")?;
    let (key, rest) = rest.split_once('@')?;
    let key = key.split(':').next().unwrap_or_default();
    let (host, project) = rest.trim_end_matches('/').rsplit_once('/')?;
    if !matches!(scheme, "http" | "https") || key.is_empty() || host.is_empty() || project.is_empty() {
        return None;
    }
    Some(Dsn {
        endpoint: format!("{scheme}://{host}/api/{project}/envelope/"),
        auth: format!("Sentry sentry_version=7, sentry_key={key}, sentry_client=cutline/{}", env!("CARGO_PKG_VERSION")),
    })
}

fn dsn() -> Option<&'static Dsn> {
    static D: OnceLock<Option<Dsn>> = OnceLock::new();
    D.get_or_init(|| {
        std::env::var("CUTLINE_SENTRY_DSN").ok().or(option_env!("CUTLINE_SENTRY_DSN").map(String::from)).and_then(|s| parse_dsn(&s))
    })
    .as_ref()
}

/// The feature exists in this build (otherwise no setting, no onboarding choice, no button)
pub fn available() -> bool {
    dsn().is_some()
}

pub fn post(dsn: &Dsn, body: &[u8]) -> Result<(), String> {
    ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .build()
        .post(&dsn.endpoint)
        .set("X-Sentry-Auth", &dsn.auth)
        .set("Content-Type", "application/x-sentry-envelope")
        .send_bytes(body)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ---------- scrubbing ----------

/// Who must not appear in a report: home folder, user and machine names
pub struct Who {
    home: String,
    names: Vec<String>,
}

impl Who {
    pub fn new(home: &str, names: &[&str]) -> Self {
        let mut names: Vec<String> = names.iter().map(|n| n.trim().to_string()).collect();
        if let Some(n) = std::path::Path::new(home.trim_end_matches(['/', '\\'])).file_name() {
            names.push(n.to_string_lossy().into_owned());
        }
        // longer names first, so a short one does not cut a longer one apart
        names.retain(|n| n.chars().count() >= 2);
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
        Who { home: home.to_string(), names }
    }

    fn current() -> Self {
        let var = |v: &str| std::env::var(v).unwrap_or_default();
        let home = var(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
        let names = ["USERNAME", "USER", "LOGNAME", "COMPUTERNAME", "HOSTNAME"].map(var);
        Who::new(&home, &names.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// Secrets, home folder, absolute paths (`<path>`), media/project file names (`<file>`),
    /// names in „…“, user and machine names (`<user>`)
    pub fn scrub(&self, s: &str) -> String {
        let s = crate::applog::anon(&crate::applog::tilde(&crate::applog::redact(s), &self.home));
        let s = s.replace("<pfad>", "<path>").replace("<datei>", "<file>");
        self.names.iter().fold(s, |s, n| replace_ci(&s, n, "<user>"))
    }
}

/// Lowercase that keeps every byte position (chars whose lowercase has another length stay)
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut l = c.to_lowercase();
            match (l.next(), l.next()) {
                (Some(x), None) if x.len_utf8() == c.len_utf8() => x,
                _ => c,
            }
        })
        .collect()
}

/// Replace the whole word `pat` ignoring case (`JÖRG` = `jörg`); inside a longer word it stays
/// (the user „max“ must not turn „maximum“ into „<user>imum“)
fn replace_ci(s: &str, pat: &str, with: &str) -> String {
    let (low, pat) = (fold(s), fold(pat));
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let letter = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    for (i, _) in low.match_indices(&pat) {
        if letter(s[..i].chars().next_back()) || letter(s[i + pat.len()..].chars().next()) {
            continue;
        }
        out.push_str(&s[last..i]);
        out.push_str(with);
        last = i + pat.len();
    }
    out.push_str(&s[last..]);
    out
}

/// Source path of a backtrace frame → crate-relative (`crates/…`, `library/…`, `egui-0.31.1/…`)
pub fn src_path(p: &str) -> String {
    let n = p.replace('\\', "/");
    if let Some(i) = n.find("/registry/src/") {
        return n[i + 14..].split_once('/').map_or("<path>".into(), |x| x.1.to_string());
    }
    for m in ["/crates/", "/library/"] {
        if let Some(i) = n.rfind(m) {
            return n[i + 1..].to_string();
        }
    }
    if n.starts_with('/') || n.as_bytes().get(1) == Some(&b':') {
        "<path>".into()
    } else {
        n
    }
}

/// `path:line[:col]` → (crate-relative path, line, column)
fn split_loc(s: &str) -> (String, Option<u64>, Option<u64>) {
    let mut p: Vec<&str> = s.rsplitn(3, ':').collect();
    let num = |x: &str| x.parse::<u64>().ok();
    match p.len() {
        3 if num(p[0]).is_some() && num(p[1]).is_some() => (src_path(p[2]), num(p[1]), num(p[0])),
        _ if p.len() >= 2 && num(p[0]).is_some() => {
            let line = num(p.remove(0));
            p.reverse();
            (src_path(&p.join(":")), line, None)
        }
        _ => (src_path(s), None, None),
    }
}

/// Frames of a `std::backtrace::Backtrace` text, oldest first (as Sentry expects)
pub fn frames(bt: &str, who: &Who) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for line in bt.lines() {
        let t = line.trim();
        if let Some(at) = t.strip_prefix("at ") {
            if let Some(Value::Object(f)) = out.last_mut() {
                let (file, line, col) = split_loc(at);
                f.insert("in_app".into(), json!(file.starts_with("crates/")));
                f.insert("filename".into(), json!(file));
                if let Some(l) = line {
                    f.insert("lineno".into(), json!(l));
                }
                if let Some(c) = col {
                    f.insert("colno".into(), json!(c));
                }
            }
        } else if let Some((n, func)) = t.split_once(": ") {
            if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) {
                out.push(json!({ "function": who.scrub(func) }));
            }
        }
    }
    out.truncate(200);
    out.reverse();
    out
}

// ---------- reports ----------

#[derive(Clone, Default)]
pub struct Report {
    /// Panic or unexpected end (fatal) instead of an error line
    pub panic: bool,
    pub message: String,
    /// Crate-relative `file:line:col` of a panic
    pub location: Option<String>,
    pub thread: Option<String>,
    pub backtrace: String,
    pub area: String,
    /// Last log lines (`LEVEL area: message`)
    pub log: Vec<String>,
    /// Messages of the `system` target (OS, graphics, FFmpeg)
    pub system: Vec<String>,
    pub screen: Option<&'static str>,
    /// „auto“ or „crash-prompt“
    pub source: &'static str,
}

impl Report {
    /// From `PanicHookInfo`'s text (`panicked at file:line:col:\nmessage`)
    pub fn panic(info: &str, thread: Option<&str>, backtrace: &str) -> Self {
        let rest = info.strip_prefix("panicked at ").unwrap_or(info);
        let (location, message) = match rest.split_once(":\n") {
            Some((loc, msg)) => {
                let (f, l, c) = split_loc(loc);
                let pos = [l, c].iter().flatten().map(|n| format!(":{n}")).collect::<String>();
                (Some(format!("{f}{pos}")), msg.to_string())
            }
            None => (None, info.to_string()),
        };
        Report {
            panic: true,
            message,
            location,
            thread: thread.map(String::from),
            backtrace: backtrace.to_string(),
            area: "absturz".into(),
            ..Default::default()
        }
    }

    pub fn error(area: &str, msg: &str) -> Self {
        Report { message: msg.to_string(), area: area.to_string(), ..Default::default() }
    }

    /// Same panic location or same error line = same report
    fn key(&self, who: &Who) -> String {
        match &self.location {
            Some(l) => l.clone(),
            None => format!("{}: {}", self.area, who.scrub(&self.message)),
        }
    }

    /// Log tail, system lines and screen of the running session
    fn with_current(mut self, source: &'static str) -> Self {
        if let Ok(t) = TAIL_LOG.try_lock() {
            self.log = t.iter().cloned().collect();
        }
        if let Ok(s) = SYSTEM.try_lock() {
            self.system = s.clone();
        }
        self.screen = match SCREEN.load(Ordering::Relaxed) {
            1 => Some("home"),
            2 => Some("editor"),
            _ => None,
        };
        self.source = source;
        self
    }
}

/// (OS version, graphics adapter, FFmpeg version) from the `system` log lines
fn system_info(lines: &[String]) -> (Option<String>, Option<String>, Option<String>) {
    let (mut os, mut gpu, mut ff) = (None, None, None);
    for l in lines.iter().flat_map(|l| l.lines()) {
        let l = l.trim();
        if let Some(v) = l.strip_prefix("System: ") {
            os = Some(v.to_string());
        }
        if let Some(v) = l.strip_prefix("Grafik: ") {
            let end = [v.find(" ("), v.find(" · ")].into_iter().flatten().min().unwrap_or(v.len());
            gpu = Some(v[..end].to_string());
        }
        if let Some(i) = l.find("ffmpeg version ") {
            ff = l[i + 15..].split_whitespace().next().map(String::from);
        }
    }
    (os, gpu, ff)
}

/// Sentry event; every free text goes through [`Who::scrub`]
pub fn event(r: &Report, who: &Who, id: &str, timestamp: f64) -> Value {
    let s = |x: &str| who.scrub(x);
    let (os, gpu, ffmpeg) = system_info(&r.system);
    let mut tags = Map::new();
    tags.insert("source".into(), json!(r.source));
    for (k, v) in [("thread", r.thread.as_deref()), ("screen", r.screen), ("location", r.location.as_deref())] {
        if let Some(v) = v {
            tags.insert(k.into(), json!(s(v)));
        }
    }
    let mut os_ctx = json!({ "name": std::env::consts::OS });
    if let Some(v) = os {
        os_ctx["version"] = json!(s(&v));
    }
    let mut contexts = json!({ "os": os_ctx, "device": { "arch": std::env::consts::ARCH } });
    if let Some(g) = gpu {
        contexts["gpu"] = json!({ "name": s(&g) });
    }
    let mut ev = json!({
        "event_id": id,
        "timestamp": timestamp,
        "platform": "native",
        "level": if r.panic { "fatal" } else { "error" },
        "logger": r.area,
        "release": format!("cutline@{}", env!("CARGO_PKG_VERSION")),
        "environment": if cfg!(debug_assertions) { "debug" } else { "release" },
        "tags": tags,
        "contexts": contexts,
        "extra": { "ffmpeg": ffmpeg.map(|f| s(&f)), "log": r.log.iter().map(|l| s(l)).collect::<Vec<_>>() },
    });
    if r.panic {
        let mut ex = json!({ "type": "panic", "value": s(&r.message), "mechanism": { "type": "panic", "handled": false } });
        let fr = frames(&r.backtrace, who);
        if !fr.is_empty() {
            ex["stacktrace"] = json!({ "frames": fr });
        }
        ev["exception"] = json!({ "values": [ex] });
    } else {
        ev["message"] = json!({ "formatted": s(&r.message) });
    }
    ev
}

pub fn envelope(ev: &Value) -> Vec<u8> {
    let body = ev.to_string();
    format!("{}\n{}\n{body}\n", json!({ "event_id": ev["event_id"] }), json!({ "type": "event", "length": body.len() })).into_bytes()
}

fn event_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn send_report(dsn: &Dsn, r: &Report, who: &Who) -> bool {
    // the reason goes to the log (a DNS blocker is a common one); a warning, so it is not reported itself
    post(dsn, &envelope(&event(r, who, &event_id(), now_secs()))).map_err(|e| log::warn!(target: "bericht", "nicht gesendet: {e}")).is_ok()
}

// ---------- automatic reports ----------

#[derive(Default)]
pub struct Limiter {
    seen: Option<HashSet<String>>,
    count: usize,
}

impl Limiter {
    fn admit(&mut self, key: String) -> bool {
        if self.count >= MAX_AUTO || !self.seen.get_or_insert_with(HashSet::new).insert(key) {
            return false;
        }
        self.count += 1;
        true
    }
}

/// Start sending an automatic report if allowed (DSN, setting on, under the limit, not a
/// duplicate). `None`: nothing leaves the machine.
pub fn auto(dsn: Option<&'static Dsn>, enabled: bool, lim: &Mutex<Limiter>, r: Report, who: Who) -> Option<JoinHandle<bool>> {
    let dsn = dsn.filter(|_| enabled)?;
    // try_lock: the panic hook must never wait on a lock the panicking thread might hold
    if !lim.try_lock().ok()?.admit(r.key(&who)) {
        return None;
    }
    std::thread::Builder::new().name("cl-report".into()).spawn(move || send_report(dsn, &r, &who)).ok()
}

/// Every log record (from `applog::Logger`): keeps the tail, sends own errors when switched on
pub fn observe(level: Level, area: &str, msg: &str, own: bool) {
    {
        let mut t = lock(&TAIL_LOG);
        if t.len() >= TAIL {
            t.pop_front();
        }
        t.push_back(format!("{level:<5} {area}: {msg}"));
    }
    if area == "system" {
        lock(&SYSTEM).push(msg.to_string());
    }
    // „meldung“ errors are messages to the user (file unreadable …), not faults of the app
    if level == Level::Error && own && area != "meldung" && ENABLED.load(Ordering::Relaxed) {
        let _ = auto(dsn(), true, &AUTO, Report::error(area, msg).with_current("auto"), Who::current());
    }
}

/// From the panic hook, after the panic is on disk: send at once when switched on (waits at
/// most [`TIMEOUT`]) and mark it as sent. Switched off: returns without any network call.
pub fn on_panic(info: &str, thread: Option<&str>, backtrace: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let r = Report::panic(info, thread, backtrace).with_current("auto");
    if auto(dsn(), true, &AUTO, r, Who::current()).and_then(|h| h.join().ok()) == Some(true) {
        log::info!(target: "sitzung", "{SENT} pid={}", std::process::id());
        crate::applog::flush();
    }
}

// ---------- crash of the previous session ----------

/// Previous session from the log text (last `beginnt` to the end): its last crash (or an
/// unexpected end without a panic) and whether that already went out.
pub fn previous(text: &str) -> Option<(Report, bool)> {
    let at = text.rfind(" sitzung: beginnt pid=")?;
    let block = &text[text[..at].rfind('\n').map_or(0, |i| i + 1)..];
    let mut entries: Vec<String> = Vec::new();
    for line in block.lines() {
        match entries.last_mut() {
            Some(e) if crate::applog::parse_stamp(line).is_none() => {
                e.push('\n');
                e.push_str(line.strip_prefix("    ").unwrap_or(line));
            }
            _ => entries.push(line.to_string()),
        }
    }
    let (mut crash, mut sent, mut log, mut system) = (None, false, Vec::new(), Vec::new());
    for e in &entries {
        // without the timestamp: `LEVEL area: message`
        let body = e.get(31..).unwrap_or_default();
        if let Some(i) = body.find(" absturz: Absturz pid=") {
            let rest = &body[i..];
            let thread = rest.split_once('„').and_then(|x| x.1.split_once('“')).map(|x| x.0);
            let info = rest.split_once("“: ").map_or("", |x| x.1);
            // the backtrace starts at the first `N: ` line
            let lines: Vec<&str> = info.lines().collect();
            let k = lines
                .iter()
                .position(|l| l.trim().split_once(": ").is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit())))
                .unwrap_or(lines.len());
            crash = Some(Report::panic(&lines[..k].join("\n"), thread, &lines[k..].join("\n")));
            sent = false;
        } else if body.contains(&format!(" sitzung: {SENT} pid=")) {
            sent = true;
        } else {
            if let Some(m) = body.split_once(" system: ").map(|x| x.1) {
                system.push(m.to_string());
            }
            log.push(body.to_string());
        }
    }
    let mut r = crash.unwrap_or_else(|| Report {
        panic: true,
        message: "unexpected exit without a recorded panic".into(),
        area: "sitzung".into(),
        ..Default::default()
    });
    r.log = log.split_off(log.len().saturating_sub(TAIL));
    r.system = system;
    r.source = "crash-prompt";
    Some((r, sent))
}

/// From the log thread at startup, when the last session ended unexpectedly
pub fn set_previous(text: &str) {
    if let Some((r, sent)) = previous(text) {
        *lock(&PREV) = Some(r);
        lock(&CRASH).sent = sent;
    }
}

/// State of „An Entwickler senden“: one send at a time, never twice after success
#[derive(Default)]
pub struct CrashSend {
    busy: bool,
    sent: bool,
    auto_tried: bool,
}

impl CrashSend {
    pub fn start(&mut self) -> bool {
        if self.busy || self.sent {
            return false;
        }
        self.busy = true;
        true
    }
    pub fn finish(&mut self, ok: bool) {
        self.busy = false;
        self.sent |= ok;
    }
}

fn send_crash(app: &mut crate::app::App, ctx: &egui::Context, dsn: &'static Dsn, r: Report, loud: bool) {
    if !lock(&CRASH).start() {
        return;
    }
    app.tasks.spawn(ctx, "crash-report", crate::tr("Bericht wird gesendet"), move |_, _| {
        let ok = send_report(dsn, &r, &Who::current());
        crate::tasks::TaskDone::Run(Box::new(move |app: &mut crate::app::App| {
            lock(&CRASH).finish(ok);
            if !loud {
            } else if ok {
                app.toast(format!("{}  Bericht gesendet", icon::CHECK));
            } else {
                app.toast_err("Bericht konnte nicht gesendet werden");
            }
        }))
    });
}

/// Every frame: follow the setting and the screen; switched on, send the previous crash once
pub fn frame(ctx: &egui::Context, app: &mut crate::app::App) {
    let on = app.settings.send_reports;
    ENABLED.store(on, Ordering::Relaxed);
    SCREEN.store(if app.screen == crate::app::Screen::Editor { 2 } else { 1 }, Ordering::Relaxed);
    if let Some(m) = &app.mcp {
        m.closed.store(app.screen != crate::app::Screen::Editor, Ordering::Relaxed);
    }
    let (Some(dsn), true) = (dsn(), on) else { return };
    if lock(&CRASH).auto_tried {
        return;
    }
    let Some(r) = lock(&PREV).clone() else { return };
    lock(&CRASH).auto_tried = true;
    send_crash(app, ctx, dsn, r, false);
}

/// „An Entwickler senden“ next to „Protokoll exportieren“ (also with the setting off: the click
/// is the consent for this one report)
pub fn crash_button(ui: &mut egui::Ui, app: &mut crate::app::App) {
    let Some(dsn) = dsn() else { return };
    if lock(&PREV).is_none() {
        return;
    }
    let can = {
        let c = lock(&CRASH);
        !c.busy && !c.sent
    };
    if ui.add_enabled(can, theme::secondary("An Entwickler senden")).clicked() {
        if let Some(r) = lock(&PREV).clone() {
            send_crash(app, ui.ctx(), dsn, r, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::Arc;

    /// Local fake Sentry: collects the raw bytes of every request, answers with `status`
    struct Fake {
        got: Arc<Mutex<Vec<Vec<u8>>>>,
        dsn: &'static Dsn,
    }

    fn fake(status: u16) -> Fake {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let got: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        let g = got.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // headers, then as many body bytes as Content-Length says
                loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= h + 4 + len {
                            break;
                        }
                    }
                }
                // recorded before answering: once the client has its answer, the bytes are here
                g.lock().unwrap().push(buf);
                let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
            }
        });
        let dsn = Box::leak(Box::new(parse_dsn(&format!("http://publickey@127.0.0.1:{port}/42")).unwrap()));
        Fake { got, dsn }
    }

    fn who() -> Who {
        Who::new(r"C:\Users\Jörg Müller", &["Jörg Müller", "JOERG-PC"])
    }

    const BT: &str = "   0: std::backtrace_rs::backtrace::dbghelp64::trace\n             at /rustc/90b35a6239c3d8bdabc530a6a0816f7ff89a0aaf/library/std/src/../../backtrace/src/backtrace/dbghelp64.rs:91\n   1: cl_app::timeline::draw\n             at C:\\Users\\Jörg Müller\\cw\\cutline\\crates\\cl-app\\src\\timeline.rs:120:9\n   2: egui::context::Context::run\n             at C:\\Users\\Jörg Müller\\.cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f\\egui-0.31.1\\src\\context.rs:854:9\n   3: main\n";

    fn planted() -> Report {
        let mut r = Report::panic(
            "panicked at crates/cl-app/src/timeline.rs:120:9:\nclip „Urlaub Mallorca“ missing in C:\\Users\\Jörg Müller\\Videos\\Urlaub Mallorca.cutline",
            Some("main"),
            BT,
        );
        r.log = vec![
            r#"INFO  export: ffmpeg -y -i "C:\Users\Jörg Müller\Videos\Urlaub Mallorca\clip 1.mp4" -c:v libx264 /home/joerg/out.mp4"#.into(),
            r"WARN  import: \\NAS-01\Medien\Urlaub Mallorca\IMG_0001.MOV: Invalid data".into(),
            "INFO  projekt: geöffnet in 12 ms: 3 Spuren · 9 Clips · 4 Medien · 61.0 s".into(),
            "INFO  sitzung: Rechner JOERG-PC, Benutzer jörg müller".into(),
        ];
        r.system = vec!["Grafik: NVIDIA GeForce GTX 1070 (Vulkan, DiscreteGpu, Treiber 566.36) · Skalierung: 1.00".into(), "Cutline 0.18.3 (Release, windows x86_64)\nSystem: Microsoft Windows [Version 10.0.26300.7171]\nFFmpeg: C:\\Users\\Jörg Müller\\ffmpeg\\ffmpeg.exe · ffmpeg version 8.0-full_build-www.gyan.dev Copyright".into()];
        r.source = "auto";
        r
    }

    fn assert_clean(raw: &[u8]) {
        let raw = String::from_utf8_lossy(raw).to_lowercase();
        for bad in ["jörg", "jorg", "müller", "joerg", "mallorca", "urlaub", "img_0001", "nas-01", "videos", "clip 1", r"c:\\users", "/home/"] {
            assert!(!raw.contains(bad), "{bad} leaked:\n{raw}");
        }
    }

    #[test]
    fn scrubber_removes_paths_names_and_users() {
        let w = who();
        let cases = [
            (r"Fehler in C:\Users\Jörg Müller\Videos\Urlaub Mallorca.cutline", "Fehler in <path>.cutline"),
            ("open /home/joerg/Videos/Urlaub Mallorca/a b.mp4: denied", "open <path>.mp4: denied"),
            (r"lesen: \\NAS-01\Medien\Ordner mit Leerzeichen\x.mov", "lesen: <path>.mov"),
            (r#"ffmpeg -i "C:\Users\Jörg Müller\Desktop\Ä Ö ü.wav" -f null -"#, r#"ffmpeg -i "<path>.wav" -f null -"#),
            ("proxy: urlaub.mp4 fertig in 12 ms", "proxy: <file>.mp4 fertig in 12 ms"),
            ("Projekt „Urlaub Mallorca“ gespeichert", "Projekt „***“ gespeichert"),
            ("Rechner joerg-pc, Benutzer JÖRG MÜLLER", "Rechner <user>, Benutzer <user>"),
            // bug (review of 0.19.0): a name inside a longer word garbled the text
            ("joerg-pcs und Jörg Müllers Maximum", "joerg-pcs und Jörg Müllers Maximum"),
            ("Authorization: Bearer abcdefghijklmnop", "Authorization: Bearer ***"),
        ];
        for (input, want) in cases {
            assert_eq!(w.scrub(input), want, "{input}");
        }
        // code paths and numbers stay readable
        for s in ["crates/cl-app/src/timeline.rs:120:9", "10.0.26300.7171", "Cutline 0.18.3 (Release)"] {
            assert_eq!(w.scrub(s), s);
        }
        assert_eq!(src_path(r"C:\Users\Jörg Müller\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\egui-0.31.1\src\context.rs"), "egui-0.31.1/src/context.rs");
        assert_eq!(src_path("/home/joerg/cutline/crates/cl-app/src/app.rs"), "crates/cl-app/src/app.rs");
        assert_eq!(src_path("/rustc/abc/library/core/src/panicking.rs"), "library/core/src/panicking.rs");
        assert_eq!(src_path("/home/joerg/elsewhere/x.rs"), "<path>");
    }

    #[test]
    fn panic_event_has_frames_and_no_personal_data() {
        let w = who();
        let ev = event(&planted(), &w, "0123456789abcdef0123456789abcdef", 1.5);
        let ex = &ev["exception"]["values"][0];
        assert_eq!(ex["type"], "panic");
        assert_eq!(ev["tags"]["location"], "crates/cl-app/src/timeline.rs:120:9");
        let fr = ex["stacktrace"]["frames"].as_array().unwrap();
        assert_eq!(fr.len(), 4);
        assert_eq!(fr[0]["function"], "main");
        assert_eq!(fr[1]["filename"], "egui-0.31.1/src/context.rs");
        assert_eq!(fr[2]["filename"], "crates/cl-app/src/timeline.rs");
        assert_eq!(fr[2]["in_app"], true);
        assert_eq!(fr[2]["lineno"], 120);
        assert_eq!(ev["contexts"]["gpu"]["name"], "NVIDIA GeForce GTX 1070");
        assert_eq!(ev["contexts"]["os"]["version"], "Microsoft Windows [Version 10.0.26300.7171]");
        assert_eq!(ev["extra"]["ffmpeg"], "8.0-full_build-www.gyan.dev");
        assert!(ev.get("user").is_none() && ev.get("server_name").is_none());
        assert_clean(&envelope(&ev));
    }

    #[test]
    fn switched_off_sends_nothing() {
        let f = fake(200);
        let lim = Mutex::new(Limiter::default());
        assert!(auto(Some(f.dsn), false, &lim, planted(), who()).is_none());
        assert!(auto(Some(f.dsn), false, &lim, Report::error("export", "boom"), who()).is_none());
        // no DSN: nothing, even when switched on
        assert!(auto(None, true, &lim, planted(), who()).is_none());
        // a build without a DSN shows nothing (setting, onboarding choice, button all ask this)
        if std::env::var_os("CUTLINE_SENTRY_DSN").is_none() && option_env!("CUTLINE_SENTRY_DSN").is_none() {
            assert!(!available());
        }
        assert_eq!(f.got.lock().unwrap().len(), 0);
        // the panic hook path with the setting off (default) returns before any send
        on_panic("panicked at crates/x.rs:1:1:\nboom", Some("main"), BT);
        assert_eq!(f.got.lock().unwrap().len(), 0);
    }

    #[test]
    fn switched_on_sends_each_error_once_within_the_limit() {
        let f = fake(200);
        let lim = Mutex::new(Limiter::default());
        let send = |r: Report| auto(Some(f.dsn), true, &lim, r, who()).map(|h| h.join().unwrap());
        assert_eq!(send(planted()), Some(true));
        // same panic location again (render loop): not again
        assert_eq!(send(planted()), None);
        for i in 0..20 {
            send(Report::error("export", &format!("Fehler {i}")));
        }
        let got = f.got.lock().unwrap();
        assert_eq!(got.len(), MAX_AUTO);
        let raw = String::from_utf8_lossy(&got[0]);
        assert!(raw.starts_with("POST /api/42/envelope/ HTTP/1.1"), "{raw}");
        assert!(raw.contains("sentry_key=publickey"), "{raw}");
        assert!(raw.contains(r#""type":"event"}"#), "{raw}");
        for g in got.iter() {
            assert_clean(g);
        }
    }

    const LOG: &str = "2026-11-25 10:00:00.000 +01:00 INFO  sitzung: beginnt pid=11 · Cutline 0.18.2 (Release)\n\
2026-11-25 10:00:01.000 +01:00 INFO  sitzung: endet sauber pid=11\n\
2026-11-26 09:00:00.000 +01:00 INFO  sitzung: beginnt pid=22 · Cutline 0.18.3 (Release)\n\
2026-11-26 09:00:00.100 +01:00 INFO  system: Grafik: NVIDIA GeForce GTX 1070 (Vulkan, DiscreteGpu, Treiber 566.36) · Skalierung: 1.00\n\
2026-11-26 09:00:01.000 +01:00 INFO  projekt: geöffnet in 12 ms: 3 Spuren · 9 Clips\n\
2026-11-26 09:00:02.000 +01:00 WARN  import: C:\\Users\\Jörg Müller\\Videos\\Urlaub Mallorca.mp4: Invalid data\n\
2026-11-26 09:00:03.000 +01:00 ERROR absturz: Absturz pid=22 im Thread „main“: panicked at crates/cl-app/src/timeline.rs:120:9:\n    index out of bounds\n       0: cl_app::timeline::draw\n                 at C:\\Users\\Jörg Müller\\cw\\crates\\cl-app\\src\\timeline.rs:120:9\n       1: main\n";

    #[test]
    fn previous_crash_is_read_from_the_log() {
        let (r, sent) = previous(LOG).unwrap();
        assert!(!sent && r.panic);
        assert_eq!(r.message, "index out of bounds");
        assert_eq!(r.location.as_deref(), Some("crates/cl-app/src/timeline.rs:120:9"));
        assert_eq!(r.thread.as_deref(), Some("main"));
        assert!(r.backtrace.contains("cl_app::timeline::draw"), "{}", r.backtrace);
        assert_eq!(r.log.len(), 4, "{:?}", r.log);
        assert!(r.log[0].starts_with("INFO  sitzung: beginnt pid=22"));
        // a crash already sent at panic time is not sent again
        let marked = format!("{LOG}2026-11-26 09:00:04.000 +01:00 INFO  sitzung: {SENT} pid=22\n");
        assert!(previous(&marked).unwrap().1);
        // killed without a panic: still a report, with the log tail
        let killed = LOG.split("2026-11-26 09:00:03").next().unwrap();
        let (r, _) = previous(killed).unwrap();
        assert!(r.message.contains("unexpected exit") && r.backtrace.is_empty() && r.log.len() == 4);
    }

    #[test]
    fn crash_prompt_sends_exactly_once_and_retries_after_failure() {
        let (crash, _) = previous(LOG).unwrap();
        // setting off does not matter: the click is the consent
        let bad = fake(500);
        let mut st = CrashSend::default();
        assert!(st.start());
        assert!(!st.start(), "no second send while one is running");
        st.finish(send_report(bad.dsn, &crash, &who()));
        assert_eq!(bad.got.lock().unwrap().len(), 1);
        let ok = fake(200);
        assert!(st.start(), "after a failure the button works again");
        st.finish(send_report(ok.dsn, &crash, &who()));
        assert!(!st.start(), "after success no second click");
        let got = ok.got.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_clean(&got[0]);
        let raw = String::from_utf8_lossy(&got[0]);
        let body = &raw[raw.find("\r\n\r\n").unwrap() + 4..];
        assert!(body.contains("index out of bounds") && body.contains("crash-prompt"), "{body}");
        println!("{body}");
    }

    #[test]
    fn dsn_parsing() {
        let d = parse_dsn("https://abc123@o1.ingest.sentry.io/4507").unwrap();
        assert_eq!(d.endpoint, "https://o1.ingest.sentry.io/api/4507/envelope/");
        assert!(d.auth.contains("sentry_key=abc123"));
        assert_eq!(parse_dsn("http://k@127.0.0.1:9000/sub/7").unwrap().endpoint, "http://127.0.0.1:9000/sub/api/7/envelope/");
        for bad in ["", "   ", "https://o1.ingest.sentry.io/4507", "ftp://k@h/1", "https://k@h/"] {
            assert!(parse_dsn(bad).is_none(), "{bad}");
        }
    }
}
