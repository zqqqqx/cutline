//! Protokoll der App: eine Datei je Tag unter `<Datenordner>/logs/` (`cutline-JJJJ-MM-TT.log`),
//! Export der letzten 24 Stunden als eine Textdatei. Geschrieben wird über einen Kanal von einem
//! eigenen Thread, der Oberflächen-Thread wartet nie auf die Platte. Angeschlossen an die
//! `log`-Fassade, damit auch Warnungen von Grafik und Fenster hier landen.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, RichText};
use log::Level;

use crate::theme::{self, icon};

/// Dateien älter als so viele Tage löscht der Start
pub const KEEP_DAYS: i64 = 7;
/// Ab dieser Größe nimmt eine Tagesdatei nur noch Warnungen und Fehler auf (beim Doppelten nichts mehr)
pub const FILE_LIMIT: u64 = 20 * 1024 * 1024;

const START: &str = "beginnt";
const END: &str = "endet sauber";
const PANIC: &str = "Absturz";
const LIMIT_NOTE: &str = "Obergrenze der Datei erreicht – ab hier nur noch Warnungen und Fehler";
const DAY_MS: i64 = 86_400_000;
/// Bereiche, die Cutline selbst mit `target:` benutzt (alles mit `cutline::…` gehört ohnehin dazu)
const OWN: &[&str] = &["sitzung", "system", "perf", "projekt", "export", "proxy", "update", "paket", "import", "meldung"];

pub struct Rec {
    pub ms: i64,
    pub level: Level,
    pub area: String,
    pub msg: String,
}

pub enum Msg {
    Rec(Rec),
    /// Antwort, sobald alles davor verarbeitet ist; `true` = auch auf die Platte bringen
    Sync(Sender<()>, bool),
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();
static DIR: OnceLock<PathBuf> = OnceLock::new();
/// Der letzte Lauf endete ohne „endet sauber“ oder mit einer Panic
static CRASHED: AtomicBool = AtomicBool::new(false);
static LAST_EXPORT: Mutex<Option<PathBuf>> = Mutex::new(None);
static UI_INFO: OnceLock<String> = OnceLock::new();
const THREAD: &str = "cl-log";

// ---------- Zeit ----------

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn offset() -> i64 {
    crate::project_tools::local_offset_secs()
}

/// Tage seit 1970 → (Jahr, Monat, Tag), proleptischer Gregorianischer Kalender
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `2026-11-15 14:03:22.123 +01:00` (Ortszeit mit Versatz zu UTC)
pub fn stamp(ms: i64, off: i64) -> String {
    let l = ms + off * 1000;
    let (y, mo, d) = civil(l.div_euclid(DAY_MS));
    let t = l.rem_euclid(DAY_MS);
    let (sign, o) = if off < 0 { ('-', -off) } else { ('+', off) };
    format!(
        "{y:04}-{mo:02}-{d:02} {:02}:{:02}:{:02}.{:03} {sign}{:02}:{:02}",
        t / 3_600_000,
        t / 60_000 % 60,
        t / 1000 % 60,
        t % 1000,
        o / 3600,
        o / 60 % 60
    )
}

/// Zeitstempel am Zeilenanfang → ms seit 1970 (UTC). Kaputte Zeilen: `None`.
pub fn parse_stamp(line: &str) -> Option<i64> {
    let s = line.get(..30)?;
    let b = s.as_bytes();
    let num = |a: usize, n: usize| -> Option<i64> {
        let t = s.get(a..a + n)?;
        t.bytes().all(|c| c.is_ascii_digit()).then(|| t.parse().ok())?
    };
    let seps = [(4, b'-'), (7, b'-'), (10, b' '), (13, b':'), (16, b':'), (19, b'.'), (23, b' '), (27, b':')];
    if seps.iter().any(|&(i, c)| b[i] != c) || !matches!(b[24], b'+' | b'-') {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, se, ms) = (num(11, 2)?, num(14, 2)?, num(17, 2)?, num(20, 3)?);
    let off = (num(25, 2)? * 3600 + num(28, 2)? * 60) * if b[24] == b'-' { -1 } else { 1 };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let local = days_from_civil(y, mo, d) * DAY_MS + ((h * 60 + mi) * 60 + se) * 1000 + ms;
    Some(local - off * 1000)
}

/// Tagesdatei für diesen Zeitpunkt (Ortszeit)
pub fn file_name(ms: i64, off: i64) -> String {
    format!("cutline-{}.log", &stamp(ms, off)[..10])
}

/// Tag (seit 1970) aus `cutline-JJJJ-MM-TT.log`
fn file_day(name: &str) -> Option<i64> {
    let d = name.strip_prefix("cutline-")?.strip_suffix(".log")?;
    let p: Vec<i64> = d.split('-').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    (p.len() == 3 && d.len() == 10).then(|| days_from_civil(p[0], p[1], p[2]))
}

// ---------- Unkenntlich machen ----------

/// Schlüssel, deren Wert in JSON (`"schlüssel": "wert"`) nie im Protokoll steht
const JSON_KEYS: &[&str] = &[
    "access_token", "refresh_token", "id_token", "client_secret", "api_key", "apikey", "password", "token", "secret",
    "authorization", "signature",
];
/// Parameter in Adressen und Befehlszeilen (`?token=…`, `key=…`)
const QUERY_KEYS: &[&str] = &[
    "access_token", "refresh_token", "id_token", "client_secret", "api_key", "apikey", "password", "token", "secret",
    "signature", "key", "sig", "code",
];
/// Diese nur direkt hinter `?`/`&` (sonst träfe „exit code=1“)
const QUERY_ONLY: &[&str] = &["sig", "code"];

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn starts_ci(b: &[u8], i: usize, pat: &str) -> bool {
    b.get(i..i + pat.len()).is_some_and(|s| s.eq_ignore_ascii_case(pat.as_bytes()))
}

fn skip(b: &[u8], mut i: usize, ok: impl Fn(u8) -> bool) -> usize {
    while i < b.len() && ok(b[i]) {
        i += 1;
    }
    i
}

fn token_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"-_.~+/=".contains(&c)
}

