//! Epidemic-Sound-Integration (Partner API v0).
//!
//! Zwei Anmeldewege, beide mit Zugangsdaten des Nutzers:
//! - **Connect** (OAuth 2.0 mit PKCE im Systembrowser, Rückruf auf 127.0.0.1): eigenes
//!   Epidemic-Konto, volle Bibliothek je Abo, „Gefällt mir“-Liste.
//! - **API-Schlüssel** aus dem Developer Portal (`epidemic_live_…`), z. B. für Partner.
//!
//! Gemeldet wird nur, was die Nutzungsbedingungen verlangen: ein Export mit Epidemic-Musik
//! (`POST /v0/usage`). Vorhören und Laden zählt die API selbst.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_BASE: &str = "https://api.epidemicsound.com";
pub const AUTH_URL: &str =
    "https://login.epidemicsound.com/auth/realms/accounts/protocol/openid-connect/auth";
pub const TOKEN_URL: &str =
    "https://login.epidemicsound.com/auth/realms/accounts/protocol/openid-connect/token";
pub const LOGOUT_URL: &str =
    "https://login.epidemicsound.com/auth/realms/accounts/protocol/openid-connect/logout";
/// Fester Rückruf-Port, damit die Adresse im Developer Portal eingetragen werden kann.
pub const REDIRECT_PORT: u16 = 53682;

pub fn redirect_uri() -> String {
    format!("http://127.0.0.1:{REDIRECT_PORT}/callback")
}

pub type Result<T> = std::result::Result<T, Error>;

/// Fehler der Schnittstelle. Anmeldung abgelaufen, Limit und „zu schnell“ sind eigene Fälle,
/// damit die App sie behandelt, statt Meldungstexte zu vergleichen.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Refresh-Token abgelaufen bzw. 401 im Modus Connect: neu verbinden
    LoginExpired,
    /// 429; Wartezeit in Sekunden aus `Retry-After`, falls genannt
    RateLimited(Option<u64>),
    /// 406: zu schnell hintereinander geladen
    TooFast,
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::LoginExpired => f.write_str("Anmeldung abgelaufen – bitte neu verbinden"),
            Error::RateLimited(Some(n)) => write!(f, "Limit erreicht, in {n} s wieder möglich"),
            Error::RateLimited(None) => f.write_str("Limit erreicht, bitte später erneut versuchen"),
            Error::TooFast => f.write_str("Zu schnell hintereinander geladen, bitte kurz warten"),
            Error::Other(s) => f.write_str(s),
        }
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Other(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Other(s.into())
    }
}

/// Wartezeit aus dem `Retry-After`-Kopf (nur Sekunden; Datumsform oder Unsinn = unbekannt).
pub fn retry_after(header: Option<&str>) -> Option<u64> {
    header?.trim().parse::<u64>().ok()
}

/// So lange keine neuen Anfragen derselben Art: genannte Wartezeit (1 s … 1 h), sonst 30 s.
pub fn limit_wait(secs: Option<u64>) -> Duration {
    Duration::from_secs(secs.map_or(30, |s| s.clamp(1, 3600)))
}

/// Eine Wiederholung nach „zu schnell“ (406): `wait` (≥ 2 s) und noch ein Versuch, erst dann Fehler.
pub fn retry_once_if_too_fast<T>(mut f: impl FnMut() -> Result<T>, wait: impl FnOnce()) -> Result<T> {
    match f() {
        Err(Error::TooFast) => {
            wait();
            f()
        }
        r => r,
    }
}

// ------------------------------------------------------------------ Einstellungen

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AuthMode {
    #[default]
    Connect,
    ApiKey,
}

/// Gespeicherte Konfiguration (liegt lokal im Datenordner der App).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: AuthMode,
    /// Client-ID der eigenen App im Epidemic Developer Portal (für Connect)
    pub client_id: String,
    /// API-Schlüssel (`epidemic_live_…`)
    pub api_key: String,
    /// Connect-Tokens nach der Anmeldung
    pub tokens: Option<Tokens>,
    /// Lokale Favoriten (Titel bleiben auch ohne Netz sichtbar)
    pub favorites: Vec<Track>,
    /// Anonyme, stabile Nutzerkennung für `x-partner-user-id`
    pub user_id: String,
    /// Abweichende API-Adresse (Tests)
    pub base_url: Option<String>,
}

