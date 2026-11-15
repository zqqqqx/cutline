//! Plugin-System, Teil 1: Manifest-Format, Validierung und Registry.
//!
//! Zwei Arten:
//! 1. `asset-pack`: nur Daten (Effekte/Filter als FFmpeg-Ketten, Übergänge als WGSL, Sounds,
//!    Sticker), kein Code
//! 2. `integration`: eingebaute Anbindung an einen Dienst mit deklarierten Rechten

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("{path}: Manifest nicht lesbar: {msg}")]
    Parse { path: PathBuf, msg: String },
    #[error("{id}: {msg}")]
    Invalid { id: String, msg: String },
    #[error("E/A-Fehler: {0}")]
    Io(#[from] std::io::Error),
}

pub const MANIFEST_FILE: &str = "plugin.toml";
pub const API_VERSION: u32 = 1;

/// Rechte, die ein Plugin anfordern kann. Unbekannte Rechte machen das Manifest ungültig.
pub const KNOWN_PERMISSIONS: &[&str] = &[
    "project.read",
    "project.write",
    "media.read",
    "fs.read",
    "fs.write",
    "net",
    "ui.panel",
    "ui.command",
    "render",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginKind {
    AssetPack,
    /// In Cutline eingebaute Anbindung an einen Dienst (`entry = "builtin:<name>"`),
    /// standardmäßig aus und nur mit Zugangsdaten des Nutzers nutzbar.
    Integration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssetKind {
    VideoEffect,
    BodyEffect,
    Transition,
    Filter,
    TextAnimation,
    ClipAnimation,
    TextTemplate,
    Sticker,
    Font,
    Sound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ParamSpec {
    Float {
        id: String,
        min: f32,
        max: f32,
        default: f32,
    },
    Int {
        id: String,
        min: i32,
        max: i32,
        default: i32,
    },
    Bool {
        id: String,
        default: bool,
    },
    Color {
        id: String,
        default: String,
    },
    Choice {
        id: String,
        options: Vec<String>,
        default: String,
    },
}

impl ParamSpec {
    pub fn id(&self) -> &str {
        match self {
            ParamSpec::Float { id, .. }
            | ParamSpec::Int { id, .. }
            | ParamSpec::Bool { id, .. }
            | ParamSpec::Color { id, .. }
            | ParamSpec::Choice { id, .. } => id,
        }
    }
}

/// Lokalisierter Text: `{ de = "…", en = "…" }`.
pub type LocalizedText = BTreeMap<String, String>;

pub fn localized<'a>(t: &'a LocalizedText, lang: &str) -> &'a str {
    t.get(lang)
        .or_else(|| t.get("en"))
        .or_else(|| t.values().next())
        .map(String::as_str)
        .unwrap_or("?")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetEntry {
    pub id: String,
    pub kind: AssetKind,
    pub name: LocalizedText,
    #[serde(default)]
    pub category: Vec<String>,
    /// WGSL-Shader relativ zum Plugin-Ordner (nur Übergänge, dort Pflicht).
    pub shader: Option<String>,
    /// Übergang mit Shader: eingebauter `xfade`-Übergang, wenn keine GPU da ist oder das Paket fehlt
    /// (Pflicht bei Shader-Übergängen).
    pub fallback: Option<String>,
    /// FFmpeg-Filterkette (Effekt/Filter) bzw. `xfade`-Name oder `custom:<Ausdruck>` (Übergang).
    /// Platzhalter: `{intensity}` (0…1) und `{<param-id>}`.
    pub ffmpeg: Option<String>,
    /// Datei relativ zum Plugin-Ordner (Sound/Sticker: Pflicht, Endung siehe [`SOUND_EXT`]/[`STICKER_EXT`]).
    pub file: Option<String>,
    /// Abweichende Angaben für dieses Asset (Standard: die des Pakets)
    pub license: Option<String>,
    pub author: Option<String>,
    pub source: Option<String>,
    /// Schlagworte: die Suche in den Tabs findet das Asset auch darüber.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub params: Vec<ParamSpec>,
}

impl AssetEntry {
    /// Standardwerte der Shader-Parameter in Reihenfolge (`params.p0` …): nur Zahl, Ganzzahl
    /// und Schalter, höchstens [`MAX_SHADER_PARAMS`].
    pub fn shader_params(&self) -> Result<Vec<f32>, String> {
        if self.params.len() > MAX_SHADER_PARAMS {
            return Err(format!("höchstens {MAX_SHADER_PARAMS} Parameter"));
        }
        self.params
            .iter()
            .map(|p| match p {
                ParamSpec::Float { default, .. } => Ok(*default),
                ParamSpec::Int { default, .. } => Ok(*default as f32),
                ParamSpec::Bool { default, .. } => Ok(*default as u8 as f32),
                _ => Err(format!("Parameter '{}': im Shader nur Zahlen und Schalter", p.id())),
            })
            .collect()
    }

    /// FFmpeg-Kette mit eingesetzten Standardwerten; {intensity} bleibt für den Host.
    pub fn ffmpeg_filled(&self) -> Option<String> {
        let mut c = self.ffmpeg.clone()?;
        for prm in &self.params {
            let v = match prm {
                ParamSpec::Float { default, .. } => default.to_string(),
                ParamSpec::Int { default, .. } => default.to_string(),
                ParamSpec::Bool { default, .. } => (*default as u8).to_string(),
                ParamSpec::Color { default, .. } | ParamSpec::Choice { default, .. } => {
                    default.clone()
                }
            };
            c = c.replace(&format!("{{{}}}", prm.id()), &v);
        }
        Some(c)
    }
}

/// Parameter eines Shader-Übergangs (`params.p0` … `params.p7`).
pub const MAX_SHADER_PARAMS: usize = 8;

/// Erlaubte Endungen für Paket-Sounds (wie die eigene Bibliothek).
pub const SOUND_EXT: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac", "opus"];
/// Erlaubte Endungen für Paket-Sticker. Bewegte Sticker: GIF oder APNG mit Endung `.png`
/// (erkannt am Inhalt, siehe `cl_media::animation`).
pub const STICKER_EXT: &[&str] = &["png", "webp", "svg", "jpg", "jpeg", "gif"];

/// Relativer Pfad ohne `..`, Wurzel oder Laufwerk (bleibt im Plugin-Ordner).
pub fn relative_inside(f: &str) -> bool {
    !f.is_empty()
        && !f.contains("..")
        && Path::new(f)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Filter, die Paket-Ketten nutzen dürfen: nur Bildberechnung, kein Datei-, Netz- oder
/// Prozesszugriff. Neue Filter hier eintragen (Test: jede Kette der mitgelieferten Pakete).
pub const ALLOWED_FILTERS: &[&str] = &[
    "avgblur", "boxblur", "cas", "chromashift", "colorbalance", "colorchannelmixer",
    "colorcontrast", "colorize", "colorlevels", "colortemperature", "convolution", "crop", "curves",
    "deband", "drawbox", "drawgrid", "edgedetect", "eq", "exposure", "format", "gblur", "geq",
    "gradfun", "hflip", "hue", "huesaturation", "lagfun", "lenscorrection", "lutrgb", "lutyuv",
    "monochrome", "negate", "noise", "overlay", "pixelize", "rgbashift", "scale", "shuffleplanes",
    "smartblur", "split", "tmix", "unsharp", "vflip", "vibrance", "vignette",
];

/// Teile von `s` an `sep` außerhalb von '…' (FFmpeg-Quoting). `None` bei offenem Anführungszeichen.
fn split_top(s: &str, sep: char) -> Option<Vec<&str>> {
    let (mut out, mut start, mut quoted) = (Vec::new(), 0, false);
    for (i, c) in s.char_indices() {
        if c == '\'' {
            quoted = !quoted;
        } else if c == sep && !quoted {
            out.push(&s[start..i]);
            start = i + 1;
        }
    }
    out.push(&s[start..]);
    (!quoted).then_some(out)
}

/// Kette aus einem Paket: nur Filter aus [`ALLOWED_FILTERS`], keine eigenen Ein-/Ausgänge,
/// Graph-Trenner, Instanznamen (`@`) oder Escapes, keine Option mit Dateibezug.
pub fn ffmpeg_chain_allowed(f: &str) -> bool {
    if f.contains([';', '[', ']', '@', '\\']) {
        return false;
    }
    let Some(filters) = split_top(f, ',') else {
        return false;
    };
    filters.iter().all(|part| {
        let (name, args) = part.split_once('=').unwrap_or((part, ""));
        let name = name.trim();
        if !ALLOWED_FILTERS.contains(&name) {
            return false;
        }
        let Some(opts) = split_top(args, ':') else {
            return false;
        };
        opts.iter().enumerate().all(|(i, o)| match o.split_once('=') {
            Some((k, _)) => {
                let k = k.trim().to_ascii_lowercase();
                !(k.contains("file") || k == "result" || k == "url")
            }
            // Optionen ohne Namen (`unsharp=5:5:0.6`): bei Filtern mit Datei-Option nur die erste,
            // sonst fiele eine per Position auf sie (`curves`: 7. Option `psfile`)
            None => i == 0 || name != "curves",
        })
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginMeta {
    pub id: String,
    pub version: String,
    pub api: u32,
    pub kind: PluginKind,
    pub name: LocalizedText,
    #[serde(default)]
    pub description: LocalizedText,
    #[serde(default)]
    pub authors: Vec<String>,
    pub license: String,
    /// Herkunft (URL), für die Danksagung
    pub source: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Bei Integrationen `builtin:<name>`.
    pub entry: Option<String>,
    /// Nach der Installation aktiv? (Integrationen: nein)
    #[serde(default = "yes")]
    pub default_enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub plugin: PluginMeta,
    #[serde(default, rename = "asset")]
    pub assets: Vec<AssetEntry>,
}

impl Manifest {
    pub fn parse(src: &str, path: &Path) -> Result<Self, PluginError> {
        toml::from_str(src).map_err(|e| PluginError::Parse {
            path: path.to_path_buf(),
            msg: e.to_string(),
        })
    }

    /// Prüft das Manifest. `dir` = Plugin-Ordner (für Dateiprüfungen), `None` = nur Syntax.
    pub fn validate(&self, dir: Option<&Path>) -> Result<(), PluginError> {
        let p = &self.plugin;
        let bad = |msg: String| {
            Err(PluginError::Invalid {
                id: p.id.clone(),
                msg,
            })
        };
        let id_ok = p.id.split('.').count() >= 2
            && p.id.split('.').all(|s| {
                !s.is_empty()
                    && s.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            });
        if !id_ok {
            return bad("ID muss im Format 'anbieter.name' sein (klein, a-z0-9-)".into());
        }
        if p.version.split('.').count() != 3
            || p.version.split('.').any(|n| n.parse::<u32>().is_err())
        {
            return bad(format!("Version '{}' ist kein SemVer (x.y.z)", p.version));
        }
        if p.api != API_VERSION {
            return bad(format!(
                "API-Version {} nicht unterstützt (erwartet {API_VERSION})",
                p.api
            ));
        }
        if p.name.is_empty() {
            return bad("Name fehlt".into());
        }
        for perm in &p.permissions {
            let base = perm.split(':').next().unwrap_or("");
            if !KNOWN_PERMISSIONS.contains(&base) {
                return bad(format!("Unbekanntes Recht '{perm}'"));
            }
        }
        match p.kind {
            PluginKind::AssetPack => {
                if !p.permissions.is_empty() {
                    return bad(
                        "Asset-Pakete führen keinen Code aus und dürfen keine Rechte anfordern"
                            .into(),
                    );
                }
                if self.assets.is_empty() {
                    return bad("Asset-Paket ohne Assets".into());
                }
            }
            PluginKind::Integration => {
                if !p
                    .entry
                    .as_deref()
                    .is_some_and(|e| e.starts_with("builtin:"))
                {
                    return bad("Integrationen brauchen 'entry = \"builtin:<name>\"'".into());
                }
                if !p.permissions.iter().any(|x| x.starts_with("net")) {
                    return bad("Integrationen müssen das Recht 'net' deklarieren".into());
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for a in &self.assets {
            if !seen.insert(&a.id) {
                return bad(format!("Asset-ID '{}' doppelt", a.id));
            }
            // nur, was der Katalog darstellen kann (cl-app `contents::catalog_packs`)
            let (fx, sh) = (a.ffmpeg.is_some(), a.shader.is_some());
            let unsupported = match a.kind {
                AssetKind::VideoEffect | AssetKind::Filter => (!fx || sh).then_some("braucht eine FFmpeg-Kette (Shader nur bei Übergängen)"),
                AssetKind::Transition => (!sh || fx).then_some("braucht einen Shader (keine FFmpeg-Kette)"),
                AssetKind::Sound | AssetKind::Sticker => (fx || sh).then_some("darf weder Shader noch FFmpeg-Kette haben"),
                _ => Some("hat eine Art, die Cutline noch nicht anzeigen kann"),
            };
            if let Some(why) = unsupported {
                return bad(format!("Asset '{}' {why}", a.id));
            }
            if a.kind == AssetKind::Transition {
                // welcher Name eingebaut ist, prüft der Katalog (unbekannt → Überblenden)
                let ok = a.fallback.as_deref().is_some_and(|f| {
                    !f.is_empty() && f.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                });
                if !ok {
                    return bad(format!(
                        "Übergang '{}' braucht 'fallback' (eingebauter Übergang, z. B. \"fade\")",
                        a.id
                    ));
                }
                if let Err(e) = a.shader_params() {
                    return bad(format!("Übergang '{}': {e}", a.id));
                }
            }
            if let Some(f) = &a.ffmpeg {
                // roh und nach dem Einsetzen der Standardwerte prüfen (Werte könnten Trenner schmuggeln)
                if !ffmpeg_chain_allowed(f) || !a.ffmpeg_filled().is_some_and(|c| ffmpeg_chain_allowed(&c)) {
                    return bad(format!(
                        "FFmpeg-Kette von '{}' enthält nicht erlaubte Elemente",
                        a.id
                    ));
                }
            }
            let mut pids = std::collections::HashSet::new();
            for prm in &a.params {
                if !pids.insert(prm.id()) {
                    return bad(format!("Parameter '{}' doppelt in '{}'", prm.id(), a.id));
                }
                match prm {
                    ParamSpec::Float {
                        min, max, default, ..
                    } if !(min <= default && default <= max) => {
                        return bad(format!(
                            "Standardwert von '{}' außerhalb [{min}, {max}]",
                            prm.id()
                        ))
                    }
                    ParamSpec::Int {
                        min, max, default, ..
                    } if !(min <= default && default <= max) => {
                        return bad(format!(
                            "Standardwert von '{}' außerhalb [{min}, {max}]",
                            prm.id()
                        ))
                    }
                    ParamSpec::Choice {
                        options, default, ..
                    } if !options.contains(default) => {
                        return bad(format!("Standardwert von '{}' ist keine Option", prm.id()))
                    }
                    _ => {}
                }
            }
            let exts = match a.kind {
                AssetKind::Sound => Some(SOUND_EXT),
                AssetKind::Sticker => Some(STICKER_EXT),
                _ => None,
            };
            if let Some(exts) = exts {
                let Some(f) = &a.file else {
                    return bad(format!("Asset '{}' braucht eine Datei ('file')", a.id));
                };
                let ext = Path::new(f)
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if !exts.contains(&ext.as_str()) {
                    return bad(format!("Dateityp von '{f}' nicht erlaubt (erlaubt: {})", exts.join(", ")));
                }
            }
            for f in [&a.shader, &a.file].into_iter().flatten() {
                if f.contains("..") || Path::new(f).is_absolute() || !relative_inside(f) {
                    return bad(format!("Pfad '{f}' muss innerhalb des Plugins liegen"));
                }
            }
            if let Some(dir) = dir {
                for f in [&a.shader, &a.file].into_iter().flatten()
                {
                    if !dir.join(f).is_file() {
                        return bad(format!("Datei '{f}' fehlt"));
                    }
                    // auch über Verknüpfungen nicht aus dem Ordner heraus
                    let inside = match (dir.join(f).canonicalize(), dir.canonicalize()) {
                        (Ok(a), Ok(d)) => a.starts_with(d),
                        _ => false,
                    };
                    if !inside {
                        return bad(format!("Pfad '{f}' muss innerhalb des Plugins liegen"));
                    }
                }
            }
        }
        if let (Some(dir), Some(entry)) =
            (dir, p.entry.as_ref().filter(|e| !e.starts_with("builtin:")))
        {
            if entry.contains("..") || !dir.join(entry).exists() {
                return bad(format!("Einstiegsdatei '{entry}' fehlt"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub enabled: bool,
}

#[derive(Debug, Default)]
pub struct Registry {
    pub plugins: Vec<LoadedPlugin>,
    pub errors: Vec<String>,
}

impl Registry {
    /// Ist die eingebaute Integration `builtin` (z. B. "epidemic-sound") geladen und aktiv?
    pub fn integration_enabled(&self, builtin: &str) -> bool {
        self.plugins.iter().any(|p| {
            p.enabled
                && p.manifest.plugin.kind == PluginKind::Integration
                && p.manifest.plugin.entry.as_deref() == Some(&format!("builtin:{builtin}"))
        })
    }

    /// Gespeicherte Ein/Aus-Zustände anwenden (Plugin-ID → aktiv).
    pub fn apply_states(&mut self, states: &std::collections::BTreeMap<String, bool>) {
        for p in &mut self.plugins {
            if let Some(on) = states.get(&p.manifest.plugin.id) {
                p.enabled = *on;
            }
        }
    }

    /// Lädt alle Unterordner mit `plugin.toml` aus den angegebenen Wurzeln.
    pub fn scan<'a>(roots: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut reg = Registry::default();
        for root in roots {
            let Ok(rd) = std::fs::read_dir(root) else {
                continue;
            };
            let mut dirs: Vec<_> = rd
                .flatten()
                .map(|e| e.path())
                // `.install-…`: Zwischenordner einer abgebrochenen Installation (Absturz)
                .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                .filter(|p| p.join(MANIFEST_FILE).is_file())
                .collect();
            dirs.sort();
            for dir in dirs {
                match reg.load_dir(&dir) {
                    Ok(()) => {}
                    Err(e) => reg.errors.push(e.to_string()),
                }
            }
        }
        reg
    }

    fn load_dir(&mut self, dir: &Path) -> Result<(), PluginError> {
        let path = dir.join(MANIFEST_FILE);
        let m = Manifest::parse(&std::fs::read_to_string(&path)?, &path)?;
        m.validate(Some(dir))?;
        if self
            .plugins
            .iter()
            .any(|p| p.manifest.plugin.id == m.plugin.id)
        {
            return Err(PluginError::Invalid {
                id: m.plugin.id.clone(),
                msg: "bereits geladen".into(),
            });
        }
        let enabled = m.plugin.default_enabled;
        self.plugins.push(LoadedPlugin {
            dir: dir.to_path_buf(),
            manifest: m,
            enabled,
        });
        Ok(())
    }

    pub fn assets(&self, kind: AssetKind) -> impl Iterator<Item = (&LoadedPlugin, &AssetEntry)> {
        self.plugins
            .iter()
            .filter(|p| p.enabled)
            .flat_map(move |p| {
                p.manifest
                    .assets
                    .iter()
                    .filter(move |a| a.kind == kind)
                    .map(move |a| (p, a))
            })
    }

}

/// Ergebnis von [`install_zip`].
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    /// Paket mit dieser ID liegt jetzt unter `root/<id>`
    Done(String),
    /// Paket gleicher ID ist schon da (nichts geändert; mit `replace` erneut aufrufen)
    Exists(String),
}

/// Höchstens so viel wird aus einem Paket entpackt (die mitgelieferten haben unter 10 MB).
const MAX_UNPACKED: u64 = 2 << 30;
/// Höchstens so viele Einträge je Paket.
const MAX_ENTRIES: usize = 10_000;

/// Eintrag kopieren, ohne mehr als `left` Bytes zu schreiben: die Größenangaben im ZIP sind
/// nicht vertrauenswürdig (gepackte „Bomben“ füllen sonst die Platte).
fn copy_capped(r: impl std::io::Read, w: &mut impl std::io::Write, left: &mut u64) -> Result<(), String> {
    let n = std::io::copy(&mut r.take(*left + 1), w).map_err(|e| e.to_string())?;
    if n > *left {
        return Err("Paket ist entpackt zu groß".into());
    }
    *left -= n;
    Ok(())
}

/// Asset-Paket aus einer ZIP-Datei nach `root/<id>` entpacken. Das Manifest liegt im Stamm des
/// Archivs oder in genau einem Unterordner. Unsichere Pfade, ein ungültiges Manifest oder eine
/// andere Art als `asset-pack` brechen ab; dann bleibt `root` unverändert.
pub fn install_zip(zip_path: &Path, root: &Path, replace: bool) -> Result<Installed, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let staging = root.join(format!(".install-{}-{nanos}", std::process::id()));
    let r = install_staged(zip_path, root, &staging, replace);
    let _ = std::fs::remove_dir_all(&staging);
    r
}

fn install_staged(zip_path: &Path, root: &Path, staging: &Path, replace: bool) -> Result<Installed, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("Keine gültige ZIP-Datei: {e}"))?;
    // die mitgelieferten Pakete haben unter 200 Dateien; Millionen leerer Einträge legten sonst
    // ebenso viele Dateien an
    if zip.len() > MAX_ENTRIES {
        return Err(format!("Paket hat zu viele Dateien ({}, höchstens {MAX_ENTRIES})", zip.len()));
    }
    let mut left = MAX_UNPACKED;
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = e.enclosed_name() else {
            return Err(format!("Unsicherer Pfad im Paket: {}", e.name().unwrap_or_default()));
        };
        let dst = staging.join(rel);
        if e.is_dir() {
            std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&dst).map_err(|e| e.to_string())?;
        copy_capped(&mut e, &mut out, &mut left)?;
    }
    let dir = if staging.join(MANIFEST_FILE).is_file() {
        staging.to_path_buf()
    } else {
        let subs: Vec<PathBuf> = std::fs::read_dir(staging)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join(MANIFEST_FILE).is_file())
            .collect();
        match subs.as_slice() {
            [one] => one.clone(),
            _ => return Err(format!("Kein {MANIFEST_FILE} im Paket")),
        }
    };
    let path = dir.join(MANIFEST_FILE);
    let m = Manifest::parse(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?, &path)
        .map_err(|e| e.to_string())?;
    m.validate(Some(&dir)).map_err(|e| e.to_string())?;
    if m.plugin.kind != PluginKind::AssetPack {
        return Err("Nur Inhaltspakete (asset-pack) lassen sich hinzufügen".into());
    }
    let id = m.plugin.id.clone();
    let target = root.join(&id);
    if target.exists() {
        if !replace {
            return Ok(Installed::Exists(id));
        }
        std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&dir, &target).map_err(|e| e.to_string())?;
    Ok(Installed::Done(id))
}

/// Paketordner löschen – nur, wenn er unterhalb von `root` liegt (eigene Pakete).
pub fn remove_pack(root: &Path, dir: &Path) -> Result<(), String> {
    let (r, d) = (
        root.canonicalize().map_err(|e| e.to_string())?,
        dir.canonicalize().map_err(|e| e.to_string())?,
    );
    if d == r || !d.starts_with(&r) {
        return Err("Nur eigene Pakete lassen sich entfernen".into());
    }
    std::fs::remove_dir_all(&d).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
    }

    #[test]
    fn bundled_plugins_are_valid() {
        let reg = Registry::scan([bundled().as_path()]);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        assert!(!reg.plugins.is_empty());
        assert!(reg.assets(AssetKind::Transition).count() >= 1);
        assert!(reg.assets(AssetKind::VideoEffect).count() >= 1);
        assert!(reg.assets(AssetKind::Filter).count() >= 1);
        // jede Kette aller mitgelieferten Pakete besteht die Liste erlaubter Filter
        let chains: Vec<String> = reg.plugins.iter().flat_map(|p| p.manifest.assets.iter()).filter_map(|a| a.ffmpeg_filled()).collect();
        assert!(chains.len() >= 30, "{}", chains.len());
        for c in &chains {
            assert!(ffmpeg_chain_allowed(c), "{c}");
        }
    }

    #[test]
    fn rejects_bad_manifests() {
        let base = r#"
            [plugin]
            id = "test.pack"
            version = "1.0.0"
            api = 1
            kind = "asset-pack"
            name = { de = "Test" }
            license = "MIT"
        "#;
        let p = Path::new("x");
        // Asset-Paket ohne Assets
        assert!(Manifest::parse(base, p).unwrap().validate(None).is_err());
        // Standardwert außerhalb des Bereichs
        let m = format!(
            "{base}\n[[asset]]\nid='a'\nkind='video-effect'\nname={{de='A'}}\nffmpeg='eq'\nparams=[{{type='float',id='x',min=0.0,max=1.0,default=2.0}}]"
        );
        let err = Manifest::parse(&m, p)
            .unwrap()
            .validate(None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("außerhalb"), "{err}");
        // Asset-Pakete dürfen keine Rechte fordern
        let m = base.replace(
            "license = \"MIT\"",
            "license = \"MIT\"\npermissions = [\"net\"]",
        ) + "\n[[asset]]\nid='f'\nkind='filter'\nname={de='F'}\nffmpeg='eq'";
        assert!(Manifest::parse(&m, p).unwrap().validate(None).is_err());
        // Unbekannte Felder werden abgelehnt (Tippfehler fallen auf)
        assert!(Manifest::parse(&format!("{base}\nfoo = 1"), p).is_err());
        // Was der Katalog nicht zeigen kann, besteht die Prüfung nicht (früher: Filter nur mit LUT)
        let asset = |kind: &str, extra: &str| format!("{base}\n[[asset]]\nid='a'\nkind='{kind}'\nname={{de='A'}}\n{extra}");
        assert!(Manifest::parse(&asset("filter", "lut='f.cube'"), p).is_err());
        for (kind, extra) in [("filter", "shader='f.wgsl'"), ("video-effect", "shader='f.wgsl'"), ("body-effect", "ffmpeg='eq'"), ("transition", "ffmpeg='fade'"), ("sound", "file='a.wav'\nffmpeg='eq'")] {
            assert!(Manifest::parse(&asset(kind, extra), p).unwrap().validate(None).is_err(), "{kind}");
        }
        Manifest::parse(&asset("filter", "ffmpeg='eq'\ntags=['warm']"), p).unwrap().validate(None).unwrap();
        // Code-Plugins (WASM, CLAP) gibt es nicht
        assert!(Manifest::parse(&base.replace("asset-pack", "wasm"), p).is_err());
        // FFmpeg-Ketten dürfen keine Dateien lesen
        let m = format!(
            "{base}
[[asset]]
id='e'
kind='video-effect'
name={{de='E'}}
ffmpeg='movie=x.mp4'"
        );
        assert!(Manifest::parse(&m, p).unwrap().validate(None).is_err());
        // Standardwert eines Parameters schmuggelt einen zweiten Graphen ein
        let m = format!(
            "{base}
[[asset]]
id='e'
kind='video-effect'
name={{de='E'}}
ffmpeg='eq=gamma={{g}}'
params=[{{type='choice',id='g',options=['1[a];movie=x.mp4'],default='1[a];movie=x.mp4'}}]"
        );
        let err = Manifest::parse(&m, p).unwrap().validate(None).unwrap_err().to_string();
        assert!(err.contains("nicht erlaubte"), "{err}");
        for bad in ["movie@m=x.mp4", r"eq=gamma=1,lut3d=C\:/x.cube", "sendcmd=c=x", "frei0r=x", "ladspa=x", " lv2=p=x", "zmq"] {
            assert!(!ffmpeg_chain_allowed(bad), "{bad}");
        }
        assert!(ffmpeg_chain_allowed("gblur=sigma=2,eq=brightness=0.04*{intensity}"));
        // Filter, die Dateien schreiben oder lesen, kamen an der alten Sperrliste vorbei
        for bad in [
            r"signature=filename=C\:/Users/x/a.docx",
            "signature=filename='C:/Users/x/a.docx'",
            "eq,vidstabdetect=result=x.trf",
            "curves=psfile=x.acv",
            "curves=none:0/0 1/1:0/0 1/1:0/0 1/1:0/0 1/1:0/0 1/1:x.acv",
            "drawtext=textfile=x.txt",
            "eq=gamma='1",
            "eq=gamma=1\\,movie=x.mp4",
            "unbekannt=1",
        ] {
            assert!(!ffmpeg_chain_allowed(bad), "{bad}");
        }
        assert!(ffmpeg_chain_allowed("format=rgba,split,overlay=x='W*sin(t)':y='max(1,H/2)'"));
        // Shader-Übergänge brauchen einen Rückfall und Zahlen-Parameter
        let tr = |extra: &str| format!("{base}
[[asset]]
id='t'
kind='transition'
name={{de='T'}}
shader='t.wgsl'
{extra}");
        assert!(Manifest::parse(&tr(""), p).unwrap().validate(None).unwrap_err().to_string().contains("fallback"));
        assert!(Manifest::parse(&tr("fallback='fade;x'"), p).unwrap().validate(None).is_err());
        Manifest::parse(&tr("fallback='wipeleft'"), p).unwrap().validate(None).unwrap();
        let m = Manifest::parse(&tr("fallback='fade'
params=[{type='float',id='a',min=0.0,max=2.0,default=1.5},{type='bool',id='b',default=true}]"), p).unwrap();
        m.validate(None).unwrap();
        assert_eq!(m.assets[0].shader_params().unwrap(), vec![1.5, 1.0]);
        let m = tr("fallback='fade'
params=[{type='color',id='c',default='#fff'}]");
        assert!(Manifest::parse(&m, p).unwrap().validate(None).is_err());
    }

    #[test]
    fn shipped_plugins_load_and_integrations_start_disabled() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let reg = Registry::scan([root.as_path()]);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let es = reg
            .plugins
            .iter()
            .find(|p| p.manifest.plugin.id == "cutline.epidemic-sound")
            .expect("Epidemic-Plugin");
        assert!(!es.enabled);
        assert!(!reg.integration_enabled("epidemic-sound"));
        let mut reg = reg;
        reg.apply_states(
            &[("cutline.epidemic-sound".to_string(), true)]
                .into_iter()
                .collect(),
        );
        assert!(reg.integration_enabled("epidemic-sound"));
    }

    const PACK: &str = r#"
[plugin]
id = "test.sounds"
version = "1.0.0"
api = 1
kind = "asset-pack"
name = { de = "Testpaket" }
license = "CC0-1.0"

[[asset]]
id = "ding"
kind = "sound"
name = { de = "Ding" }
file = "ding.wav"
license = "CC-BY-4.0"
"#;

    #[test]
    fn sound_and_sticker_files_stay_inside_and_have_allowed_types() {
        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("p");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("ding.wav"), b"RIFF").unwrap();
        std::fs::write(dir.path().join("x.wav"), b"RIFF").unwrap();
        let m = Manifest::parse(PACK, Path::new("x")).unwrap();
        m.validate(Some(&pack)).unwrap();
        assert_eq!(m.assets[0].license.as_deref(), Some("CC-BY-4.0"));
        for bad in ["../x.wav", "/x.wav", "C:/x.wav", r"C:x.wav", "sub/../../x.wav"] {
            let t = PACK.replace("file = \"ding.wav\"", &format!("file = '{bad}'"));
            let err = Manifest::parse(&t, Path::new("x")).unwrap().validate(Some(&pack)).unwrap_err().to_string();
            assert!(err.contains("innerhalb"), "{bad}: {err}");
        }
        for (f, ok) in [("ding.exe", false), ("ding.gif", false), ("ding", false), ("DING.WAV", true)] {
            let t = PACK.replace("ding.wav", f);
            let r = Manifest::parse(&t, Path::new("x")).unwrap().validate(None);
            assert_eq!(r.is_ok(), ok, "{f}: {r:?}");
        }
        // Sticker brauchen eine Datei
        let t = PACK.replace("kind = \"sound\"", "kind = \"sticker\"").replace("file = \"ding.wav\"", "");
        assert!(Manifest::parse(&t, Path::new("x")).unwrap().validate(None).is_err());
        for (f, ok) in [("a.svg", true), ("a.gif", true), ("a.png", true), ("a.wav", false)] {
            let t = PACK.replace("kind = \"sound\"", "kind = \"sticker\"").replace("ding.wav", f);
            let r = Manifest::parse(&t, Path::new("x")).unwrap().validate(None);
            assert_eq!(r.is_ok(), ok, "{f}: {r:?}");
        }
    }

    fn zip_with(path: &Path, files: &[(&str, &[u8])]) {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (n, d) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(d).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn zip_entries_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        let zp = dir.path().join("b.zip");
        zip_with(&zp, &[("null.bin", &vec![0u8; 1 << 20])]);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&zp).unwrap()).unwrap();
        let mut out = Vec::new();
        let mut left = 1000;
        assert!(copy_capped(&mut zip.by_index(0).unwrap(), &mut out, &mut left).is_err());
        assert!(out.len() <= 1001);
    }

    #[test]
    fn install_zip_is_safe_and_asks_before_replacing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Plugins");
        let zp = dir.path().join("p.zip");
        // Manifest in einem Unterordner
        zip_with(&zp, &[("pack/plugin.toml", PACK.as_bytes()), ("pack/ding.wav", b"RIFF")]);
        assert_eq!(install_zip(&zp, &root, false).unwrap(), Installed::Done("test.sounds".into()));
        assert!(root.join("test.sounds/ding.wav").is_file());
        // gleiche ID: erst fragen, dann ersetzen
        zip_with(&zp, &[("plugin.toml", PACK.as_bytes()), ("ding.wav", b"RIFF2")]);
        assert_eq!(install_zip(&zp, &root, false).unwrap(), Installed::Exists("test.sounds".into()));
        assert_eq!(std::fs::read(root.join("test.sounds/ding.wav")).unwrap(), b"RIFF");
        assert_eq!(install_zip(&zp, &root, true).unwrap(), Installed::Done("test.sounds".into()));
        assert_eq!(std::fs::read(root.join("test.sounds/ding.wav")).unwrap(), b"RIFF2");
        // bösartiger Pfad: nichts wird übernommen
        let evil = dir.path().join("evil.zip");
        zip_with(&evil, &[("plugin.toml", PACK.replace("test.sounds", "test.evil").as_bytes()), ("ding.wav", b"x"), ("../boese.txt", b"x")]);
        assert!(install_zip(&evil, &root, true).is_err());
        assert!(!dir.path().join("boese.txt").exists());
        assert!(!root.join("test.evil").exists());
        // ungültiges Manifest (Datei fehlt) und Integrationen werden abgelehnt
        zip_with(&evil, &[("plugin.toml", PACK.replace("test.sounds", "test.evil").as_bytes())]);
        assert!(install_zip(&evil, &root, true).is_err());
        let integ = "[plugin]
id='test.int'
version='1.0.0'
api=1
kind='integration'
entry='builtin:x'
name={de='I'}
license='MIT'
permissions=['net']
";
        zip_with(&evil, &[("plugin.toml", integ.as_bytes())]);
        assert!(install_zip(&evil, &root, true).is_err());
        // nur Ordner mit test.sounds übrig, kein Zwischenordner
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("test.sounds")]);
        // Entfernen nur unterhalb von root
        assert!(remove_pack(&root, dir.path()).is_err());
        assert!(remove_pack(&root, &root).is_err());
        remove_pack(&root, &root.join("test.sounds")).unwrap();
        assert!(!root.join("test.sounds").exists());
    }

    /// Zwei Ordner mit derselben Paket-ID: der erste gewinnt, der zweite wird als Fehler gemeldet.
    /// Reste einer abgebrochenen Installation (`.install-…`) werden nicht geladen.
    #[test]
    fn duplicate_ids_and_install_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["a", "b", ".install-1-2"] {
            let p = dir.path().join(d);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join(MANIFEST_FILE), PACK).unwrap();
            std::fs::write(p.join("ding.wav"), b"RIFF").unwrap();
        }
        let reg = Registry::scan([dir.path()]);
        assert_eq!(reg.plugins.len(), 1);
        assert!(reg.plugins[0].dir.ends_with("a"));
        assert_eq!(reg.errors.len(), 1, "{:?}", reg.errors);
        assert!(reg.errors[0].contains("bereits geladen"), "{:?}", reg.errors);
        // dieselbe Wurzel zweimal (z. B. Datenordner = Programmordner): kein Absturz, nur Meldungen
        let reg = Registry::scan([dir.path(), dir.path()]);
        assert_eq!(reg.plugins.len(), 1);
    }

    /// Kaputte Manifeste aus fremder Hand: nie ein Absturz, immer ein Fehler.
    #[test]
    fn garbage_manifests_are_errors() {
        let p = Path::new("x");
        let long = "a".repeat(1 << 20);
        for src in [
            "", "\u{feff}", "[plugin]", "[[asset]]\nid='x'", "\0\0\0\u{1}", &long,
            "[plugin]\nid='a.b'\nversion='1.0.0'\napi=1\nkind='asset-pack'\nname={de='N'}", // ohne Lizenz
        ] {
            assert!(Manifest::parse(src, p).is_err(), "{:?}", &src[..src.len().min(40)]);
        }
        let with = |k: &str, v: &str| PACK.replace(&format!("{k} = "), &format!("{k} = {v} #"));
        for (k, v) in [
            ("id", "'Test.Sounds'"), ("id", "'nopunkt'"), ("id", "'a..b'"), ("id", "'.a.b'"), ("id", "'a.b/../c'"),
            ("id", "'ä.b'"), ("id", "''"), ("version", "'1.0'"), ("version", "'1.0.0-beta'"), ("version", "'-1.0.0'"),
            ("api", "2"), ("api", "0"),
        ] {
            let m = Manifest::parse(&with(k, v), p);
            assert!(m.is_err() || m.unwrap().validate(None).is_err(), "{k} = {v}");
        }
        assert!(Manifest::parse(&with("api", "99999999999999999999"), p).is_err());
        // Pfad mit Unicode/Leerzeichen innerhalb ist erlaubt
        let t = PACK.replace("ding.wav", "Glöckchen 🔔.wav");
        Manifest::parse(&t, p).unwrap().validate(None).unwrap();
    }

    #[test]
    fn zip_with_too_many_entries_or_links_is_handled() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Plugins");
        let zp = dir.path().join("viele.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
        w.start_file("plugin.toml", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(PACK.as_bytes()).unwrap();
        for i in 0..MAX_ENTRIES {
            w.start_file(format!("x/{i}"), zip::write::SimpleFileOptions::default()).unwrap();
        }
        w.finish().unwrap();
        let e = install_zip(&zp, &root, true).unwrap_err();
        assert!(e.contains("zu viele"), "{e}");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "nichts entpackt");
        // Verknüpfung im Archiv wird eine gewöhnliche Datei (zeigt nicht nach außen)
        let secret = dir.path().join("geheim.txt");
        std::fs::write(&secret, "geheim").unwrap();
        let lz = dir.path().join("link.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&lz).unwrap());
        w.start_file("plugin.toml", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(PACK.as_bytes()).unwrap();
        w.add_symlink("ding.wav", secret.to_string_lossy(), zip::write::SimpleFileOptions::default()).unwrap();
        w.finish().unwrap();
        if install_zip(&lz, &root, true).is_ok() {
            let f = root.join("test.sounds/ding.wav");
            assert!(!std::fs::symlink_metadata(&f).unwrap().file_type().is_symlink());
            assert_ne!(std::fs::read_to_string(&f).unwrap(), "geheim");
        }
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "geheim");
        // kein ZIP / leere Datei / abgeschnittenes ZIP: Fehler, root bleibt sauber
        let bad = dir.path().join("kaputt.zip");
        for data in [&b""[..], b"PK\x03\x04kaputt", &std::fs::read(&zp).unwrap()[..1000]] {
            std::fs::write(&bad, data).unwrap();
            assert!(install_zip(&bad, &root, true).is_err());
        }
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(left.iter().all(|n| !n.to_string_lossy().starts_with(".install")), "{left:?}");
    }

    /// Jeder mitgelieferte Sound hat ein Rezept (selbst erzeugt) und eine CC0-Lizenz.
    #[test]
    fn bundled_sounds_have_recipes() {
        let dir = bundled().join("cutline-sounds");
        let recipes = std::fs::read_to_string(dir.join("RECIPES.md")).unwrap();
        let reg = Registry::scan([bundled().as_path()]);
        let p = reg.plugins.iter().find(|p| p.manifest.plugin.id == "cutline.sounds").unwrap();
        assert_eq!(p.manifest.plugin.license, "CC0-1.0");
        let mut n = 0;
        for a in p.manifest.assets.iter().filter(|a| a.kind == AssetKind::Sound) {
            assert!(recipes.contains(&format!("## {} (", a.id)), "kein Rezept für {}", a.id);
            n += 1;
        }
        assert!(n >= 50, "{n}");
    }
}
