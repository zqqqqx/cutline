//! MCP-Endpunkt (Model Context Protocol) für Cutline.
//!
//! - Transport `stdio`: `cutline --mcp <projekt.json>` (headless)
//! - Transport HTTP: `127.0.0.1:<port>/mcp`, nur lokal und nur mit Bearer-Token, für die laufende App
//!
//! Jeder schreibende Tool-Aufruf wird zu [`EditOp`]s und läuft über [`Document::apply`].
//! Er ist damit rückgängig machbar und im Verlauf als „MCP: <Client>“ sichtbar.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cl_core::*;
use serde_json::{json, Value};
use std::result::Result;

pub const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Clone)]
pub struct McpServer {
    pub doc: Arc<Mutex<Document>>,
    pub fonts: Option<Arc<Mutex<cl_fonts::FontLibrary>>>,
    /// Nur lesende Tools erlauben (Schalter in der App).
    pub read_only: bool,
    client: Arc<Mutex<String>>,
}

struct ToolErr(String);
impl<E: std::fmt::Display> From<E> for ToolErr {
    fn from(e: E) -> Self {
        ToolErr(e.to_string())
    }
}
type ToolResult = Result<Value, ToolErr>;

fn secs(v: &Value, key: &str) -> Result<Time, ToolErr> {
    v.get(key)
        .and_then(Value::as_f64)
        .map(Time::from_secs_f64)
        .ok_or_else(|| ToolErr(format!("'{key}' (Sekunden) fehlt")))
}
fn opt_secs(v: &Value, key: &str) -> Option<Time> {
    v.get(key).and_then(Value::as_f64).map(Time::from_secs_f64)
}
fn id<T: std::str::FromStr>(v: &Value, key: &str) -> Result<T, ToolErr> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ToolErr(format!("'{key}' fehlt oder ist keine gültige ID")))
}

struct Tool {
    name: &'static str,
    description: &'static str,
    write: bool,
    schema: fn() -> Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}
fn none() -> Value {
    obj(json!({}), &[])
}

const TOOLS: &[Tool] = &[
    Tool { name: "project_info", description: "Projektname, Format (Auflösung, Bildrate, Seitenverhältnis), Dauer und Anzahl Spuren/Clips.", write: false, schema: none },
    Tool { name: "list_timeline", description: "Alle Spuren mit ihren Clips (ID, Typ, Start/Dauer in Sekunden, Bezeichnung).", write: false, schema: none },
    Tool { name: "list_media", description: "Importierte Medien mit ID, Pfad, Typ, Dauer, Auflösung und Codecs.", write: false, schema: none },
    Tool { name: "list_fonts", description: "Importierte Schriftfamilien der Schriftbibliothek.", write: false, schema: none },
    Tool { name: "history", description: "Änderungsverlauf (älteste zuerst) mit Quelle (Oberfläche, MCP, Plugin).", write: false, schema: none },
    Tool {
        name: "import_media",
        description: "Importiert eine Video-, Audio- oder Bilddatei von einem lokalen Pfad in den Medienpool.",
        write: true,
        schema: || obj(json!({ "path": { "type": "string", "description": "Absoluter Dateipfad" } }), &["path"]),
    },
    Tool {
        name: "add_media_clip",
        description: "Legt ein importiertes Medium auf die Zeitleiste. Videos landen auf der Hauptspur (Magnet hängt hinten an).",
        write: true,
        schema: || obj(json!({
            "media_id": { "type": "string" },
            "start": { "type": "number", "description": "Startzeit in Sekunden (optional)" },
            "duration": { "type": "number", "description": "Dauer in Sekunden (optional, Standard: ganzes Medium bzw. 5 s bei Bildern)" }
        }), &["media_id"]),
    },
    Tool {
        name: "add_text",
        description: "Fügt einen Textclip hinzu (eigene Textspur).",
        write: true,
        schema: || obj(json!({
            "text": { "type": "string" },
            "start": { "type": "number" },
            "duration": { "type": "number" },
            "font_family": { "type": "string", "description": "Familienname einer importierten oder System-Schrift" },
            "size": { "type": "number" },
            "uppercase": { "type": "boolean" }
        }), &["text", "start", "duration"]),
    },
    Tool {
        name: "split_clip",
        description: "Teilt einen Clip an einer Zeitleisten-Position (Sekunden). Gibt die ID des rechten Teils zurück.",
        write: true,
        schema: || obj(json!({ "clip_id": { "type": "string" }, "at": { "type": "number" } }), &["clip_id", "at"]),
    },
    Tool {
        name: "trim_clip",
        description: "Verschiebt Anfang (edge=start) oder Ende (edge=end) eines Clips auf eine Zeitleisten-Position in Sekunden.",
        write: true,
        schema: || obj(json!({
            "clip_id": { "type": "string" },
            "edge": { "type": "string", "enum": ["start", "end"] },
            "to": { "type": "number" }
        }), &["clip_id", "edge", "to"]),
    },
    Tool {
        name: "delete_clip",
        description: "Löscht einen Clip. Auf der Hauptspur rücken die folgenden Clips nach (Magnet).",
        write: true,
        schema: || obj(json!({ "clip_id": { "type": "string" } }), &["clip_id"]),
    },
    Tool {
        name: "set_format",
        description: "Ändert das Projektformat: Seitenverhältnis (z. B. '9:16', '16:9', '1:1') oder width/height, optional fps.",
        write: true,
        schema: || obj(json!({
            "aspect": { "type": "string" },
            "width": { "type": "integer" },
            "height": { "type": "integer" },
            "fps": { "type": "string", "description": "z. B. '30', '29.97', '60'" }
        }), &[]),
    },
    Tool {
        name: "apply_ops",
        description: "Fortgeschritten: wendet eine Liste roher EditOps (JSON, Feld 'op') nacheinander an. Jede Op ist einzeln rückgängig machbar.",
        write: true,
        schema: || obj(json!({ "ops": { "type": "array", "items": { "type": "object" } } }), &["ops"]),
    },
    Tool { name: "undo", description: "Macht die letzte Änderung rückgängig.", write: true, schema: none },
    Tool { name: "redo", description: "Stellt die zuletzt rückgängig gemachte Änderung wieder her.", write: true, schema: none },
];