/// Geheimnis ab Byte `i`: (Ende des sichtbaren Teils, Ende des Werts)
fn secret_at(b: &[u8], i: usize) -> Option<(usize, usize)> {
    let boundary = i == 0 || !is_word(b[i - 1]);
    if starts_ci(b, i, "epidemic_live_") {
        let v = i + 14;
        let e = skip(b, v, |c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-');
        return (e > v).then_some((v, e));
    }
    if !boundary {
        return None;
    }
    // Adressen: der ganze Abfrageteil (signierte Links: `jwt=`, `X-Amz-…`, Anbieter-Parameter)
    if starts_ci(b, i, "http://") || starts_ci(b, i, "https://") {
        let mut e = skip(b, i, |c| !c.is_ascii_whitespace() && !b"\"'<>".contains(&c));
        if b[e - 1] == b':' {
            e -= 1;
        }
        let q = b[i..e].iter().position(|&c| c == b'?')? + i + 1;
        return (e > q).then_some((q, e));
    }
    if starts_ci(b, i, "bearer ") {
        let v = skip(b, i + 7, |c| c == b' ');
        let e = skip(b, v, token_char);
        // echte Tokens sind lang; „Bearer of bad news“ bleibt
        return (e >= v + 8).then_some((v, e));
    }
    for &key in JSON_KEYS.iter().chain(QUERY_KEYS) {
        let k = i + key.len();
        if !starts_ci(b, i, key) || b.get(k).is_some_and(|&c| is_word(c)) {
            continue;
        }
        let quoted = i > 0 && b[i - 1] == b'"' && b.get(k) == Some(&b'"');
        if quoted && JSON_KEYS.contains(&key) {
            let j = skip(b, k + 1, |c| c == b' ');
            if b.get(j) != Some(&b':') {
                continue;
            }
            let j = skip(b, j + 1, |c| c == b' ');
            if b.get(j) != Some(&b'"') {
                continue;
            }
            let (v, mut e) = (j + 1, j + 1);
            while e < b.len() && b[e] != b'"' {
                e += if b[e] == b'\\' { 2 } else { 1 };
            }
            return Some((v, e.min(b.len())));
        }
        if key == "authorization" && b.get(k) == Some(&b':') {
            let mut v = skip(b, k + 1, |c| c == b' ');
            for scheme in ["basic ", "bearer ", "token "] {
                if starts_ci(b, v, scheme) {
                    v = skip(b, v + scheme.len(), |c| c == b' ');
                }
            }
            let e = skip(b, v, |c| !c.is_ascii_whitespace() && c != b'"' && c != b',');
            return (e > v).then_some((v, e));
        }
        if b.get(k) == Some(&b'=') && QUERY_KEYS.contains(&key) {
            if QUERY_ONLY.contains(&key) && !(i > 0 && matches!(b[i - 1], b'?' | b'&')) {
                continue;
            }
            let v = k + 1;
            let e = skip(b, v, |c| !c.is_ascii_whitespace() && !b"&\"'#,;)<>".contains(&c));
            return (e > v).then_some((v, e));
        }
    }
    None
}

/// Zugangsdaten, Schlüssel und Tokens unkenntlich machen (die einzige Stelle dafür):
/// `epidemic_live_…`, `Bearer …`, `Authorization: …`, Token-Felder in JSON, Adress-Parameter.
pub fn redact(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        // Treffer beginnen immer mit einem ASCII-Zeichen, liegen also auf Zeichengrenzen
        if let Some((v, e)) = secret_at(b, i) {
            out.push_str(&s[copied..v]);
            out.push_str("***");
            copied = e;
            i = e;
        } else {
            i += 1;
        }
    }
    out.push_str(&s[copied..]);
    out
}

/// Benutzerordner durch `~` ersetzen (Groß-/Kleinschreibung und `/` oder `\` egal)
pub fn tilde(text: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.len() < 3 {
        return text.to_string();
    }
    let norm = |s: &str| s.to_ascii_lowercase().replace('\\', "/");
    // gleiche Länge wie `text`: Treffer liegen an denselben Bytes
    let (low, pat) = (norm(text), norm(home));
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in low.match_indices(&pat) {
        if i >= last {
            out.push_str(&text[last..i]);
            out.push('~');
            last = i + pat.len();
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Endungen, an denen ein einzelner Dateiname (ohne Ordner) in einer Meldung erkannt wird
const NAME_EXTS: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mts", "m2ts", "wmv", "flv", "3gp", "mxf", "mpg", "mpeg", "ts", "mp3", "wav",
    "m4a", "aac", "flac", "ogg", "opus", "wma", "aif", "aiff", "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff",
    "heic", "svg", "cutline", "srt", "vtt", "ass", "ttf", "otf", "zip", "lut", "cube",
];

/// Einzelne Dateinamen (`kaputt_text.mp4`) werden `<datei>` mit Endung; schon ersetzte (`<pfad>.mp4`)
/// bleiben. shortcut: ein Name mit Leerzeichen verliert nur sein letztes Wort – Meldungen setzen
/// Namen deshalb in „…“.
fn anon_names(s: &str) -> String {
    let sep = |c: char| c.is_whitespace() || "\"'()[]„“,;=".contains(c);
    let mut out = String::with_capacity(s.len());
    let mut start = 0;
    for (i, c) in s.char_indices().chain(std::iter::once((s.len(), ' '))) {
        if !sep(c) {
            continue;
        }
        let w = &s[start..i];
        // „name.mp4: Fehler“ – der Doppelpunkt gehört nicht zum Namen
        let core = w.trim_end_matches([':', '.']);
        match core.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && !core.starts_with('<') && NAME_EXTS.contains(&ext.to_ascii_lowercase().as_str()) => {
                out.push_str("<datei>.");
                out.push_str(ext);
                out.push_str(&w[core.len()..]);
            }
            _ => out.push_str(w),
        }
        if i < s.len() {
            out.push(c);
        }
        start = i + c.len_utf8();
    }
    out
}

/// Inhalte unkenntlich machen (für Meldungen, Export-Fehler und das exportierte Protokoll; Protokolle
/// landen in öffentlichen Issues): Namen in „…“ werden `***`, Pfade (`C:\`, `C:/`, `\\`, `~/`, `~\` oder
/// `/` am Wortanfang, bis Zeilenende oder `: `, ` · `, `"`, `'`, `)`, `“`) werden `<pfad>` mit Endung,
/// einzelne Dateinamen `<datei>` mit Endung.
pub fn anon(s: &str) -> String {
    anon_names(&anon_paths(s))
}

fn anon_paths(s: &str) -> String {
    let mut q = String::with_capacity(s.len());
    let mut parts = s.split('„');
    q.push_str(parts.next().unwrap_or_default());
    for p in parts {
        q.push('„');
        match p.split_once('“') {
            Some((_, rest)) => {
                q.push_str("***“");
                q.push_str(rest);
            }
            None => q.push_str(p),
        }
    }
    let (s, b) = (q.as_str(), q.as_bytes());
    let mut out = String::with_capacity(s.len());
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        let after = |k: usize| b.get(i + k).copied();
        let lead = i == 0 || matches!(b[i - 1], b' ' | b'\t' | b'\n' | b'"' | b'\'' | b'(' | b'=' | b'[');
        let start = lead
            && ((b[i].is_ascii_alphabetic() && after(1) == Some(b':') && matches!(after(2), Some(b'\\' | b'/')))
                || (b[i] == b'\\' && after(1) == Some(b'\\'))
                || (b[i] == b'~' && matches!(after(1), Some(b'\\' | b'/')))
                || (b[i] == b'/' && after(1).is_some_and(|c| is_word(c) || c == b'.')));
        if !start {
            i += 1;
            continue;
        }
        let mut e = i + 1;
        while e < b.len()
            && !matches!(b[e], b'\n' | b'\r' | b'"' | b'\'' | b')')
            && !(b[e] == b':' && matches!(b.get(e + 1), None | Some(b' ')))
            && !b[e..].starts_with("“".as_bytes())
            && !b[e..].starts_with(" · ".as_bytes())
        {
            e += 1;
        }
        let name = s[i..e].trim_end().rsplit(['/', '\\']).next().unwrap_or_default();
        let ext = name.rsplit_once('.').map(|x| x.1).filter(|x| (1..=8).contains(&x.len()) && x.bytes().all(|c| c.is_ascii_alphanumeric()));
        out.push_str(&s[copied..i]);
        out.push_str("<pfad>");
        if let Some(x) = ext {
            out.push('.');
            out.push_str(x);
        }
        (i, copied) = (e, e);
    }
    out.push_str(&s[copied..]);
    out
}

fn home() -> String {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(var).unwrap_or_default()
}

// ---------- Schreiben ----------