impl Config {
    pub fn base(&self) -> &str {
        self.base_url.as_deref().unwrap_or(DEFAULT_BASE)
    }
    /// Sind Zugangsdaten vorhanden?
    pub fn ready(&self) -> bool {
        match self.mode {
            AuthMode::ApiKey => !self.api_key.trim().is_empty(),
            AuthMode::Connect => self.tokens.is_some(),
        }
    }
    pub fn is_favorite(&self, id: &str) -> bool {
        self.favorites.iter().any(|t| t.id == id)
    }
    pub fn toggle_favorite(&mut self, t: &Track) {
        if let Some(i) = self.favorites.iter().position(|x| x.id == t.id) {
            self.favorites.remove(i);
        } else {
            self.favorites.insert(0, t.clone());
        }
    }
    pub fn ensure_user_id(&mut self) {
        if self.user_id.is_empty() {
            self.user_id = format!("cl-{}", random_token(12));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix-Sekunden
    pub expires_at: u64,
}

// ------------------------------------------------------------------ Datentypen

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Named {
    pub id: String,
    pub name: String,
}

/// Die API liefert Namen mal als Objekt `{id, name}`, mal als bloßen Text (z. B. `mainArtists`).
impl<'de> Deserialize<'de> for Named {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Obj {
                #[serde(default)]
                id: String,
                #[serde(default)]
                name: String,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Text(name) => Named { id: name.clone(), name },
            Raw::Obj { id, name } => Named { id, name },
        })
    }
}

