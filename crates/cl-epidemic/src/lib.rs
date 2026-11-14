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

pub type Result<T> = std::result::Result<T, String>;

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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Named {
    pub id: String,
    pub name: String,
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

fn err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401, _) => "Anmeldung abgelaufen oder ungültig".into(),
        ureq::Error::Status(403, _) => "Kein Zugriff (Abo oder Partnervertrag prüfen)".into(),
        ureq::Error::Status(429, _) => "Zu viele Anfragen, bitte kurz warten".into(),
        ureq::Error::Status(c, r) => format!(
            "Epidemic Sound antwortet mit {c}: {}",
            r.into_string()
                .unwrap_or_default()
                .chars()
                .take(160)
                .collect::<String>()
        ),
        ureq::Error::Transport(t) => format!("Keine Verbindung zu Epidemic Sound ({t})"),
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
            .map_err(err)?
            .into_json::<serde_json::Value>()
            .map_err(|e| e.to_string())
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
        serde_json::from_value(self.get(path, &params)?).map_err(|e| e.to_string())
    }

    fn list(&self, path: &str, key: &str) -> Result<Vec<Named>> {
        let v = self.get(path, &[("limit", "20".into()), ("type", "featured".into())])?;
        serde_json::from_value(v.get(key).cloned().unwrap_or_default()).map_err(|e| e.to_string())
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
        let url = self.download_url(id, true)?;
        let resp = self.agent.get(&url).call().map_err(err)?;
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = dest.with_extension("part");
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut resp.into_reader(), &mut f).map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
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
        .map_err(|e| e.to_string())
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
            .map_err(err)?;
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
        self.listener
            .set_nonblocking(true)
            .map_err(|e| e.to_string())?;
        let until = std::time::Instant::now() + timeout;
        let (stream, _) = loop {
            match self.listener.accept() {
                Ok(s) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() > until {
                        return Err("Anmeldung abgebrochen (Zeit abgelaufen)".into());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e.to_string()),
            }
        };
        stream.set_nonblocking(false).ok();
        let mut line = String::new();
        BufReader::new(&stream)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let target = line.split_whitespace().nth(1).unwrap_or("");
        let params = parse_query(target.split_once('?').map(|x| x.1).unwrap_or(""));
        let get = |k: &str| params.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone());
        let ok = get("state").as_deref() == Some(self.state.as_str()) && get("code").is_some();
        let html = if ok {
            "Cutline ist mit Epidemic Sound verbunden. Du kannst dieses Fenster schließen."
        } else {
            "Anmeldung fehlgeschlagen. Bitte in Cutline erneut versuchen."
        };
        let body = format!("<!doctype html><meta charset=utf-8><title>Cutline</title><body style=\"font-family:sans-serif;background:#111;color:#eee;display:grid;place-items:center;height:90vh\"><p>{html}</p>");
        let _ = write!(&stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        if !ok {
            return Err(get("error_description")
                .or_else(|| get("error"))
                .unwrap_or_else(|| "Ungültiger Rückruf".into()));
        }
        exchange(
            &self.client_id,
            &get("code").unwrap_or_default(),
            &self.verifier,
        )
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

fn token_request(form: &[(&str, &str)]) -> Result<Tokens> {
    let r: TokenResponse = agent()
        .post(TOKEN_URL)
        .send_form(form)
        .map_err(err)?
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
    ])
}

pub fn refresh(client_id: &str, refresh_token: &str) -> Result<Tokens> {
    token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ])
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
}