/// Was mit einer Zeile bei dieser Dateigröße passiert: (Hinweis davor schreiben, Zeile schreiben)
pub fn admit(size: u64, limit: u64, level: Level, noted: bool) -> (bool, bool) {
    if size < limit {
        (false, true)
    } else if size >= limit.saturating_mul(2) {
        (false, false)
    } else {
        (!noted, level <= Level::Warn)
    }
}

pub fn format_line(r: &Rec, off: i64) -> String {
    let msg = r.msg.trim_end().replace("\r\n", "\n").replace('\n', "\n    ");
    format!("{} {:<5} {}: {}\n", stamp(r.ms, off), r.level, r.area, msg)
}

struct Open {
    name: String,
    file: std::io::BufWriter<std::fs::File>,
    size: u64,
    noted: bool,
}

struct Writer {
    dir: PathBuf,
    off: i64,
    limit: u64,
    open: Option<Open>,
    /// letzte Zeile und wie oft sie seitdem gleich wiederkam
    last: Option<(Rec, u32)>,
}

impl Writer {
    fn handle(&mut self, mut r: Rec) {
        r.msg = redact(&r.msg);
        if let Some((l, n)) = &mut self.last {
            if l.level == r.level && l.area == r.area && l.msg == r.msg {
                *n += 1;
                l.ms = r.ms;
                return;
            }
        }
        self.repeats();
        self.write(&r);
        if r.level == Level::Error {
            self.flush();
        }
        self.last = Some((r, 0));
    }

    /// „… 37× wiederholt“ für die zuletzt zusammengefasste Zeile
    fn repeats(&mut self) {
        if let Some((l, n)) = &mut self.last {
            if *n > 0 {
                let r = Rec { ms: l.ms, level: l.level, area: l.area.clone(), msg: format!("… {n}× wiederholt") };
                *n = 0;
                self.write(&r);
            }
        }
    }

    fn write(&mut self, r: &Rec) {
        let name = file_name(r.ms, self.off);
        if self.open.as_ref().is_none_or(|o| o.name != name) {
            self.flush();
            let _ = std::fs::create_dir_all(&self.dir);
            self.open = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(&name)).ok().map(|f| {
                let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                Open { name, file: std::io::BufWriter::with_capacity(64 * 1024, f), size, noted: size >= self.limit }
            });
        }
        // Schreibfehler (Platte voll, kein Recht): Zeile fällt weg, die App läuft weiter
        let Some(o) = &mut self.open else { return };
        let (note, line) = admit(o.size, self.limit, r.level, o.noted);
        let mut text = String::new();
        if note {
            o.noted = true;
            text += &format_line(&Rec { ms: r.ms, level: Level::Warn, area: "sitzung".into(), msg: LIMIT_NOTE.into() }, self.off);
        }
        if line {
            text += &format_line(r, self.off);
        }
        if o.file.write_all(text.as_bytes()).is_ok() {
            o.size += text.len() as u64;
        }
    }

    fn flush(&mut self) {
        if let Some(o) = &mut self.open {
            let _ = o.file.flush();
        }
    }
}

/// Beim Start: alte Dateien löschen (nach dem Datum im Namen)
pub fn prune(dir: &Path, today: i64) -> usize {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| file_day(&e.file_name().to_string_lossy()).is_some_and(|d| d < today - KEEP_DAYS))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Protokolldateien im Ordner, nach Datum sortiert
fn log_files(dir: &Path) -> Vec<(i64, PathBuf)> {
    let mut v: Vec<(i64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((file_day(&e.file_name().to_string_lossy())?, e.path())))
        .collect();
    v.sort();
    v
}

fn marker_pid(line: &str, area: &str, what: &str) -> Option<u32> {
    let rest = &line[line.find(&format!(" {area}: {what} pid="))? + area.len() + what.len() + 8..];
    rest[..rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len())].parse().ok()
}

/// Endete der letzte Lauf unerwartet? (kein „endet sauber“ nach seinem Start oder eine Panic;
/// ein noch laufendes zweites Fenster zählt nicht)
pub fn unclean_exit(text: &str, alive: impl Fn(u32) -> bool) -> bool {
    let mut last: Option<(u32, bool, bool)> = None;
    for line in text.lines() {
        if let Some(pid) = marker_pid(line, "sitzung", START) {
            last = Some((pid, false, false));
        } else if let Some((pid, clean, panic)) = &mut last {
            if marker_pid(line, "sitzung", END) == Some(*pid) {
                *clean = true;
            } else if marker_pid(line, "absturz", PANIC) == Some(*pid) {
                *panic = true;
            }
        }
    }
    last.is_some_and(|(pid, clean, panic)| (panic || !clean) && !alive(pid))
}

/// Schreib-Thread starten. `check`: vorher den letzten Lauf prüfen und alte Dateien löschen.
pub fn spawn(dir: PathBuf, limit: u64, check: bool) -> Sender<Msg> {
    let (tx, rx) = channel();
    let off = offset();
    let w = Writer { dir, off, limit, open: None, last: None };
    let _ = std::thread::Builder::new().name(THREAD.into()).spawn(move || run(rx, w, check));
    tx
}

fn run(rx: Receiver<Msg>, mut w: Writer, check: bool) {
    if check {
        prune(&w.dir, (now_ms() + w.off * 1000).div_euclid(DAY_MS));
        let files = log_files(&w.dir);
        let text: String = files.iter().rev().take(2).rev().filter_map(|(_, p)| std::fs::read(p).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).collect();
        CRASHED.store(unclean_exit(&text, crate::paths::pid_alive), Ordering::Relaxed);
    }
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Msg::Rec(r)) => w.handle(r),
            Ok(Msg::Sync(ack, flush)) => {
                if flush {
                    w.repeats();
                    w.flush();
                }
                let _ = ack.send(());
            }
            Err(RecvTimeoutError::Timeout) => {
                w.repeats();
                w.flush();
            }
            Err(RecvTimeoutError::Disconnected) => {
                w.repeats();
                w.flush();
                return;
            }
        }
    }
}

fn sync(tx: &Sender<Msg>, flush: bool, wait: Duration) {
    let (a, b) = channel();
    if tx.send(Msg::Sync(a, flush)).is_ok() {
        let _ = b.recv_timeout(wait);
    }
}

/// Alles bisher Gemeldete auf die Platte bringen (wartet höchstens 2 s)
pub fn flush() {
    if let Some(tx) = TX.get() {
        if std::thread::current().name() != Some(THREAD) {
            sync(tx, true, Duration::from_secs(2));
        }
    }
}

struct Logger;

fn own(target: &str) -> bool {
    target.starts_with("cutline") || OWN.contains(&target)
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= Level::Warn || (m.level() <= Level::Info && own(m.target()))
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let area = r.target().strip_prefix("cutline::").unwrap_or(r.target());
        let msg = r.args().to_string();
        // bisherige Fehlerausgaben bleiben in der Konsole sichtbar
        if r.level() <= Level::Warn && own(r.target()) {
            eprintln!("{area}: {msg}");
        }
        if let Some(tx) = TX.get() {
            let _ = tx.send(Msg::Rec(Rec { ms: now_ms(), level: r.level(), area: area.into(), msg }));
        }
    }
    fn flush(&self) {
        flush();
    }
}

static LOGGER: Logger = Logger;

/// Protokoll für das Programm einrichten (Schreib-Thread, `log`-Fassade, Panic-Haken)
pub fn init() {
    init_at(crate::paths::logs_dir(), true);
    log::info!(target: "sitzung", "{START} pid={} · Cutline {} ({})", std::process::id(), env!("CARGO_PKG_VERSION"), build());
}