/// Liste lesen, in der einzelne Einträge kaputt sein dürfen (die übrigen bleiben).
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D) -> std::result::Result<Vec<T>, D::Error> {
    let raw: Vec<serde_json::Value> = Vec::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Images {
    pub default: Option<String>,
    #[serde(rename = "S")]
    pub s: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub title: String,
    pub main_artists: Vec<Named>,
    pub bpm: Option<f32>,
    /// Länge in Sekunden (die API liefert Sekunden oder Millisekunden, siehe `seconds()`)
    pub length: Option<f64>,
    pub moods: Vec<Named>,
    pub genres: Vec<Named>,
    pub images: Images,
    pub waveform_url: Option<String>,
    pub has_vocals: bool,
    pub is_preview_only: bool,
}

impl Track {
    pub fn artists(&self) -> String {
        self.main_artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
    /// Länge in Sekunden; Werte über 3600 werden als Millisekunden gedeutet.
    pub fn seconds(&self) -> Option<f64> {
        self.length.map(|l| if l > 3600.0 { l / 1000.0 } else { l })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Page {
    #[serde(deserialize_with = "lenient")]
    pub tracks: Vec<Track>,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pagination {
    pub limit: u32,
    pub offset: u32,
}

/// Suchanfrage. Leerer Begriff = Stöbern nach Filtern.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub term: String,
    pub moods: Vec<String>,
    pub genres: Vec<String>,
    pub bpm: Option<(u32, u32)>,
    /// Nur Instrumentals
    pub no_vocals: bool,
    pub offset: u32,
}

// ------------------------------------------------------------------ Client

pub struct Client {
    mode: AuthMode,
    base: String,
    bearer: String,
    user_id: String,
    agent: ureq::Agent,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(30))
        .build()
}

fn err(e: ureq::Error, mode: AuthMode) -> Error {
    match e {
        ureq::Error::Status(401, _) if mode == AuthMode::Connect => Error::LoginExpired,
        ureq::Error::Status(401, _) => "Schlüssel ungültig".into(),
        ureq::Error::Status(403, _) => "Kein Zugriff: Der Titel ist in deinem Plan oder Abo nicht enthalten".into(),
        ureq::Error::Status(406, _) => Error::TooFast,
        ureq::Error::Status(429, r) => Error::RateLimited(retry_after(r.header("Retry-After"))),
        ureq::Error::Status(c, r) => format!(
            "Epidemic Sound antwortet mit {c}: {}",
            r.into_string()
                .unwrap_or_default()
                .chars()
                .take(160)
                .collect::<String>()
        )
        .into(),
        ureq::Error::Transport(t) => format!("Keine Verbindung zu Epidemic Sound ({t})").into(),
    }
}

impl Client {
    /// Client aus der Konfiguration; Connect-Tokens werden bei Bedarf erneuert (`cfg` wird dann aktualisiert).
    pub fn from_config(cfg: &mut Config) -> Result<Self> {
        cfg.ensure_user_id();
        let bearer = match cfg.mode {
            AuthMode::ApiKey => {
                if cfg.api_key.trim().is_empty() {
                    return Err("Kein API-Schlüssel eingetragen".into());
                }
                cfg.api_key.trim().to_string()
            }
            AuthMode::Connect => {
                let Some(t) = cfg.tokens.clone() else {
                    return Err("Nicht mit Epidemic Sound verbunden".into());
                };
                if t.expires_at <= now() + 20 {
                    let fresh = refresh(&cfg.client_id, &t.refresh_token)?;
                    cfg.tokens = Some(fresh.clone());
                    fresh.access_token
                } else {
                    t.access_token
                }
            }
        };
        Ok(Self {
            mode: cfg.mode,
            base: cfg.base().trim_end_matches('/').to_string(),
            bearer,
            user_id: cfg.user_id.clone(),
            agent: agent(),
        })
    }

    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<serde_json::Value> {
        let mut r = self
            .agent
            .get(&format!("{}{path}", self.base))
            .set("Authorization", &format!("Bearer {}", self.bearer))
            .set("x-partner-user-id", &self.user_id);
        for (k, v) in query {
            r = r.query(k, v);
        }
        r.call()
            .map_err(|e| err(e, self.mode))?
            .into_json::<serde_json::Value>()
            .map_err(|e| e.to_string().into())
    }

    /// Suche (`/v0/tracks/search`) bzw. Stöbern nach Filtern (`/v0/tracks`).
    pub fn search(&self, q: &Query) -> Result<Page> {
        let mut params: Vec<(&str, String)> =
            vec![("limit", "40".into()), ("offset", q.offset.to_string())];
        for m in &q.moods {
            params.push(("mood", m.clone()));
        }
        for g in &q.genres {
            params.push(("genre", g.clone()));
        }
        if let Some((lo, hi)) = q.bpm {
            params.push(("bpmMin", lo.to_string()));
            params.push(("bpmMax", hi.to_string()));
        }
        if q.no_vocals {
            params.push(("vocalType", "NONE".into()));
        }
        let path = if q.term.trim().is_empty() {
            "/v0/tracks"
        } else {
            params.push(("term", q.term.trim().chars().take(500).collect()));
            params.push(("sort", "Relevance".into()));
            params.push(("order", "desc".into()));
            "/v0/tracks/search"
        };
        serde_json::from_value(self.get(path, &params)?).map_err(|e| e.to_string().into())
    }

    fn list(&self, path: &str, key: &str) -> Result<Vec<Named>> {
        let v = self.get(path, &[("limit", "20".into()), ("type", "featured".into())])?;
        serde_json::from_value(v.get(key).cloned().unwrap_or_default()).map_err(|e| e.to_string().into())
    }

    /// Hervorgehobene Stimmungen
    pub fn moods(&self) -> Result<Vec<Named>> {
        self.list("/v0/moods", "moods")
    }

    /// Hervorgehobene Genres
    pub fn genres(&self) -> Result<Vec<Named>> {
        self.list("/v0/genres", "genres")
    }

    /// HLS-Adresse zum Vorhören (FFmpeg kann sie direkt abspielen).
    pub fn stream_url(&self, id: &str) -> Result<String> {
        let v = self.get(&format!("/v0/tracks/{id}/stream"), &[])?;
        v.get("url")
            .and_then(|u| u.as_str())
            .map(String::from)
            .ok_or_else(|| "Keine Vorhör-Adresse".into())
    }

    /// Signierte MP3-Adresse (320 kbit/s bei `high`).
    pub fn download_url(&self, id: &str, high: bool) -> Result<String> {
        let v = self.get(
            &format!("/v0/tracks/{id}/download"),
            &[
                ("format", "mp3".into()),
                ("quality", if high { "high" } else { "normal" }.into()),
            ],
        )?;
        v.get("url")
            .and_then(|u| u.as_str())
            .map(String::from)
            .ok_or_else(|| "Keine Download-Adresse".into())
    }

    /// Lädt den Titel als MP3 nach `dest`.
    pub fn download(&self, id: &str, dest: &std::path::Path) -> Result<()> {
        // 406: zu schnell hintereinander geladen – einmal nach 2 s noch einmal
        let resp = retry_once_if_too_fast(
            || {
                let url = self.download_url(id, true)?;
                self.agent.get(&url).call().map_err(|e| err(e, self.mode))
            },
            || std::thread::sleep(Duration::from_secs(2)),
        )?;
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = dest.with_extension("part");
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut resp.into_reader(), &mut f).map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&tmp, dest).map_err(|e| e.to_string().into())
    }

    /// Schläge eines Titels in Sekunden (Beta-Endpunkt; leer, wenn nicht verfügbar).
    pub fn beats(&self, id: &str) -> Vec<f64> {
        let Ok(v) = self.get(&format!("/v0/tracks/{id}/beats"), &[]) else {
            return Vec::new();
        };
        v.get("beats")
            .and_then(|b| b.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|b| b.get("time").and_then(|t| t.as_f64()))
                    .map(|t| if t > 10_000.0 { t / 1000.0 } else { t })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// „Gefällt mir“-Titel des verbundenen Kontos (nur Connect).
    pub fn liked(&self, offset: u32) -> Result<Page> {
        serde_json::from_value(self.get(
            "/v0/users/me/liked/tracks",
            &[("offset", offset.to_string())],
        )?)
        .map_err(|e| e.to_string().into())
    }

    /// Angemeldeter Nutzer (Name für die Anzeige).
    pub fn me(&self) -> Result<String> {
        let v = self.get("/v0/users/me", &[])?;
        Ok(["firstName", "name", "email"]
            .iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(String::from))
            .unwrap_or_else(|| "Epidemic Sound".into()))
    }

    /// Meldet einen Export mit diesen Titeln (Pflicht laut Nutzungsbedingungen).
    pub fn report_export(&self, track_ids: &[String], platform: &str) -> Result<()> {
        if track_ids.is_empty() {
            return Ok(());
        }
        let body = serde_json::json!({ "eventType": "EXPORTED", "platform": platform, "trackIds": track_ids });
        self.agent
            .post(&format!("{}/v0/usage", self.base))
            .set("Authorization", &format!("Bearer {}", self.bearer))
            .set("x-partner-user-id", &self.user_id)
            .send_json(body)
            .map_err(|e| err(e, self.mode))?;
        Ok(())
    }
}

// ------------------------------------------------------------------ Connect (OAuth mit PKCE)

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_token(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::getrandom(&mut b).expect("Zufallsquelle");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// PKCE: (verifier, challenge)
pub fn pkce() -> (String, String) {
    let verifier = random_token(48);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Laufende Anmeldung: Browser öffnen mit `url`, dann `finish` (blockiert bis zum Rückruf).
pub struct Login {
    pub url: String,
    verifier: String,
    state: String,
    client_id: String,
    listener: TcpListener,
}

pub fn start_login(client_id: &str) -> Result<Login> {
    if client_id.trim().is_empty() {
        return Err("Client-ID fehlt".into());
    }
    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
        .map_err(|e| format!("Port {REDIRECT_PORT} ist belegt ({e})"))?;
    let (verifier, challenge) = pkce();
    let state = random_token(12);
    let enc = |s: &str| {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>()
    };
    let url = format!(
        "{AUTH_URL}?response_type=code&client_id={}&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256&state={state}",
        enc(client_id.trim()),
        enc(&redirect_uri())
    );
    Ok(Login {
        url,
        verifier,
        state,
        client_id: client_id.trim().to_string(),
        listener,
    })
}

impl Login {
    /// Wartet bis zu `timeout` auf den Rückruf des Browsers und tauscht den Code gegen Tokens.
    pub fn finish(self, timeout: Duration) -> Result<Tokens> {
        let until = std::time::Instant::now() + timeout;
        let code = wait_for_callback(&self.listener, &self.state, until)?;
        exchange(&self.client_id, &code, &self.verifier)
    }
}

/// Nimmt Verbindungen an, bis der echte Rückruf (`/callback` mit passendem `state`) kommt, und liefert den
/// Code. Alles andere (vorab geöffnete Browser-Sockets, `/favicon.ico`, Scanner) bekommt 404 und wird
/// übergangen; eine stumme Verbindung blockiert höchstens 5 s.
fn wait_for_callback(listener: &TcpListener, state: &str, until: std::time::Instant) -> Result<String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    loop {
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() > until {
                    return Err("Anmeldung abgebrochen (Zeit abgelaufen)".into());
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e.to_string().into()),
        };
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut line = String::new();
        let _ = BufReader::new(&stream).read_line(&mut line);
        let target = line.split_whitespace().nth(1).unwrap_or("");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let params = parse_query(query);
        let get = |k: &str| params.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone());
        if path != "/callback" || get("state").as_deref() != Some(state) {
            let _ = write!(&stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }
        let code = get("code");
        let html = if code.is_some() {
            "Cutline ist mit Epidemic Sound verbunden. Du kannst dieses Fenster schließen."
        } else {
            "Anmeldung fehlgeschlagen. Bitte in Cutline erneut versuchen."
        };
        let body = format!("<!doctype html><meta charset=utf-8><title>Cutline</title><body style=\"font-family:sans-serif;background:#111;color:#eee;display:grid;place-items:center;height:90vh\"><p>{html}</p>");
        let _ = write!(&stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        return code.ok_or_else(|| {
            get("error_description")
                .or_else(|| get("error"))
                .unwrap_or_else(|| "Ungültiger Rückruf".into())
                .into()
        });
    }
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    let dec = |s: &str| {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < b.len() => {
                    // auf Bytes dekodieren: ein &str-Schnitt mitten in „€“ würde abstürzen
                    let h = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                    out.push(h.and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(b'?'));
                    i += 2;
                }
                c => out.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (dec(k), dec(v)))
        .collect()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

/// Fehler der Token-Anfrage; beim Refresh heißt 400/401 (`invalid_grant`): Refresh-Token abgelaufen.
fn token_err(e: ureq::Error, refreshing: bool) -> Error {
    match e {
        ureq::Error::Status(400 | 401, _) if refreshing => Error::LoginExpired,
        e => err(e, AuthMode::Connect),
    }
}

fn token_request(form: &[(&str, &str)], refreshing: bool) -> Result<Tokens> {
    let r: TokenResponse = agent()
        .post(TOKEN_URL)
        .send_form(form)
        .map_err(|e| token_err(e, refreshing))?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(Tokens {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_at: now() + r.expires_in,
    })
}

fn exchange(client_id: &str, code: &str, verifier: &str) -> Result<Tokens> {
    token_request(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &redirect_uri()),
        ("code_verifier", verifier),
        ("client_id", client_id),
    ], false)
}

pub fn refresh(client_id: &str, refresh_token: &str) -> Result<Tokens> {
    token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ], true)
}

