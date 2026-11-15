//! Plugin-System, Teil 1: Manifest-Format, Validierung und Registry.
//!
//! Drei Ebenen:
//! 1. `asset-pack`: nur Daten (Effekte/Übergänge/Filter als WGSL + Manifest), kein Code
//! 2. `wasm`: WebAssembly-Komponente mit deklarierten Rechten (Host folgt in Phase 1)
//! 3. `clap`: native Audio-Plugins, außerhalb des App-Prozesses (Phase 3)

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
    Wasm,
    Clap,
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
        #[serde(default)]
        keyframable: bool,
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
    /// WGSL-Shader relativ zum Plugin-Ordner (bei Effekten/Übergängen/Filtern Pflicht).
    pub shader: Option<String>,
    /// Optional: LUT (.cube) für Filter.
    pub lut: Option<String>,
    /// FFmpeg-Filterkette (Effekt/Filter) bzw. `xfade`-Name oder `custom:<Ausdruck>` (Übergang).
    /// Platzhalter: `{intensity}` (0…1) und `{<param-id>}`.
    pub ffmpeg: Option<String>,
    /// Datei relativ zum Plugin-Ordner (Sound/Sticker: Pflicht, Endung siehe [`SOUND_EXT`]/[`STICKER_EXT`]).
    pub file: Option<String>,
    pub preview: Option<String>,
    /// Abweichende Angaben für dieses Asset (Standard: die des Pakets)
    pub license: Option<String>,
    pub author: Option<String>,
    pub source: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Benötigte Analysen, z. B. "segmentation" für Körpereffekte.
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub params: Vec<ParamSpec>,
}

impl AssetEntry {
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

/// Erlaubte Endungen für Paket-Sounds (wie die eigene Bibliothek).
pub const SOUND_EXT: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac", "opus"];
/// Erlaubte Endungen für Paket-Sticker. GIF/APNG fehlen bewusst: Sticker werden als Bild-Clip
/// eingefügt, und der Export liest Bild-Clips mit `-loop 1`, das der GIF-/APNG-Leser ablehnt.
pub const STICKER_EXT: &[&str] = &["png", "webp", "svg", "jpg", "jpeg"];

/// Relativer Pfad ohne `..`, Wurzel oder Laufwerk (bleibt im Plugin-Ordner).
pub fn relative_inside(f: &str) -> bool {
    !f.is_empty()
        && !f.contains("..")
        && Path::new(f)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Kette darf keine eigenen Ein-/Ausgänge, Dateien, Graph-Trenner, Instanznamen (`@`)
/// oder Filter mit Datei-/Fremdcode-Zugriff enthalten.
pub fn ffmpeg_chain_allowed(f: &str) -> bool {
    const BLOCKED: [&str; 8] = [
        "movie", "amovie", "frei0r", "ladspa", "lv2", "sendcmd", "zmq", "lut3d",
    ];
    if [";", "[", "@", "movie=", "file="].iter().any(|x| f.contains(x)) {
        return false;
    }
    // Filtername = Anfang jedes Kommas-Abschnitts bis `=` (zu streng bei Kommas in Werten, das ist ok)
    !f.split(',').any(|part| {
        let name = part.split('=').next().unwrap_or("").trim().to_ascii_lowercase();
        BLOCKED.contains(&name.as_str()) || name == "azmq" || name == "asendcmd"
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
    /// WASM-Komponente bzw. CLAP-Bundle relativ zum Plugin-Ordner, bei Integrationen `builtin:<name>`.
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
            PluginKind::Wasm | PluginKind::Clap => {
                if p.entry.is_none() {
                    return bad("'entry' fehlt".into());
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for a in &self.assets {
            if !seen.insert(&a.id) {
                return bad(format!("Asset-ID '{}' doppelt", a.id));
            }
            let needs_shader = matches!(
                a.kind,
                AssetKind::VideoEffect | AssetKind::BodyEffect | AssetKind::Transition
            );
            if needs_shader && a.shader.is_none() && a.ffmpeg.is_none() {
                return bad(format!(
                    "Asset '{}' braucht einen Shader oder eine FFmpeg-Kette",
                    a.id
                ));
            }
            if a.kind == AssetKind::Filter
                && a.shader.is_none()
                && a.lut.is_none()
                && a.ffmpeg.is_none()
            {
                return bad(format!(
                    "Filter '{}' braucht Shader, LUT oder FFmpeg-Kette",
                    a.id
                ));
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
            for f in [&a.shader, &a.lut, &a.file, &a.preview].into_iter().flatten() {
                if f.contains("..") || Path::new(f).is_absolute() || !relative_inside(f) {
                    return bad(format!("Pfad '{f}' muss innerhalb des Plugins liegen"));
                }
            }
            if let Some(dir) = dir {
                for f in [&a.shader, &a.lut, &a.file, &a.preview]
                    .into_iter()
                    .flatten()
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

    /// Alle sofort lauffähigen FFmpeg-Assets: (volle ID `plugin/asset`, Art, Name, Kette).
    pub fn ffmpeg_assets(&self, lang: &str) -> Vec<(String, AssetKind, String, String)> {
        self.plugins
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| {
                p.manifest.assets.iter().filter_map(move |a| {
                    let c = a.ffmpeg_filled().filter(|c| ffmpeg_chain_allowed(c))?;
                    Some((
                        format!("{}/{}", p.manifest.plugin.id, a.id),
                        a.kind,
                        localized(&a.name, lang).to_string(),
                        c,
                    ))
                })
            })
            .collect()
    }

    pub fn asset_count(&self) -> usize {
        self.plugins
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.manifest.assets.len())
            .sum()
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
        std::io::copy(&mut e, &mut out).map_err(|e| e.to_string())?;
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
        let ff = reg.ffmpeg_assets("de");
        assert!(
            ff.iter()
                .any(|a| a.1 == AssetKind::VideoEffect && a.3.contains("{intensity}")),
            "{ff:?}"
        );
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
            "{base}\n[[asset]]\nid='a'\nkind='video-effect'\nname={{de='A'}}\nshader='a.wgsl'\nparams=[{{type='float',id='x',min=0.0,max=1.0,default=2.0}}]"
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
        ) + "\n[[asset]]\nid='f'\nkind='filter'\nname={de='F'}\nlut='f.cube'";
        assert!(Manifest::parse(&m, p).unwrap().validate(None).is_err());
        // Unbekannte Felder werden abgelehnt (Tippfehler fallen auf)
        assert!(Manifest::parse(&format!("{base}\nfoo = 1"), p).is_err());
        // WASM ohne entry
        let m = base.replace("asset-pack", "wasm");
        assert!(Manifest::parse(&m, p)
            .unwrap()
            .validate(None)
            .unwrap_err()
            .to_string()
            .contains("entry"));
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
        let t = PACK.replace("kind = \"sound\"", "kind = \"sticker\"").replace("ding.wav", "a.svg");
        Manifest::parse(&t, Path::new("x")).unwrap().validate(None).unwrap();
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
}