fn init_at(dir: PathBuf, check: bool) {
    if TX.set(spawn(dir.clone(), FILE_LIMIT, check)).is_err() {
        return;
    }
    let _ = DIR.set(dir);
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        panic_hook(&info.to_string());
        prev(info);
    }));
}

/// Panic ins Protokoll: Meldung, Ort, Rückverfolgung; danach das bisherige Verhalten
fn panic_hook(info: &str) {
    let th = std::thread::current();
    let msg = format!(
        "{PANIC} pid={} im Thread „{}“: {info}\n{}",
        std::process::id(),
        th.name().unwrap_or("?"),
        std::backtrace::Backtrace::force_capture()
    );
    let r = Rec { ms: now_ms(), level: Level::Error, area: "absturz".into(), msg: redact(&msg) };
    if th.name() == Some(THREAD) {
        // der Schreib-Thread selbst: direkt anhängen
        if let Some(dir) = DIR.get() {
            let line = format_line(&r, offset());
            let f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(file_name(r.ms, offset())));
            let _ = f.and_then(|mut f| f.write_all(line.as_bytes()));
        }
    } else if let Some(tx) = TX.get() {
        let _ = tx.send(Msg::Rec(r));
        sync(tx, true, Duration::from_secs(2));
    }
}

fn build() -> &'static str {
    if cfg!(debug_assertions) {
        "Debug"
    } else {
        "Release"
    }
}

/// Sauberes Ende (vor dem Schließen des Fensters)
pub fn session_end() {
    log::info!(target: "sitzung", "{END} pid={}", std::process::id());
    flush();
}

/// Messzeile (`CUTLINE_PERF`): wie bisher nach stderr, zusätzlich ins Protokoll
macro_rules! perf_line {
    ($($t:tt)*) => {{
        let s = format!($($t)*);
        eprintln!("{s}");
        if crate::app::perf() {
            log::info!(target: "perf", "{s}");
        }
    }};
}
pub(crate) use perf_line;

/// Ergebnis eines Vorgangs protokollieren und unverändert zurückgeben
pub fn outcome<T, E: std::fmt::Display>(area: &str, what: &str, t0: std::time::Instant, r: Result<T, E>) -> Result<T, E> {
    let ms = t0.elapsed().as_millis();
    match &r {
        Ok(_) => log::info!(target: area, "{what} fertig in {ms} ms"),
        Err(e) => log::warn!(target: area, "{what} fehlgeschlagen nach {ms} ms: {e}"),
    }
    r
}

/// Meldung als Warnung protokollieren und zurückgeben (für Fehler, die in einem Feld landen)
pub fn warned(area: &str, msg: String) -> String {
    log::warn!(target: area, "{msg}");
    msg
}

/// Kennzahlen eines Projekts ohne Inhalte
pub fn counts(p: &cl_core::Project) -> String {
    let tracks = &p.timeline.tracks;
    format!(
        "{} Spuren · {} Clips · {} Medien · {:.1} s",
        tracks.len(),
        tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
        p.media.len(),
        p.duration().as_secs_f64()
    )
}

/// Projekt geöffnet/gespeichert/geschlossen – nur Kennzahlen, kein Name und kein Pfad
pub fn project(what: &str, p: &cl_core::Project, t0: Option<std::time::Instant>) {
    let ms = t0.map(|t| format!(" in {} ms", t.elapsed().as_millis())).unwrap_or_default();
    log::info!(target: "projekt", "{what}{ms}: {}", counts(p));
}

/// FFmpeg-Befehlszeile fürs Protokoll: Aufbau bleibt, Pfade werden unkenntlich (Texte stehen in Dateien)
pub fn ffmpeg_cmd(args: &[String]) -> String {
    let a: Vec<String> = args.iter().map(|x| anon(x)).collect();
    format!("ffmpeg {}", a.join(" "))
}

// ---------- System ----------

/// Encoder für Hardware aus `ffmpeg -encoders`
pub fn hw_encoders(list: &str) -> Vec<&str> {
    list.lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|n| ["nvenc", "qsv", "amf", "vaapi", "videotoolbox", "_mf", "v4l2m2m"].iter().any(|h| n.contains(h)))
        .collect()
}

fn run_out(c: Option<std::process::Command>) -> String {
    c.and_then(|mut c| c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn os_version() -> String {
    #[cfg(windows)]
    let c = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "ver"]).creation_flags(0x0800_0000); // ohne Konsolenfenster
        c
    };
    #[cfg(not(windows))]
    let c = {
        let mut c = std::process::Command::new("uname");
        c.arg("-srm");
        c
    };
    let v = run_out(Some(c));
    let v = v.trim();
    if v.is_empty() { std::env::consts::OS.to_string() } else { v.to_string() }
}

fn ram_gb() -> Option<f64> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct MemStatus {
            len: u32,
            load: u32,
            total: u64,
            rest: [u64; 6],
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalMemoryStatusEx(m: *mut MemStatus) -> i32;
        }
        let mut m = MemStatus { len: std::mem::size_of::<MemStatus>() as u32, load: 0, total: 0, rest: [0; 6] };
        // SAFETY: Struktur mit gesetzter Länge, nur gelesen
        (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then(|| m.total as f64 / (1u64 << 30) as f64)
    }
    #[cfg(not(windows))]
    {
        let t = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: f64 = t.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / (1u64 << 20) as f64)
    }
}

/// Älteste FFmpeg-Hauptversion, mit der Cutline läuft (wie im README)
pub const MIN_FFMPEG: u32 = 7;
static OLD_FFMPEG: Mutex<Option<String>> = Mutex::new(None);

/// Version aus der ersten Zeile von `ffmpeg -version`, wenn sie unter [`MIN_FFMPEG`] liegt.
/// Entwicklungsstände („N-12345-g…“, „git-…“) und Unlesbares gelten als neu genug.
pub fn ffmpeg_too_old(line: &str) -> Option<String> {
    let v = line.strip_prefix("ffmpeg version ")?.split_whitespace().next()?;
    let major: u32 = v.trim_start_matches('n').split(['.', '-']).next()?.parse().ok()?;
    (major < MIN_FFMPEG).then(|| v.to_string())
}

/// Einmal je Sitzung: zu altes FFmpeg (gefundene Version), sobald `system_info` gelaufen ist
pub fn take_old_ffmpeg() -> Option<String> {
    OLD_FFMPEG.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// System, Kerne, Speicher, FFmpeg (einmal ermittelt, im Hintergrund)
pub fn system_info() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| {
        let ff = cl_media::ffmpeg_command().ok();
        let path = ff.as_ref().map(|c| c.get_program().to_string_lossy().into_owned()).unwrap_or_else(|| "nicht gefunden".into());
        let version = run_out(cl_media::ffmpeg_command().ok().map(|mut c| {
            c.arg("-version");
            c
        }));
        let encoders = run_out(cl_media::ffmpeg_command().ok().map(|mut c| {
            c.args(["-hide_banner", "-encoders"]);
            c
        }));
        let hw = hw_encoders(&encoders);
        if let Some(v) = ffmpeg_too_old(version.lines().next().unwrap_or("")) {
            log::warn!(target: "system", "FFmpeg {v} ist zu alt: Cutline braucht {MIN_FFMPEG} oder neuer");
            *OLD_FFMPEG.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        }
        format!(
            "Cutline {} ({}, {} {})\nSystem: {}\nCPU: {} Kerne · Arbeitsspeicher: {}\nFFmpeg: {path} · {}\nHardware-Encoder: {}",
            env!("CARGO_PKG_VERSION"),
            build(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            os_version(),
            std::thread::available_parallelism().map_or(0, |n| n.get()),
            ram_gb().map_or("?".into(), |g| format!("{g:.1} GB")),
            version.lines().next().unwrap_or("?"),
            if hw.is_empty() { "keine".into() } else { hw.join(", ") }
        )
    })
}