/// Abmelden (Refresh-Token beim Anbieter ungültig machen).
pub fn logout(client_id: &str, refresh_token: &str) {
    let _ = agent()
        .post(LOGOUT_URL)
        .send_form(&[("client_id", client_id), ("refresh_token", refresh_token)]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Fremde Verbindungen vor dem Rückruf (leerer Socket, `/favicon.ico`, falscher `state`) werden
    /// übergangen; der echte Rückruf danach liefert den Code.
    #[test]
    fn the_callback_waits_past_foreign_connections() {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = l.local_addr().unwrap();
        let send = |req: &str| {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            s
        };
        let _empty = std::net::TcpStream::connect(addr).map(drop);
        let mut fav = send("GET /favicon.ico HTTP/1.1\r\n\r\n");
        let _wrong = send("GET /callback?state=falsch&code=x HTTP/1.1\r\n\r\n");
        let mut ok = send("GET /callback?state=s1&code=abc HTTP/1.1\r\n\r\n");
        let until = std::time::Instant::now() + Duration::from_secs(30);
        assert_eq!(wait_for_callback(&l, "s1", until).unwrap(), "abc");
        let mut r = String::new();
        fav.read_to_string(&mut r).unwrap();
        assert!(r.starts_with("HTTP/1.1 404"), "{r}");
        r.clear();
        ok.read_to_string(&mut r).unwrap();
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    }

    fn status(c: u16) -> ureq::Error {
        ureq::Error::Status(c, ureq::Response::new(c, "x", "").unwrap())
    }

    #[test]
    fn statuses_become_typed_errors() {
        // Connect: 401 heißt neu anmelden, mit API-Schlüssel „ungültig“
        assert_eq!(err(status(401), AuthMode::Connect), Error::LoginExpired);
        assert_eq!(err(status(401), AuthMode::ApiKey), Error::Other("Schlüssel ungültig".into()));
        assert_eq!(err(status(406), AuthMode::Connect), Error::TooFast);
        assert_eq!(err(status(429), AuthMode::Connect), Error::RateLimited(None));
        let r: ureq::Response = "HTTP/1.1 429 Too Many Requests
Retry-After: 17

".parse().unwrap();
        assert_eq!(err(ureq::Error::Status(429, r), AuthMode::Connect), Error::RateLimited(Some(17)));
    }

    #[test]
    fn an_expired_refresh_token_is_its_own_error() {
        assert_eq!(token_err(status(400), true), Error::LoginExpired);
        assert_eq!(token_err(status(401), true), Error::LoginExpired);
        // beim ersten Anmelden (Code-Tausch) ist 400 ein gewöhnlicher Fehler
        assert!(matches!(token_err(status(400), false), Error::Other(_)));
    }

    #[test]
    fn retry_after_and_wait_are_read_safely() {
        assert_eq!(retry_after(None), None);
        assert_eq!(retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")), None);
        assert_eq!(retry_after(Some("-5")), None);
        assert_eq!(limit_wait(None), Duration::from_secs(30));
        assert_eq!(limit_wait(Some(0)), Duration::from_secs(1));
        assert_eq!(limit_wait(Some(12)), Duration::from_secs(12));
        assert_eq!(limit_wait(Some(999_999)), Duration::from_secs(3600));
        assert_eq!(Error::RateLimited(Some(12)).to_string(), "Limit erreicht, in 12 s wieder möglich");
        assert_eq!(Error::RateLimited(None).to_string(), "Limit erreicht, bitte später erneut versuchen");
    }

    #[test]
    fn too_fast_is_retried_exactly_once() {
        use std::cell::Cell;
        let (calls, waits) = (Cell::new(0), Cell::new(0));
        let run = |fail_times: u32| {
            calls.set(0);
            waits.set(0);
            retry_once_if_too_fast(
                || {
                    calls.set(calls.get() + 1);
                    if calls.get() <= fail_times { Err(Error::TooFast) } else { Ok(7) }
                },
                || waits.set(waits.get() + 1),
            )
        };
        assert_eq!((run(0), calls.get(), waits.get()), (Ok(7), 1, 0));
        assert_eq!((run(1), calls.get(), waits.get()), (Ok(7), 2, 1));
        assert_eq!((run(2), calls.get(), waits.get()), (Err(Error::TooFast), 2, 1));
    }

    /// Mini-HTTP-Server, der auf jede Anfrage mit `body` antwortet und die Anfragezeile zurückgibt.
    fn serve(body: &'static str, n: usize) -> (String, std::sync::mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for s in l.incoming().take(n).flatten() {
                let mut r = BufReader::new(&s);
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut head = String::new();
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                    head.push_str(&h);
                }
                let len = head
                    .lines()
                    .find_map(|h| {
                        h.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                let mut buf = vec![0; len];
                r.read_exact(&mut buf).ok();
                tx.send(format!(
                    "{}|{}|{}",
                    first.trim(),
                    head.to_ascii_lowercase(),
                    String::from_utf8_lossy(&buf)
                ))
                .unwrap();
                let _ = write!(&s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (addr, rx)
    }

    fn cfg(base: &str) -> Config {
        Config {
            mode: AuthMode::ApiKey,
            api_key: "epidemic_live_test".into(),
            base_url: Some(base.into()),
            ..Default::default()
        }
    }

    #[test]
    fn search_builds_query_and_parses_tracks() {
        let (base, rx) = serve(
            r#"{"tracks":[{"id":"t1","title":"Neon Run","mainArtists":[{"name":"Ava"}],"bpm":128,"length":154,"moods":[{"id":"m","name":"Epic"}],"images":{"S":"https://img"},"hasVocals":false}],"pagination":{"limit":40,"offset":0}}"#,
            1,
        );
        let mut c = cfg(&base);
        let client = Client::from_config(&mut c).unwrap();
        let p = client
            .search(&Query {
                term: "gaming beat".into(),
                moods: vec!["epic".into(), "hopeful".into()],
                no_vocals: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.tracks[0].title, "Neon Run");
        assert_eq!(p.tracks[0].artists(), "Ava");
        assert_eq!(p.tracks[0].seconds(), Some(154.0));
        let req = rx.recv().unwrap();
        assert!(req.starts_with("GET /v0/tracks/search?"), "{req}");
        assert!(
            req.contains("mood=epic")
                && req.contains("mood=hopeful")
                && req.contains("vocalType=NONE")
                && req.contains("term=gaming"),
            "{req}"
        );
        assert!(
            req.contains("authorization: bearer epidemic_live_test"),
            "{req}"
        );
        assert!(req.contains("x-partner-user-id: cl-"), "{req}");
    }

    #[test]
    fn browse_without_term_and_usage_report() {
        let (base, rx) = serve(r#"{"tracks":[],"url":"https://cdn/x.mp3"}"#, 3);
        let mut c = cfg(&base);
        let client = Client::from_config(&mut c).unwrap();
        client
            .search(&Query {
                genres: vec!["hip-hop".into()],
                ..Default::default()
            })
            .unwrap();
        assert!(rx.recv().unwrap().starts_with("GET /v0/tracks?"));
        assert_eq!(
            client.download_url("t1", true).unwrap(),
            "https://cdn/x.mp3"
        );
        assert!(rx.recv().unwrap().contains("quality=high"));
        client.report_export(&["t1".into()], "LOCAL").unwrap();
        let r = rx.recv().unwrap();
        assert!(r.starts_with("POST /v0/usage"), "{r}");
        assert!(
            r.contains("\"eventType\":\"EXPORTED\"")
                && r.contains("\"trackIds\":[\"t1\"]")
                && r.contains("\"platform\":\"LOCAL\""),
            "{r}"
        );
    }

    #[test]
    fn pkce_and_favorites() {
        let (v, c) = pkce();
        assert!(v.len() >= 43 && c.len() == 43);
        assert_eq!(
            c,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(v.as_bytes()))
        );
        let mut cfg = Config::default();
        let t = Track {
            id: "a".into(),
            ..Default::default()
        };
        cfg.toggle_favorite(&t);
        assert!(cfg.is_favorite("a"));
        cfg.toggle_favorite(&t);
        assert!(!cfg.is_favorite("a"));
        assert!(!cfg.ready());
        assert_eq!(
            parse_query("code=a%2Fb&state=x+y"),
            vec![
                ("code".into(), "a/b".into()),
                ("state".into(), "x y".into())
            ]
        );
        // `%` vor einem Mehrbyte-Zeichen darf nicht abstürzen
        assert_eq!(parse_query("a=%1€").len(), 1);
    }

    #[test]
    fn query_parsing_config_and_lengths() {
        let q = |s: &str| parse_query(s);
        assert_eq!(q("a=%E2%82%AC"), vec![("a".into(), "€".into())]);
        assert_eq!(q("a=b=c&x&=y&"), vec![("a".into(), "b=c".into()), ("".into(), "y".into())]);
        assert_eq!(q("a=%zz1"), vec![("a".into(), "?1".into())]);
        assert_eq!(q("a=%"), vec![("a".into(), "%".into())]);
        assert_eq!(q("a=%4"), vec![("a".into(), "%4".into())]);
        assert_eq!(q("a=%41"), vec![("a".into(), "A".into())]);
        assert_eq!(q("a=%FF%FE"), vec![("a".into(), "\u{fffd}\u{fffd}".into())]);
        assert!(q("").is_empty());
        for junk in ["%%%%", "=&=&=", "a=%€€", "🎬=🎬"] {
            let _ = q(junk);
        }
        // Länge: Sekunden oder Millisekunden
        let t = |l: Option<f64>| Track { length: l, ..Default::default() }.seconds();
        assert_eq!((t(None), t(Some(180.0)), t(Some(180_000.0)), t(Some(3600.0))), (None, Some(180.0), Some(180.0), Some(3600.0)));
        let a = Track { main_artists: vec![Named { id: "1".into(), name: "A".into() }, Named { id: "2".into(), name: "B 🎬".into() }], ..Default::default() };
        assert_eq!(a.artists(), "A, B 🎬");
        assert_eq!(Track::default().artists(), "");
        // Konfiguration: leer, unbekannte Felder, Nutzerkennung stabil
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c, Config::default());
        let c: Config = serde_json::from_str(r#"{"api_key":"  ","mode":"ApiKey","neu":1}"#).unwrap();
        assert!(!c.ready(), "nur Leerzeichen sind kein Schlüssel");
        assert!(serde_json::from_str::<Config>(r#"{"mode":"Zauber"}"#).is_err());
        let mut c = Config::default();
        c.ensure_user_id();
        let id = c.user_id.clone();
        c.ensure_user_id();
        assert!(id.starts_with("cl-") && id.len() > 10 && c.user_id == id);
        assert_eq!(c.base(), DEFAULT_BASE);
        assert!(redirect_uri().starts_with("http://127.0.0.1:"));
        assert!(start_login("   ").is_err());
        // Zufallswerte unterscheiden sich
        assert_ne!(pkce().0, pkce().0);
    }

    #[test]
    fn names_come_as_text_or_object_and_one_bad_track_does_not_spoil_the_page() {
        let p: Page = serde_json::from_str(
            r#"{"tracks":[
                {"id":"a","title":"Eins","mainArtists":["Jon Björk","Ava"],"moods":[{"id":"m","name":"Epic"}],"genres":["Pop"]},
                {"id":"b","title":"Zwei","mainArtists":[{"id":"x","name":"Mia"}]},
                {"id":"c","title":{"kaputt":true}},
                {"id":"d","title":"Vier"}
            ]}"#,
        )
        .unwrap();
        let ids: Vec<&str> = p.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "d"]);
        assert_eq!(p.tracks[0].artists(), "Jon Björk, Ava");
        assert_eq!(p.tracks[0].genres[0].name, "Pop");
        assert_eq!(p.tracks[1].artists(), "Mia");
        assert_eq!(p.tracks[0].moods[0], Named { id: "m".into(), name: "Epic".into() });
    }
}