impl McpServer {
    pub fn new(
        doc: Arc<Mutex<Document>>,
        fonts: Option<Arc<Mutex<cl_fonts::FontLibrary>>>,
    ) -> Self {
        Self {
            doc,
            fonts,
            read_only: false,
            client: Arc::new(Mutex::new("unbekannt".into())),
        }
    }

    pub fn client_name(&self) -> String {
        self.client.lock().unwrap().clone()
    }

    /// Verarbeitet eine JSON-RPC-Nachricht (oder ein Batch). `None` bei reinen Notifications.
    pub fn handle(&self, msg: Value) -> Option<Value> {
        if let Value::Array(batch) = msg {
            let out: Vec<Value> = batch.into_iter().filter_map(|m| self.handle(m)).collect();
            return (!out.is_empty()).then_some(Value::Array(out));
        }
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let id = id?; // Notification (z. B. notifications/initialized): keine Antwort

        let result = match method {
            "initialize" => {
                if let Some(name) = params.pointer("/clientInfo/name").and_then(Value::as_str) {
                    *self.client.lock().unwrap() = name.to_string();
                }
                let requested = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = if SUPPORTED_PROTOCOLS.contains(&requested) {
                    requested
                } else {
                    SUPPORTED_PROTOCOLS[0]
                };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false }, "resources": { "listChanged": false } },
                    "serverInfo": { "name": "cutline", "title": "Cutline Videoeditor", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "Zeiten sind Sekunden (float). Erst list_timeline/list_media lesen, dann ändern. Jede Änderung ist mit 'undo' umkehrbar."
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": TOOLS.iter().filter(|t| !(self.read_only && t.write)).map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": (t.schema)(),
                    "annotations": { "readOnlyHint": !t.write, "destructiveHint": t.name == "delete_clip" }
                })).collect::<Vec<_>>()
            })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                Ok(match self.call_tool(name, &args) {
                    Ok(v) => {
                        json!({ "content": [{ "type": "text", "text": v.to_string() }], "structuredContent": v, "isError": false })
                    }
                    Err(ToolErr(e)) => {
                        json!({ "content": [{ "type": "text", "text": e }], "isError": true })
                    }
                })
            }
            "resources/list" => Ok(json!({ "resources": [
                { "uri": "project://current", "name": "Aktuelles Projekt", "mimeType": "application/json" },
                { "uri": "project://timeline", "name": "Zeitleiste", "mimeType": "application/json" }
            ]})),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
                let doc = self.doc.lock().unwrap();
                let text = match uri {
                    "project://current" => Some(doc.project().to_json()),
                    "project://timeline" => Some(timeline_json(doc.project()).to_string()),
                    _ => None,
                };
                match text {
                    Some(t) => Ok(
                        json!({ "contents": [{ "uri": uri, "mimeType": "application/json", "text": t }] }),
                    ),
                    None => Err((-32002, format!("Ressource nicht gefunden: {uri}"))),
                }
            }
            "" => Err((-32600, "Ungültige Anfrage".into())),
            m => Err((-32601, format!("Methode nicht gefunden: {m}"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        })
    }

    fn apply(&self, op: EditOp) -> Result<OpOutput, ToolErr> {
        let src = OpSource::Mcp(self.client_name());
        Ok(self.doc.lock().unwrap().apply(op, src)?)
    }

    fn call_tool(&self, name: &str, a: &Value) -> ToolResult {
        let tool = TOOLS
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| ToolErr(format!("Unbekanntes Tool: {name}")))?;
        if tool.write && self.read_only {
            return Err(ToolErr(
                "Der MCP-Zugang ist auf „nur lesen“ gestellt".into(),
            ));
        }
        match name {
            "project_info" => {
                let doc = self.doc.lock().unwrap();
                let p = doc.project();
                Ok(json!({
                    "name": p.name,
                    "width": p.settings.width, "height": p.settings.height,
                    "fps": p.settings.fps.as_f64(),
                    "aspect": format!("{:?}", p.settings.aspect),
                    "duration": p.duration().as_secs_f64(),
                    "tracks": p.timeline.tracks.len(),
                    "clips": p.timeline.tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
                    "media": p.media.len(),
                    "main_track_magnet": p.settings.main_track_magnet,
                }))
            }
            "list_timeline" => Ok(timeline_json(self.doc.lock().unwrap().project())),
            "list_media" => {
                let doc = self.doc.lock().unwrap();
                Ok(json!(doc.project().media.iter().map(|m| json!({
                    "id": m.id.to_string(), "path": m.path, "kind": format!("{:?}", m.kind),
                    "duration": m.duration.as_secs_f64(), "width": m.width, "height": m.height,
                    "fps": m.fps.map(|f| f.as_f64()), "video_codec": m.video_codec, "audio_codec": m.audio_codec,
                })).collect::<Vec<_>>()))
            }
            "list_fonts" => {
                let Some(f) = &self.fonts else {
                    return Ok(json!([]));
                };
                let lib = f.lock().unwrap();
                Ok(json!(lib
                    .families()
                    .iter()
                    .map(|(fam, faces)| json!({
                        "family": fam,
                        "styles": faces.iter().map(|f| &f.info.style).collect::<Vec<_>>(),
                        "variable": faces.iter().any(|f| f.info.is_variable()),
                    }))
                    .collect::<Vec<_>>()))
            }
            "history" => {
                let doc = self.doc.lock().unwrap();
                Ok(json!(doc
                    .history()
                    .map(|(l, s)| json!({ "label": l, "source": s }))
                    .collect::<Vec<_>>()))
            }
            "import_media" => {
                let path = PathBuf::from(
                    a.get("path")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ToolErr("'path' fehlt".into()))?,
                );
                if !path.is_file() {
                    return Err(ToolErr(format!("Datei nicht gefunden: {}", path.display())));
                }
                let m = cl_media::probe(&path)?;
                let out = json!({ "media_id": m.id.to_string(), "kind": format!("{:?}", m.kind), "duration": m.duration.as_secs_f64() });
                self.apply(EditOp::ImportMedia { media: m })?;
                Ok(out)
            }
            "add_media_clip" => {
                let mid: MediaId = id(a, "media_id")?;
                let m = self
                    .doc
                    .lock()
                    .unwrap()
                    .project()
                    .media(mid)
                    .cloned()
                    .ok_or_else(|| ToolErr(format!("Medium {mid} nicht gefunden")))?;
                let dur = opt_secs(a, "duration").unwrap_or(if m.kind == MediaKind::Image {
                    Time::from_secs_f64(5.0)
                } else {
                    m.duration
                });
                let start = opt_secs(a, "start")
                    .unwrap_or_else(|| self.doc.lock().unwrap().project().duration());
                let clip = Clip::new(
                    ClipContent::Media {
                        media: mid,
                        kind: m.kind,
                    },
                    start,
                    dur,
                );
                match self.apply(EditOp::AddClip { track: None, clip })? {
                    OpOutput::Clip(c) => Ok(json!({ "clip_id": c.to_string() })),
                    o => Ok(json!(o)),
                }
            }
            "add_text" => {
                let mut style = TextStyle {
                    text: a["text"].as_str().unwrap_or_default().to_string(),
                    ..Default::default()
                };
                style.font_family = a
                    .get("font_family")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(s) = a.get("size").and_then(Value::as_f64) {
                    // wie Inspektor/Import: 0, negative oder riesige Größen bringen das Rendern durcheinander
                    style.size = (s as f32).clamp(8.0, 600.0);
                }
                style.uppercase = a.get("uppercase").and_then(Value::as_bool).unwrap_or(false);
                let clip = Clip::new(
                    ClipContent::Text(style),
                    secs(a, "start")?,
                    secs(a, "duration")?,
                );
                match self.apply(EditOp::AddClip { track: None, clip })? {
                    OpOutput::Clip(c) => Ok(json!({ "clip_id": c.to_string() })),
                    o => Ok(json!(o)),
                }
            }
            "split_clip" => match self.apply(EditOp::SplitClip {
                clip: id(a, "clip_id")?,
                at: secs(a, "at")?,
            })? {
                OpOutput::Clip(c) => Ok(json!({ "right_clip_id": c.to_string() })),
                o => Ok(json!(o)),
            },
            "trim_clip" => {
                let edge = match a.get("edge").and_then(Value::as_str) {
                    Some("start") => Edge::Start,
                    Some("end") => Edge::End,
                    _ => return Err(ToolErr("'edge' muss 'start' oder 'end' sein".into())),
                };
                self.apply(EditOp::TrimClip {
                    clip: id(a, "clip_id")?,
                    edge,
                    to: secs(a, "to")?,
                })?;
                Ok(json!({ "ok": true }))
            }
            "delete_clip" => {
                self.apply(EditOp::RemoveClip {
                    clip: id(a, "clip_id")?,
                })?;
                Ok(json!({ "ok": true }))
            }
            "set_format" => {
                let mut s = self.doc.lock().unwrap().project().settings.clone();
                if let Some(asp) = a.get("aspect").and_then(Value::as_str) {
                    let preset = AspectPreset::PRESETS
                        .iter()
                        .find(|(n, _)| *n == asp)
                        .map(|(_, p)| *p)
                        .ok_or_else(|| ToolErr(format!("Unbekanntes Seitenverhältnis '{asp}'")))?;
                    s.apply_aspect(preset);
                }
                if let (Some(w), Some(h)) = (
                    a.get("width").and_then(Value::as_u64),
                    a.get("height").and_then(Value::as_u64),
                ) {
                    if !(16..=16384).contains(&w) || !(16..=16384).contains(&h) {
                        return Err(ToolErr(
                            "Auflösung muss zwischen 16 und 16384 liegen".into(),
                        ));
                    }
                    s.width = w as u32 & !1;
                    s.height = h as u32 & !1;
                    s.aspect = AspectPreset::Custom;
                }
                if let Some(f) = a.get("fps").and_then(Value::as_str) {
                    s.fps = Rational::parse(f)
                        .filter(|r| r.as_f64() > 0.0 && r.as_f64() <= 240.0)
                        .ok_or_else(|| ToolErr(format!("Ungültige Bildrate '{f}'")))?;
                }
                let out = json!({ "width": s.width, "height": s.height, "fps": s.fps.as_f64() });
                self.apply(EditOp::SetSettings { settings: s })?;
                Ok(out)
            }
            "apply_ops" => {
                let ops = a
                    .get("ops")
                    .and_then(Value::as_array)
                    .ok_or_else(|| ToolErr("'ops' fehlt".into()))?;
                let mut results = Vec::new();
                for (i, raw) in ops.iter().enumerate() {
                    let op: EditOp = serde_json::from_value(raw.clone())
                        .map_err(|e| ToolErr(format!("Op {i}: {e}")))?;
                    results.push(json!(self
                        .apply(op)
                        .map_err(|ToolErr(e)| ToolErr(format!("Op {i}: {e}")))?));
                }
                Ok(json!({ "results": results }))
            }
            "undo" => Ok(json!({ "undone": self.doc.lock().unwrap().undo()? })),
            "redo" => Ok(json!({ "redone": self.doc.lock().unwrap().redo()? })),
            _ => unreachable!("Tool-Liste und match stimmen überein"),
        }
    }

    /// Zeilenbasiertes JSON-RPC über stdin/stdout (MCP-Transport „stdio“).
    /// `after` läuft nach jeder Nachricht (Headless-Modus speichert dort).
    pub fn serve_stdio(&self, mut after: impl FnMut()) -> std::io::Result<()> {
        self.serve(std::io::stdin().lock(), std::io::stdout().lock(), &mut after)
    }

    fn serve(&self, input: impl BufRead, mut stdout: impl Write, after: &mut dyn FnMut()) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let resp = match serde_json::from_str::<Value>(&line) {
                Ok(v) => self.handle(v),
                Err(e) => Some(
                    json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }),
                ),
            };
            after(); // vor dem Antworten: auch bei geschlossener Ausgabe gespeichert
            if let Some(r) = resp {
                writeln!(stdout, "{r}")?;
                stdout.flush()?;
            }
        }
        Ok(())
    }

    /// Startet den lokalen HTTP-Endpunkt (POST /mcp, JSON-Antworten) in einem Hintergrund-Thread.
    /// Bindet ausschließlich an 127.0.0.1 und verlangt `Authorization: Bearer <token>`.
    pub fn serve_http(
        self,
        port: u16,
        token: String,
    ) -> std::io::Result<(SocketAddr, std::thread::JoinHandle<()>)> {
        self.serve_http_until(
            port,
            token,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
    }

    /// Wie [`serve_http`](Self::serve_http), endet aber, sobald `stop` gesetzt wird
    /// (der Port wird dann sofort frei).
    pub fn serve_http_until(
        self,
        port: u16,
        token: String,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::io::Result<(SocketAddr, std::thread::JoinHandle<()>)> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let handle = std::thread::Builder::new()
            .name("cl-mcp-http".into())
            .spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let srv = self.clone();
                            let token = token.clone();
                            std::thread::spawn(move || {
                                let end = stream.try_clone();
                                let _ = srv.http_connection(stream, &token);
                                // geordnet schließen: erst FIN, dann (begrenzt) warten, bis der
                                // Client schließt – sofort geschlossen kam unter Last ab und zu RST
                                // statt der schon gesendeten Antwort an
                                if let Ok(s) = end {
                                    let _ = s.shutdown(std::net::Shutdown::Write);
                                    let _ = std::io::copy(&mut (&s).take(64 * 1024), &mut std::io::sink());
                                }
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(40))
                        }
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(40)),
                    }
                }
            })?;
        Ok((addr, handle))
    }

    fn http_connection(&self, stream: TcpStream, token: &str) -> std::io::Result<()> {
        stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut out = stream;
        // Kopfzeilen vor der Anmeldung höchstens 64 KiB lesen (sonst unbegrenzter Speicher)
        let mut head = (&mut reader).take(64 * 1024);
        let mut request_line = String::new();
        head.read_line(&mut request_line)?;
        let mut parts = request_line.split_whitespace();
        let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let (mut len, mut auth, mut origin, mut host) =
            (0usize, String::new(), None::<String>, String::new());
        let mut complete = false;
        loop {
            let mut h = String::new();
            if head.read_line(&mut h)? == 0 {
                break;
            }
            if h == "\r\n" || h == "\n" {
                complete = true;
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().unwrap_or(0),
                    "authorization" => auth = v.trim().to_string(),
                    "origin" => origin = Some(v.trim().to_string()),
                    "host" => host = v.trim().to_ascii_lowercase(),
                    _ => {}
                }
            }
        }
        let reply = |out: &mut TcpStream, status: &str, body: &str| -> std::io::Result<()> {
            write!(out, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        };
        if !complete {
            return reply(&mut out, "431 Request Header Fields Too Large", "{}");
        }
        // Vor einer Fehlerantwort den angekündigten Körper (höchstens 8 MiB) lesen und verwerfen:
        // schließt der Server mit ungelesenen Daten, schickt das System RST statt FIN, und ein
        // Client, der Kopf und Körper getrennt sendet, sieht statt 401/403 einen Verbindungsabbruch.
        let mut fail = |out: &mut TcpStream, status: &str, body: &str| -> std::io::Result<()> {
            let _ = std::io::copy(&mut (&mut reader).take(len.min(8 * 1024 * 1024) as u64), &mut std::io::sink());
            reply(out, status, body)
        };
        // Schutz gegen DNS-Rebinding: Host und Origin müssen genau localhost sein.
        if !local_authority(&host) {
            return fail(&mut out, "403 Forbidden", r#"{"error":"host"}"#);
        }
        if let Some(o) = origin {
            if !o.strip_prefix("http://").is_some_and(local_authority) {
                return fail(&mut out, "403 Forbidden", r#"{"error":"origin"}"#);
            }
        }
        if path != "/mcp" {
            return fail(&mut out, "404 Not Found", r#"{"error":"not found"}"#);
        }
        if auth != format!("Bearer {token}") {
            return fail(&mut out, "401 Unauthorized", r#"{"error":"token"}"#);
        }
        if method != "POST" {
            return fail(
                &mut out,
                "405 Method Not Allowed",
                r#"{"error":"POST only"}"#,
            );
        }
        if len > 8 * 1024 * 1024 {
            return reply(&mut out, "413 Payload Too Large", "{}");
        }
        let mut body = vec![0; len];
        reader.read_exact(&mut body)?;
        match serde_json::from_slice::<Value>(&body) {
            Ok(v) => match self.handle(v) {
                Some(r) => reply(&mut out, "200 OK", &r.to_string()),
                None => write!(
                    out,
                    "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            },
            Err(e) => reply(
                &mut out,
                "400 Bad Request",
                &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}})
                    .to_string(),
            ),
        }
    }
}

/// `127.0.0.1` oder `localhost`, optional mit `:<Port>` – sonst nichts (kein Präfixvergleich).
fn local_authority(a: &str) -> bool {
    let (host, port) = match a.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (a, None),
    };
    matches!(host, "127.0.0.1" | "localhost")
        && port.is_none_or(|p| (1..=5).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
}

fn timeline_json(p: &Project) -> Value {
    json!(p
        .timeline
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| json!({
            "index": i,
            "track_id": t.id.to_string(),
            "kind": format!("{:?}", t.kind),
            "main": t.main, "locked": t.locked, "hidden": t.hidden, "muted": t.muted,
            "clips": t.clips.iter().map(|c| json!({
                "clip_id": c.id.to_string(),
                "label": c.label(p),
                "start": c.start.as_secs_f64(),
                "duration": c.duration.as_secs_f64(),
                "end": c.end().as_secs_f64(),
                "source_in": c.source_in.as_secs_f64(),
            })).collect::<Vec<_>>()
        }))
        .collect::<Vec<_>>())
}

/// Zufälliges Token für den HTTP-Transport.
pub fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> McpServer {
        McpServer::new(
            Arc::new(Mutex::new(Document::new(Project::new("MCP-Test")))),
            None,
        )
    }

    fn call(s: &McpServer, name: &str, args: Value) -> Value {
        let r = s.handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
        r["result"].clone()
    }

    #[test]
    fn stdio_reports_each_message_before_a_broken_line() {
        let s = server();
        let mut input = format!(
            "{}\n",
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"add_text","arguments":{"text":"Hallo","start":0.0,"duration":2.0}}})
        )
        .into_bytes();
        input.extend(b"\xff\xfe\n"); // ungültiges UTF-8 beendet die Schleife mit Fehler
        let mut seen = Vec::new();
        let r = s.serve(&input[..], Vec::new(), &mut || seen.push(s.doc.lock().unwrap().revision()));
        assert!(r.is_err());
        assert_eq!(seen.len(), 1);
        assert_ne!(seen[0], 0, "Änderung war beim Rückruf schon da");
    }

    #[test]
    fn handshake_and_listing() {
        let s = server();
        let r = s
            .handle(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","clientInfo":{"name":"test-client"}}}))
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(s.client_name(), "test-client");
        assert!(s
            .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none());
        let tools = s
            .handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .unwrap();
        let names: Vec<_> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"split_clip".to_string()));
        let unknown = s
            .handle(json!({"jsonrpc":"2.0","id":3,"method":"foo"}))
            .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn edit_via_tools_is_undoable_and_attributed() {
        let s = server();
        s.handle(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"clientInfo":{"name":"test-client"}}}));
        let r = call(
            &s,
            "add_text",
            json!({"text":"Hallo","start":0.0,"duration":4.0,"font_family":"Impact","uppercase":true}),
        );
        assert_eq!(r["isError"], false, "{r}");
        let clip = r["structuredContent"]["clip_id"]
            .as_str()
            .unwrap()
            .to_string();
        let r = call(&s, "split_clip", json!({"clip_id": clip, "at": 1.5}));
        assert_eq!(r["isError"], false, "{r}");
        let tl = call(&s, "list_timeline", json!({}));
        assert_eq!(
            tl["structuredContent"][0]["clips"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let h = call(&s, "history", json!({}));
        assert_eq!(h["structuredContent"][1]["source"]["name"], "test-client");
        assert_eq!(
            call(&s, "undo", json!({}))["structuredContent"]["undone"],
            "Teilen"
        );
        let r = call(&s, "set_format", json!({"aspect":"9:16","fps":"60"}));
        assert_eq!(r["structuredContent"]["width"], 1080);
        assert_eq!(r["structuredContent"]["height"], 1920);
        // Fehler kommen als isError zurück, nicht als Protokollfehler
        let r = call(&s, "split_clip", json!({"clip_id": clip, "at": 99.0}));
        assert_eq!(r["isError"], true);
        // Rohe EditOps
        let r = call(
            &s,
            "apply_ops",
            json!({"ops":[{"op":"rename","name":"Per MCP"}]}),
        );
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(s.doc.lock().unwrap().project().name, "Per MCP");
    }

    #[test]
    fn read_only_blocks_writes() {
        let mut s = server();
        s.read_only = true;
        let tools = s
            .handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .unwrap();
        assert!(tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true));
        assert_eq!(
            call(&s, "add_text", json!({"text":"x","start":0,"duration":1}))["isError"],
            true
        );
    }

    #[test]
    fn http_transport_requires_token() {
        let s = server();
        let (addr, _h) = s.serve_http(0, "geheim".into()).unwrap();
        // in einem Stück senden (`write!` auf den Strom schrieb in vielen kleinen Teilen)
        let post = |auth: &str, body: &str| -> String {
            let req = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: {auth}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", addr.port(), body.len());
            http(addr, req.as_bytes())
        };
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        assert!(post("Bearer falsch", body).starts_with("HTTP/1.1 401"));
        let ok = post("Bearer geheim", body);
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains(r#""result":{}"#));
        let raw = |req: &[u8]| -> String {
            let mut c = TcpStream::connect(addr).unwrap();
            let _ = c.write_all(req);
            let mut s = String::new();
            let _ = c.read_to_string(&mut s);
            s
        };
        let req = |host: &str, origin: &str| {
            format!("POST /mcp HTTP/1.1\r\nHost: {host}\r\n{origin}Authorization: Bearer geheim\r\nContent-Length: {}\r\n\r\n{body}", body.len())
        };
        // DNS-Rebinding: fremder Host
        assert!(raw(req("evil.com", "").as_bytes()).starts_with("HTTP/1.1 403"));
        // Origin nur exakt, kein Präfix
        assert!(raw(req("localhost", "Origin: http://localhost.evil.com\r\n").as_bytes()).starts_with("HTTP/1.1 403"));
        assert!(raw(req("localhost:9", "Origin: http://localhost:9\r\n").as_bytes()).starts_with("HTTP/1.1 200"));
        // Kopfbereich ohne Ende nach 64 KiB wird nicht weiter gelesen
        let mut big = b"POST /mcp HTTP/1.1\r\nX: ".to_vec();
        big.resize(64 * 1024, b'a'); // genau die Grenze: Server liest alles, kein RST
        assert!(raw(&big).starts_with("HTTP/1.1 431"));
    }

    #[test]
    fn local_authority_is_exact() {
        assert!(local_authority("127.0.0.1") && local_authority("localhost:8080"));
        for bad in ["localhost.evil.com", "127.0.0.1.evil", "localhost:", "localhost:80x", "x"] {
            assert!(!local_authority(bad), "{bad}");
        }
    }

    fn rpc(s: &McpServer, method: &str, params: Value) -> Value {
        s.handle(json!({"jsonrpc":"2.0","id":7,"method":method,"params":params})).unwrap()
    }

    fn http(addr: SocketAddr, raw: &[u8]) -> String {
        let mut c = TcpStream::connect(addr).unwrap();
        let _ = c.write_all(raw);
        let _ = c.shutdown(std::net::Shutdown::Write);
        let mut s = String::new();
        let _ = c.read_to_string(&mut s);
        s
    }

    /// Client sendet Kopf und Körper getrennt, Token falsch: die 401 kommt an (der Server liest den
    /// Körper vorher, sonst bricht die Verbindung mit RST ab und die Antwort geht verloren).
    #[test]
    fn error_reply_arrives_when_body_follows_the_head() {
        let (addr, _h) = server().serve_http(0, "geheim".into()).unwrap();
        let body = vec![b' '; 1 << 20];
        for (host, auth, want) in [("localhost", "Bearer falsch", "HTTP/1.1 401"), ("evil.com", "Bearer geheim", "HTTP/1.1 403")] {
            let mut c = TcpStream::connect(addr).unwrap();
            let head = format!("POST /mcp HTTP/1.1\r\nHost: {host}\r\nAuthorization: {auth}\r\nContent-Length: {}\r\n\r\n", body.len());
            c.write_all(head.as_bytes()).unwrap();
            let _ = c.write_all(&body);
            let mut s = String::new();
            c.read_to_string(&mut s).unwrap_or_else(|e| panic!("{host}: {e} {s:?}"));
            assert!(s.starts_with(want), "{s}");
        }
    }

    #[test]
    fn token_must_match_exactly() {
        let (addr, _h) = server().serve_http(0, "geheim".into()).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let req = |auth: Option<&str>| {
            let a = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
            format!("POST /mcp HTTP/1.1\r\nHost: localhost\r\n{a}Content-Length: {}\r\n\r\n{body}", body.len())
        };
        for bad in [None, Some(""), Some("Bearer"), Some("Bearer "), Some("Basic geheim"), Some("bearer geheim"), Some("Bearer geheim2"), Some("Bearer geheimx"), Some("Bearer gehei")] {
            assert!(http(addr, req(bad).as_bytes()).starts_with("HTTP/1.1 401"), "{bad:?}");
        }
        assert!(http(addr, req(Some("Bearer geheim")).as_bytes()).starts_with("HTTP/1.1 200"));
        // falsche Methode/Pfad
        let get = "GET /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer geheim\r\n\r\n";
        assert!(http(addr, get.as_bytes()).starts_with("HTTP/1.1 405"));
        let other = format!("POST /mcp/x HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer geheim\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        assert!(http(addr, other.as_bytes()).starts_with("HTTP/1.1 404"));
        // übergroße Anfrage wird abgelehnt, ohne den Körper zu lesen
        let huge = "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer geheim\r\nContent-Length: 999999999999\r\n\r\n";
        assert!(http(addr, huge.as_bytes()).starts_with("HTTP/1.1 413"));
        // kaputtes JSON → 400 mit Parse-Fehler
        let junk = "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer geheim\r\nContent-Length: 5\r\n\r\n{\"a\":";
        let r = http(addr, junk.as_bytes());
        assert!(r.starts_with("HTTP/1.1 400") && r.contains("-32700"), "{r}");
        // Müll statt HTTP
        assert!(!http(addr, b"\x00\xff\xfe garbage\r\n\r\n").starts_with("HTTP/1.1 200"));
        assert!(http(addr, b"").is_empty() || !http(addr, b"").starts_with("HTTP/1.1 200"));
        // Server lebt noch
        assert!(http(addr, req(Some("Bearer geheim")).as_bytes()).starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn read_only_rejects_every_write_tool() {
        let mut s = server();
        s.read_only = true;
        let before = s.doc.lock().unwrap().project().clone();
        for t in TOOLS.iter() {
            let r = call(&s, t.name, json!({"text":"x","start":0,"duration":1,"path":"x","ops":[{"op":"rename","name":"x"}]}));
            if t.write {
                assert_eq!(r["isError"], true, "{}", t.name);
                assert!(r["content"][0]["text"].as_str().unwrap().contains("nur lesen"), "{}", t.name);
            } else {
                assert_eq!(r["isError"], false, "{}: {r}", t.name);
            }
        }
        assert_eq!(s.doc.lock().unwrap().project(), &before);
        assert_eq!(s.doc.lock().unwrap().undo_entries().len(), 0);
    }

    #[test]
    fn broken_json_rpc_messages() {
        let s = server();
        // ohne Methode, mit ID
        assert_eq!(s.handle(json!({"jsonrpc":"2.0","id":1})).unwrap()["error"]["code"], -32600);
        // keine Antwort auf Notifications, auch nicht im Batch
        assert!(s.handle(json!([{"jsonrpc":"2.0","method":"ping"}])).is_none());
        let b = s.handle(json!([{"jsonrpc":"2.0","id":1,"method":"ping"}, {"jsonrpc":"2.0","method":"x"}, {"jsonrpc":"2.0","id":2,"method":"nope"}])).unwrap();
        assert_eq!(b.as_array().unwrap().len(), 2);
        // falsche Typen: keine Panik
        for m in [json!(5), json!("x"), json!(null), json!([]), json!({"id":{"a":1},"method":5}), json!({"id":1,"method":"tools/call","params":5}), json!({"id":1,"method":"tools/call","params":{"name":7,"arguments":"x"}})] {
            let _ = s.handle(m);
        }
        let r = rpc(&s, "tools/call", json!({"name":"gibt_es_nicht"}));
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("Unbekanntes Tool"));
        assert_eq!(rpc(&s, "resources/read", json!({"uri":"project://geheim"}))["error"]["code"], -32002);
        assert!(rpc(&s, "resources/read", json!({"uri":"project://current"}))["result"]["contents"][0]["text"].as_str().is_some());
        // tiefe Verschachtelung im Batch
        let mut deep = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
        for _ in 0..50 {
            deep = json!([deep]);
        }
        assert!(s.handle(deep).is_some());
        // unbekannte Protokollversion → neueste unterstützte
        assert_eq!(rpc(&s, "initialize", json!({"protocolVersion":"1999-01-01"}))["result"]["protocolVersion"], SUPPORTED_PROTOCOLS[0]);
    }

    #[test]
    fn out_of_range_values_are_rejected_or_clamped() {
        let s = server();
        for args in [
            json!({"text":"x","start":0,"duration":0}),
            json!({"text":"x","start":0,"duration":-1}),
            json!({"text":"x","start":1e300,"duration":1}),
            json!({"text":"x","start":0,"duration":1e300}),
            json!({"text":"x","start":"0","duration":1}),
            json!({"text":"x","duration":1}),
        ] {
            assert_eq!(call(&s, "add_text", args.clone())["isError"], true, "{args}");
        }
        assert_eq!(s.doc.lock().unwrap().undo_entries().len(), 0);
        // negativer Start → 0; Fehler: Schriftgröße 0/negativ/riesig wurde übernommen
        for (size, want) in [(-5.0, 8.0), (0.0, 8.0), (1e9, 600.0), (72.0, 72.0)] {
            let r = call(&s, "add_text", json!({"text":"🎬 שלום","start":-3,"duration":1,"size":size}));
            assert_eq!(r["isError"], false, "{r}");
            let id: ClipId = r["structuredContent"]["clip_id"].as_str().unwrap().parse().unwrap();
            let doc = s.doc.lock().unwrap();
            let (_, c) = doc.project().find_clip(id).unwrap();
            assert_eq!(c.start, Time::ZERO);
            let ClipContent::Text(st) = &c.content else { panic!() };
            assert_eq!(st.size, want, "{size}");
        }
        for args in [json!({"width":8,"height":100}), json!({"width":20000,"height":100}), json!({"aspect":"7:3"}), json!({"fps":"0"}), json!({"fps":"500"}), json!({"fps":"abc"}), json!({"fps":"NaN"})] {
            assert_eq!(call(&s, "set_format", args.clone())["isError"], true, "{args}");
        }
        let r = call(&s, "set_format", json!({"width":1001,"height":721,"fps":"23.976"}));
        assert_eq!((r["structuredContent"]["width"].as_u64(), r["structuredContent"]["height"].as_u64()), (Some(1000), Some(720)));
        assert_eq!(s.doc.lock().unwrap().project().settings.fps, Rational::new(24000, 1001));
        // Rohe Ops mit unendlichen Zahlen (f32-Überlauf) ändern nichts
        let clip = s.doc.lock().unwrap().project().timeline.tracks[0].clips[0].id;
        let r = call(&s, "apply_ops", json!({"ops":[{"op":"set_transform","clip":clip.to_string(),"transform":{"x":1e300,"y":0,"scale":1,"rotation_deg":0}}]}));
        assert_eq!(r["isError"], true, "{r}");
        let json = s.doc.lock().unwrap().project().to_json();
        assert!(Project::from_json(&json).is_ok());
        // kaputte Op mitten in der Liste: davor Angewandtes bleibt, Fehler nennt die Nummer
        let r = call(&s, "apply_ops", json!({"ops":[{"op":"rename","name":"A"},{"op":"zaubern"}]}));
        assert!(r["content"][0]["text"].as_str().unwrap().starts_with("Op 1"));
        assert_eq!(call(&s, "apply_ops", json!({"ops":"x"}))["isError"], true);
    }

    #[test]
    fn every_tool_once() {
        let s = server();
        assert_eq!(call(&s, "project_info", json!({}))["structuredContent"]["clips"], 0);
        assert_eq!(call(&s, "list_media", json!({}))["structuredContent"], json!([]));
        assert_eq!(call(&s, "list_fonts", json!({}))["structuredContent"], json!([]));
        assert_eq!(call(&s, "list_timeline", json!({}))["structuredContent"], json!([]));
        assert_eq!(call(&s, "history", json!({}))["structuredContent"], json!([]));
        assert_eq!(call(&s, "undo", json!({}))["isError"], true);
        assert_eq!(call(&s, "redo", json!({}))["isError"], true);
        assert_eq!(call(&s, "import_media", json!({"path":"C:/gibt/es/nicht.mp4"}))["isError"], true);
        assert_eq!(call(&s, "import_media", json!({}))["isError"], true);
        assert_eq!(call(&s, "add_media_clip", json!({"media_id":"keine-id"}))["isError"], true);
        assert_eq!(call(&s, "add_media_clip", json!({"media_id":MediaId::new().to_string()}))["isError"], true);
        let id = call(&s, "add_text", json!({"text":"a","start":0,"duration":2}))["structuredContent"]["clip_id"].clone();
        assert_eq!(call(&s, "trim_clip", json!({"clip_id":id,"edge":"mitte","to":1}))["isError"], true);
        assert_eq!(call(&s, "trim_clip", json!({"clip_id":id,"edge":"end","to":1}))["isError"], false);
        assert_eq!(call(&s, "delete_clip", json!({"clip_id":id}))["isError"], false);
        assert_eq!(call(&s, "delete_clip", json!({"clip_id":id}))["isError"], true);
        assert_eq!(call(&s, "undo", json!({}))["structuredContent"]["undone"], "Clip löschen");
        assert_eq!(call(&s, "redo", json!({}))["structuredContent"]["redone"], "Clip löschen");
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cl-mcp-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let v = dir.join("Ä b 🎬.mp4");
        assert!(cl_media::ffmpeg_command().unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=160x90:r=25:d=1", "-threads", "2", "-pix_fmt", "yuv420p"])
            .arg(&v).status().unwrap().success());
        let r = call(&s, "import_media", json!({"path": v}));
        assert_eq!(r["isError"], false, "{r}");
        let mid = r["structuredContent"]["media_id"].clone();
        let r = call(&s, "add_media_clip", json!({"media_id": mid}));
        assert_eq!(r["isError"], false, "{r}");
        let info = call(&s, "project_info", json!({}));
        assert!((info["structuredContent"]["duration"].as_f64().unwrap() - 1.0).abs() < 0.1, "{info}");
        assert_eq!(call(&s, "list_media", json!({}))["structuredContent"][0]["kind"], "Video");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