/// Start: Grafik, Skalierung, Sprache, Design (vom UI-Thread), System im Hintergrund
pub fn startup(cc: &eframe::CreationContext<'_>, s: &crate::settings::AppSettings) {
    let gpu = cc.wgpu_render_state.as_ref().map(|r| {
        let i = r.adapter.get_info();
        format!("{} ({:?}, {:?}, Treiber {} {})", i.name, i.backend, i.device_type, i.driver, i.driver_info)
    });
    let info = format!(
        "Grafik: {} · Skalierung: {:.2} × {:.2} · Sprache: {} · Design: {}",
        gpu.as_deref().unwrap_or("?"),
        cc.egui_ctx.native_pixels_per_point().unwrap_or(1.0),
        s.ui_scale,
        s.lang,
        s.theme
    );
    log::info!(target: "system", "{info}");
    let _ = UI_INFO.set(info);
    let _ = std::thread::Builder::new().name("cl-sysinfo".into()).spawn(|| log::info!(target: "system", "{}", system_info()));
}

// ---------- Export ----------

/// Einträge der letzten 24 Stunden aus den Dateien (in Dateireihenfolge), zeitlich sortiert.
/// Zeilen ohne Zeitstempel gehören zum Eintrag davor; ohne Eintrag davor fallen sie weg.
pub fn last_24h(texts: &[String], now: i64) -> String {
    let mut entries: Vec<(i64, String)> = Vec::new();
    for t in texts {
        let mut cur = None;
        for line in t.lines() {
            match parse_stamp(line) {
                Some(ms) => {
                    entries.push((ms, line.to_string()));
                    cur = Some(entries.len() - 1);
                }
                None => {
                    if let Some(c) = cur {
                        let e: &mut (i64, String) = &mut entries[c];
                        e.1.push('\n');
                        e.1.push_str(line);
                    }
                }
            }
        }
    }
    entries.retain(|e| e.0 >= now - DAY_MS && e.0 <= now + 60_000);
    entries.sort_by_key(|e| e.0);
    let mut out = entries.into_iter().map(|e| e.1).collect::<Vec<_>>().join("\n");
    out.push('\n');
    out
}

/// Einstellungen ohne Geheimnisse und ohne persönliche Pfadlisten
pub fn scrub_settings(v: &mut serde_json::Value) {
    if let Some(m) = v.as_object_mut() {
        for k in ["recent", "projects_dir", "layout", "layouts"] {
            m.remove(k);
        }
    }
    fn walk(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.retain(|k, _| {
                    let k = k.to_ascii_lowercase();
                    !(["token", "secret", "password", "api_key", "apikey", "authorization"].iter().any(|s| k.contains(s)) || k == "key")
                });
                m.values_mut().for_each(walk);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(v);
}

/// Kopf des Exports
pub fn header(system: &str, ui: &str, mut settings: serde_json::Value, packages: &[(String, String, bool)], project: Option<String>, now: i64) -> String {
    scrub_settings(&mut settings);
    let packs: Vec<String> = packages.iter().map(|(id, v, on)| format!("{id} {v}{}", if *on { "" } else { " (aus)" })).collect();
    let text = format!(
        "Cutline-Protokoll · exportiert {}\n{system}\n{ui}\nPakete: {}\nProjekt: {}\nEinstellungen: {}\n\n---- Protokoll der letzten 24 Stunden ----\n",
        stamp(now, offset()),
        packs.join(", "),
        project.as_deref().unwrap_or("keins"),
        settings
    );
    redact(&text)
}

/// Ganze Exportdatei: Kopf + Zeilen der letzten 24 Stunden ohne Pfade und Dateinamen ([`anon`]),
/// Benutzerordner als `~`
pub fn export_text(dir: &Path, head: &str, now: i64, home: &str) -> String {
    let today = (now + offset() * 1000).div_euclid(DAY_MS);
    let texts: Vec<String> = log_files(dir)
        .into_iter()
        .filter(|(d, _)| *d >= today - 2)
        .filter_map(|(_, p)| std::fs::read(p).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .collect();
    let text = redact(&format!("{head}{}", last_24h(&texts, now)));
    let text: Vec<String> = text.split_inclusive('\n').map(anon).collect();
    tilde(&text.concat(), home)
}

/// Vorschlag für den Dateinamen: `cutline-protokoll-JJJJ-MM-TT-HHMM.txt`
pub fn export_name(now: i64) -> String {
    let s = stamp(now, offset());
    format!("cutline-protokoll-{}-{}{}.txt", &s[..10], &s[11..13], &s[14..16])
}

impl crate::app::App {
    /// Einstellungen → Über Cutline: „Protokoll exportieren“
    pub fn export_log(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        // Klicktest ohne Datei-Dialog: Ziel aus der Umgebung (nur mit CUTLINE_SHOTS)
        let target = match std::env::var_os("CUTLINE_LOG_EXPORT").filter(|_| std::env::var_os("CUTLINE_SHOTS").is_some()) {
            Some(p) => Some(PathBuf::from(p)),
            None => crate::dialog().add_filter(crate::tr("Textdatei"), &["txt"]).set_file_name(export_name(now)).save_file(),
        };
        let Some(out) = target else { return };
        let packages: Vec<(String, String, bool)> = self
            .plugins
            .plugins
            .iter()
            .map(|p| (p.manifest.plugin.id.clone(), p.manifest.plugin.version.clone(), p.enabled))
            .collect();
        let project = (self.screen == crate::app::Screen::Editor).then(|| counts(&self.project));
        let settings = serde_json::to_value(&self.settings).unwrap_or_default();
        let ui = UI_INFO.get().cloned().unwrap_or_default();
        flush();
        self.tasks.spawn(ctx, "log-export", crate::tr("Protokoll wird exportiert"), move |_, _| {
            let head = header(system_info(), &ui, settings, &packages, project, now);
            let text = export_text(&crate::paths::logs_dir(), &head, now, &home());
            match crate::paths::write_atomic(&out, text) {
                Ok(()) => crate::tasks::TaskDone::Run(Box::new(move |app: &mut crate::app::App| {
                    log::info!(target: "sitzung", "Protokoll exportiert: {}", out.display());
                    app.toast(format!("{}  Protokoll gespeichert", icon::CHECK));
                    *LAST_EXPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(out);
                })),
                Err(e) => crate::tasks::TaskDone::Failed(format!("{}: {e}", out.display())),
            }
        });
        CRASHED.store(false, Ordering::Relaxed);
    }
}

/// Knöpfe in Einstellungen → Über Cutline
pub fn about_buttons(app: &mut crate::app::App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let busy = app.tasks.running("log-export").is_some();
        if ui
            .add_enabled(!busy, theme::secondary(format!("{}  Protokoll exportieren", icon::FILE_TEXT)))
            .on_hover_text(crate::tr("Letzte 24 Stunden als Textdatei speichern"))
            .clicked()
        {
            app.export_log(ui.ctx());
        }
        if theme::icon_button(ui, icon::FOLDER_OPEN, "Protokollordner öffnen", true, false).clicked() {
            let d = crate::paths::logs_dir();
            let _ = std::fs::create_dir_all(&d);
            crate::home::reveal(&d);
        }
        let last = LAST_EXPORT.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(p) = last {
            if ui.add(theme::ghost(format!("{}  Im Explorer zeigen", icon::FOLDER_OPEN))).clicked() {
                crate::home::reveal(&p);
            }
        }
    });
}

/// Nach einem Absturz beim nächsten Start: dezente Meldung mit „Protokoll exportieren“ (einmal)
pub fn crash_notice(ctx: &egui::Context, app: &mut crate::app::App) {
    if !CRASHED.load(Ordering::Relaxed) {
        return;
    }
    let mut close = false;
    egui::Area::new(egui::Id::new("crash-notice"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            crate::dialog::frame(ctx).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::WARNING_CIRCLE).color(theme::TEXT_DIM()));
                    ui.label(crate::tr("Cutline wurde beim letzten Mal unerwartet beendet"));
                    if ui.add(theme::secondary("Protokoll exportieren")).clicked() {
                        app.export_log(ctx);
                    }
                    close |= theme::icon_button(ui, icon::X, "Schließen", true, false).clicked();
                });
            });
        });
    if close {
        CRASHED.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_version_check() {
        let old = |v: &str| ffmpeg_too_old(&format!("ffmpeg version {v} Copyright (c) 2000-2026 the FFmpeg developers"));
        assert_eq!(old("6.1.1-3ubuntu5"), Some("6.1.1-3ubuntu5".into()));
        assert_eq!(old("4.4.2-0ubuntu0.22.04.1"), Some("4.4.2-0ubuntu0.22.04.1".into()));
        assert_eq!(old("n6.0"), Some("n6.0".into()));
        for v in ["7.1.5-0+deb13u1", "n8.1", "n8.1.3-35-gf1acb0a266-20261116", "8.0-full_build-www.gyan.dev", "N-12345-g0123abcd", "git-2026-11-01-abc"] {
            assert_eq!(old(v), None, "{v}");
        }
        assert_eq!(ffmpeg_too_old(""), None);
        assert_eq!(ffmpeg_too_old("?"), None);
    }

    fn rec(ms: i64, level: Level, msg: &str) -> Rec {
        Rec { ms, level, area: "test".into(), msg: msg.into() }
    }

    #[test]
    fn names_paths_and_texts_stay_out_of_the_log() {
        assert_eq!(anon("„Bewerbung Firma X“ importiert: 3 Clips"), "„***“ importiert: 3 Clips");
        assert_eq!(anon(r"Archiv erstellt: D:\Kunden\Firma X\a.zip"), "Archiv erstellt: <pfad>.zip");
        assert_eq!(anon("Stand gesichert in /home/u/Firma X/p.cutline"), "Stand gesichert in <pfad>.cutline");
        assert_eq!(
            anon("Error opening input C:/Firma X/b.mp4: No such file\n~/x/y.wav: denied"),
            "Error opening input <pfad>.mp4: No such file\n<pfad>.wav: denied"
        );
        // keine Pfade: bleibt
        for s in ["10 MB/s", "-/filter_complex", "https://a.b/c", "und/oder", "1/2", "Paket entfernt"] {
            assert_eq!(anon(s), s);
        }
        let args: Vec<String> =
            ["-y", "-i", r"D:\Kunden\Firma X\clip.mov", "-/filter_complex", "graph.txt", "-c:v", "libx264", "/home/u/out.mp4"]
                .map(String::from)
                .into();
        assert_eq!(
            ffmpeg_cmd(&args),
            "ffmpeg -y -i <pfad>.mov -/filter_complex graph.txt -c:v libx264 <pfad>.mp4"
        );
    }

    /// Zeilen aus einem exportierten Protokoll (Klicktest): Pfade mit `~/` und gemischten Trennern,
    /// Dateinamen in Meldungen – im Export bleibt nur die Endung.
    #[test]
    fn exported_log_has_no_file_or_project_names() {
        let lines = [
            r"2026-11-16 19:09:17.136 +01:00 INFO  export: fertig nach 245.4 s: C:\Users\nutzer/AppData/Local/Temp/qa/K5_proj\entwurf.mp4 · Some((852, 480, Rational { num: 60, den: 1 }, Mp4, H264, Recommended, Nvenc, true))",
            r"2026-11-16 19:10:31.628 +01:00 INFO  export: Fehler nach 27.3 s: ~/AppData/Local/Temp/qa/K5_proj\K5_abbruch.mp4 · Some((852, 480))",
            r"2026-11-16 19:11:56.839 +01:00 INFO  export: fertig nach 12.0 s: ~/AppData/Local/Temp/qa/K5_proj\entwurf.gif · Some((1920, 1080))",
            "2026-11-16 18:44:12.469 +01:00 INFO  proxy: videoplayback.mp4 fertig in 129747 ms",
            "2026-11-16 18:51:54.712 +01:00 ERROR meldung: kaputt_text.mp4 konnte nicht gelesen werden: Datei kann nicht gelesen werden",
            "2026-11-16 18:52:57.789 +01:00 INFO  meldung: Vorlage „Mein Projekt“ gespeichert",
        ];
        let dir = tempfile::tempdir().unwrap();
        let now = lines.iter().filter_map(|l| parse_stamp(l)).max().unwrap() + 1000;
        std::fs::write(dir.path().join(file_name(now, offset())), lines.join("\n") + "\n").unwrap();
        let out = export_text(dir.path(), "", now, r"C:\Users\nutzer");
        for bad in ["entwurf", "K5_", "videoplayback", "kaputt_text", "AppData", "Mein Projekt", "nutzer"] {
            assert!(!out.contains(bad), "{bad} in\n{out}");
        }
        assert!(out.contains("fertig nach 245.4 s: <pfad>.mp4 · Some((852, 480, Rational { num: 60, den: 1 }"), "{out}");
        assert!(out.contains("<pfad>.gif · ") && out.contains("proxy: <datei>.mp4 fertig") && out.contains("meldung: <datei>.mp4 konnte"), "{out}");
        // Versionen, Zeiten und Zahlen bleiben
        for s in ["Cutline 0.11.1 (Release)", "18:44:12.469", "graph.txt", "10.0.26300.9974", "245.4 s"] {
            assert_eq!(anon(s), s);
        }
    }

    fn read_all(dir: &Path) -> String {
        log_files(dir).into_iter().map(|(_, p)| std::fs::read_to_string(p).unwrap()).collect()
    }

    #[test]
    fn stamps_round_trip_across_offsets_and_days() {
        for off in [0, 3600, 7200, -5 * 3600, 5 * 3600 + 1800] {
            for ms in [0, 1_763_200_000_123, 1_767_225_599_999, 951_782_400_000] {
                let s = stamp(ms, off);
                assert_eq!(parse_stamp(&format!("{s} INFO  x: y")), Some(ms), "{s}");
            }
        }
        assert_eq!(stamp(1_763_215_402_123, 3600), "2025-11-15 15:03:22.123 +01:00");
        assert_eq!(file_name(1_763_247_600_000, 3600), "cutline-2025-11-16.log", "Mitternacht Ortszeit");
        for bad in ["", "2026-11-15", "2026-13-15 10:00:00.000 +01:00 x", "2026-11-15 10:00:00.000 01:00 x", "xxxx-11-15 10:00:00.000 +01:00", "    at main.rs"] {
            assert_eq!(parse_stamp(bad), None, "{bad:?}");
        }
        assert_eq!(file_day("cutline-2026-11-15.log"), Some(days_from_civil(2026, 11, 15)));
        assert_eq!(file_day("cutline-protokoll.txt"), None);
        assert_eq!(civil(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn redacts_all_secret_patterns() {
        let cases = [
            ("Schlüssel epidemic_live_AbC123-xyz_9 benutzt", "Schlüssel epidemic_live_*** benutzt"),
            ("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc.def", "Authorization: Bearer ***"),
            ("Authorization: Basic dXNlcjpwYXNz", "Authorization: Basic ***"),
            ("Kopf bearer   abcdefgh12345678", "Kopf bearer   ***"),
            (r#"{"access_token":"AAA.bbb","refresh_token": "RRR","expires_in":3600}"#, r#"{"access_token":"***","refresh_token": "***","expires_in":3600}"#),
            (r#"{"token": "abc\"def", "read_only": true}"#, r#"{"token": "***", "read_only": true}"#),
            (r#""headers": { "Authorization": "Bearer t0k3n-s3cret" }"#, r#""headers": { "Authorization": "***" }"#),
            ("GET https://x.example/a?token=abc123&page=2 HTTP", "GET https://x.example/a?*** HTTP"),
            ("https://cdn.example/f.mp3?key=K1&signature=S%2F2#frag", "https://cdn.example/f.mp3?***"),
            ("https://login.example/cb?code=xyz&state=1", "https://login.example/cb?***"),
            // ureq-Fehler nach der GitHub-Umleitung: signierte Asset-Adresse mit `jwt=`
            ("https://objects.example/a.zip?sp=r&jwt=eyJ0eXAi.x.y: Connection Failed", "https://objects.example/a.zip?***: Connection Failed"),
            ("(\"http://127.0.0.1:9/x?a=1\")", "(\"http://127.0.0.1:9/x?***\")"),
            ("X-Amz-Signature=deadbeef", "X-Amz-Signature=***"),
        ];
        for (inp, want) in cases {
            assert_eq!(redact(inp), want, "{inp}");
        }
    }

    #[test]
    fn redaction_leaves_harmless_text() {
        for s in [
            "ffmpeg -i C:\\Users\\Ünïcödé\\Videos\\a b.mp4 -c:v libx264 -crf 23 -g 60 -keyint_min 30 -vf colorkey=0x00ff00:0.3 out.mp4",
            "chromakey=green:0.1 keyframes=3 monkey=1",
            "FFmpeg beendet: exit code=1 · Tokens: 5 · token count 3",
            "https://example.com/a/b.zip: Dns Failed",
            "Bearer of bad news",
            "\"key\": \"Strg+S\"",
            "Projekt „Urlaub 🎬“ · 3 Spuren",
            "",
        ] {
            assert_eq!(redact(s), s);
        }
    }

    #[test]
    fn home_becomes_tilde() {
        let h = if cfg!(windows) { "C:\\Users\\Ada" } else { "/home/ada" };
        assert_eq!(tilde(&format!("{h}\\Videos\\a.mp4 und {h}/x"), h), "~\\Videos\\a.mp4 und ~/x");
        assert_eq!(tilde("c:/users/ada/b.mp4 · Ü", "C:\\Users\\Ada\\"), "~/b.mp4 · Ü");
        assert_eq!(tilde("/a/b", ""), "/a/b");
        assert_eq!(tilde("ÄÖÜ ohne Pfad", h), "ÄÖÜ ohne Pfad");
    }

    #[test]
    fn last_24h_merges_files_in_time_order() {
        let off = 3600;
        let now = 1_763_251_200_000; // Sonntag 01:00 Ortszeit
        let line = |ms: i64, m: &str| format_line(&rec(ms, Level::Info, m), off);
        let yesterday = format!(
            "{}{}kaputt ohne Zeit\n{}{}",
            line(now - DAY_MS - 1000, "zu alt"),
            line(now - 3_600_000, "gestern spät"),
            line(now - 7_200_000, "noch früher"),
            &line(now - 1000, "halb")[..20]
        );
        let today = format!("Müll am Anfang\n{}{}", line(now - 1_800_000, "nach Mitternacht"), format_line(&rec(now - 60_000, Level::Error, "Panic\nZeile 2"), off));
        let out = last_24h(&[yesterday, today], now);
        let order: Vec<&str> = ["noch früher", "gestern spät", "nach Mitternacht", "Panic", "    Zeile 2"].to_vec();
        let pos: Vec<usize> = order.iter().map(|m| out.find(m).unwrap_or_else(|| panic!("{m} fehlt:\n{out}"))).collect();
        assert!(pos.windows(2).all(|w| w[0] < w[1]), "{out}");
        assert!(!out.contains("zu alt") && !out.contains("Müll"), "{out}");
        assert!(out.contains("kaputt ohne Zeit"), "gehört zum Eintrag davor");
    }

    #[test]
    fn limit_lets_only_warnings_through_and_notes_once() {
        assert_eq!(admit(10, 100, Level::Info, false), (false, true));
        assert_eq!(admit(100, 100, Level::Info, false), (true, false));
        assert_eq!(admit(150, 100, Level::Info, true), (false, false));
        assert_eq!(admit(150, 100, Level::Warn, true), (false, true));
        assert_eq!(admit(120, 100, Level::Error, false), (true, true));
        assert_eq!(admit(200, 100, Level::Error, true), (false, false));
        let d = tempfile::tempdir().unwrap();
        let tx = spawn(d.path().into(), 2000, false);
        let now = now_ms();
        for i in 0..200 {
            tx.send(Msg::Rec(rec(now, Level::Info, &format!("Zeile {i}")))).unwrap();
        }
        tx.send(Msg::Rec(rec(now, Level::Warn, "Warnung bleibt"))).unwrap();
        sync(&tx, true, Duration::from_secs(10));
        let t = read_all(d.path());
        assert_eq!(t.matches(LIMIT_NOTE).count(), 1, "{t}");
        assert!(t.contains("Warnung bleibt") && !t.contains("Zeile 199"));
        assert!(t.len() < 2600, "{}", t.len());
    }

    #[test]
    fn repeats_are_folded() {
        let d = tempfile::tempdir().unwrap();
        let tx = spawn(d.path().into(), FILE_LIMIT, false);
        let now = now_ms();
        for _ in 0..38 {
            tx.send(Msg::Rec(rec(now, Level::Warn, "Ton: Aussetzer"))).unwrap();
        }
        tx.send(Msg::Rec(rec(now, Level::Info, "anders"))).unwrap();
        tx.send(Msg::Rec(rec(now, Level::Info, "anders"))).unwrap();
        sync(&tx, true, Duration::from_secs(10));
        let t = read_all(d.path());
        assert_eq!(t.matches("Ton: Aussetzer").count(), 1, "{t}");
        assert!(t.contains("test: … 37× wiederholt") && t.contains("test: … 1× wiederholt"), "{t}");
        assert!(t.find("37×").unwrap() < t.find("anders").unwrap());
    }

    #[test]
    fn old_files_are_pruned() {
        let d = tempfile::tempdir().unwrap();
        let today = days_from_civil(2026, 11, 15);
        for n in ["cutline-2026-11-15.log", "cutline-2026-11-08.log", "cutline-2026-11-07.log", "cutline-2025-01-01.log", "notizen.txt", "cutline-kaputt.log"] {
            std::fs::write(d.path().join(n), "x").unwrap();
        }
        assert_eq!(prune(d.path(), today), 2);
        let mut left: Vec<String> = std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["cutline-2026-11-08.log", "cutline-2026-11-15.log", "cutline-kaputt.log", "notizen.txt"]);
    }

    #[test]
    fn header_has_no_secrets() {
        let mut s = serde_json::to_value(crate::settings::AppSettings::default()).unwrap();
        let m = s.as_object_mut().unwrap();
        m.insert("recent".into(), serde_json::json!(["/home/ada/geheim.cutline"]));
        m.insert("projects_dir".into(), serde_json::json!("/home/ada/Privat"));
        m.insert("mcp_token".into(), serde_json::json!("mcp-0123456789abcdef"));
        m.insert("epidemic".into(), serde_json::json!({"api_key": "epidemic_live_Q1W2E3", "tokens": {"access_token": "AT-555", "refresh_token": "RT-777"}, "user_id": "cl-1"}));
        m.insert("notiz".into(), serde_json::json!("Authorization: Bearer zzzzzzzzzzzzzzzz"));
        let h = header("System: Test", "Grafik: Test", s, &[("standard".into(), "1.2.0".into(), true)], Some("2 Spuren".into()), 1_763_251_200_000);
        for secret in ["geheim.cutline", "Privat", "mcp-0123456789abcdef", "Q1W2E3", "AT-555", "RT-777", "zzzzzzzz"] {
            assert!(!h.contains(secret), "{secret} in\n{h}");
        }
        assert!(h.contains("standard 1.2.0") && h.contains("2 Spuren") && h.contains("\"theme\"") && h.contains("user_id"), "{h}");
    }

    #[test]
    fn detects_unclean_exit() {
        let l = |m: &str| format!("2026-11-15 10:00:00.000 +01:00 INFO  {m}\n");
        let start = |p: u32| l(&format!("sitzung: {START} pid={p} · Cutline 0.12.0 (Release)"));
        let end = |p: u32| l(&format!("sitzung: {END} pid={p}"));
        let panic = |p: u32| l(&format!("absturz: {PANIC} pid={p} im Thread „main“: panicked at x.rs:1:1:"));
        let dead = |_| false;
        assert!(!unclean_exit("", dead), "kein Protokoll");
        assert!(!unclean_exit(&format!("{}{}", start(1), end(1)), dead));
        assert!(unclean_exit(&start(1), dead), "kein Ende");
        assert!(unclean_exit(&format!("{}{}{}", start(1), panic(1), end(1)), dead), "Panic");
        assert!(!unclean_exit(&start(1), |p| p == 1), "läuft noch (zweites Fenster)");
        assert!(!unclean_exit(&format!("{}{}{}{}", start(1), start(2), end(2), end(1)), dead), "zählt der letzte Start");
        assert!(unclean_exit(&format!("{}{}{}", start(1), end(1), start(2)), dead));
        assert!(!unclean_exit(&format!("{}{}{}", start(2), end(9), end(2)), dead));
    }

    #[test]
    fn many_threads_arrive_complete_and_unbroken() {
        let d = tempfile::tempdir().unwrap();
        let tx = spawn(d.path().into(), FILE_LIMIT, false);
        let hs: Vec<_> = (0..8)
            .map(|t| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for i in 0..1250 {
                        let msg = format!("t{t} n{i} {}", "x".repeat(40 + i % 50));
                        tx.send(Msg::Rec(Rec { ms: now_ms(), level: Level::Info, area: "test".into(), msg })).unwrap();
                    }
                })
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        sync(&tx, true, Duration::from_secs(30));
        let t = read_all(d.path());
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines.len(), 10_000);
        let mut seen = std::collections::HashSet::new();
        for l in &lines {
            assert!(parse_stamp(l).is_some(), "{l}");
            let msg = l.split_once(" test: ").unwrap().1;
            let (id, xs) = msg.rsplit_once(' ').unwrap();
            let i: usize = id.split_once(" n").unwrap().1.parse().unwrap();
            assert_eq!(xs, "x".repeat(40 + i % 50), "{l}");
            assert!(seen.insert(id.to_string()), "{l}");
        }
    }

    #[test]
    fn error_is_on_disk_at_once() {
        let d = tempfile::tempdir().unwrap();
        let tx = spawn(d.path().into(), FILE_LIMIT, false);
        tx.send(Msg::Rec(rec(now_ms(), Level::Info, "davor"))).unwrap();
        tx.send(Msg::Rec(rec(now_ms(), Level::Error, "Fehler XYZ"))).unwrap();
        // nur warten, bis verarbeitet – ohne Flush
        sync(&tx, false, Duration::from_secs(10));
        let t = read_all(d.path());
        assert!(t.contains("davor") && t.contains("ERROR test: Fehler XYZ"), "{t:?}");
    }

    #[test]
    fn write_errors_do_not_hurt() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("datei");
        std::fs::write(&file, "x").unwrap();
        let tx = spawn(file.join("logs"), FILE_LIMIT, true);
        for i in 0..100 {
            tx.send(Msg::Rec(rec(now_ms(), Level::Error, &format!("{i}")))).unwrap();
        }
        let (a, b) = channel();
        tx.send(Msg::Sync(a, true)).unwrap();
        b.recv_timeout(Duration::from_secs(10)).expect("Schreib-Thread lebt");
    }

    #[test]
    fn finds_hardware_encoders() {
        let list = " V....D libx264              libx264 H.264\n V....D h264_nvenc           NVIDIA NVENC\n V....D hevc_qsv  QSV\n A....D aac   AAC\n V..... h264_mf  MediaFoundation\n";
        assert_eq!(hw_encoders(list), ["h264_nvenc", "hevc_qsv", "h264_mf"]);
    }

    /// Kindprozess für [`panic_hook_logs_message_and_place`]
    #[test]
    #[ignore]
    fn panic_child() {
        let Some(dir) = std::env::var_os("CUTLINE_LOG_CHILD") else { return };
        init_at(PathBuf::from(dir), false);
        log::info!(target: "projekt", "über die Fassade");
        log::info!(target: "wgpu_core::device", "fremde Info");
        log::warn!(target: "wgpu_core::device", "fremde Warnung");
        panic!("Testabsturz 4711");
    }

    #[test]
    fn panic_hook_logs_message_and_place() {
        let d = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["applog::tests::panic_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env("CUTLINE_LOG_CHILD", d.path())
            .env("RUST_BACKTRACE", "0")
            .output()
            .unwrap();
        assert!(!out.status.success(), "Kind soll abstürzen");
        let t = read_all(d.path());
        assert!(t.contains("ERROR absturz: Absturz pid=") && t.contains("Testabsturz 4711") && t.contains("applog.rs"), "{t}\n{}", String::from_utf8_lossy(&out.stderr));
        assert!(t.contains("INFO  projekt: über die Fassade") && t.contains("WARN  wgpu_core::device: fremde Warnung") && !t.contains("fremde Info"), "{t}");
        assert!(unclean_exit(&format!("2026-11-15 10:00:00.000 +01:00 INFO  sitzung: {START} pid={}\n{t}", out_pid(&t)), |_| false));
        // bisheriges Verhalten bleibt: die Standardmeldung kommt danach trotzdem
        assert!(String::from_utf8_lossy(&out.stderr).contains("Testabsturz 4711"));
    }

    fn out_pid(t: &str) -> u32 {
        t.lines().find_map(|l| marker_pid(l, "absturz", PANIC)).unwrap()
    }
}
