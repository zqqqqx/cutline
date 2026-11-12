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
pub const KNOWN_PERMISSIONS: &[&str] =
    &["project.read", "project.write", "media.read", "fs.read", "fs.write", "net", "ui.panel", "ui.command", "render"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginKind {
    AssetPack,
    Wasm,
    Clap,
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
    Float { id: String, min: f32, max: f32, default: f32, #[serde(default)] keyframable: bool },
    Int { id: String, min: i32, max: i32, default: i32 },
    Bool { id: String, default: bool },
    Color { id: String, default: String },
    Choice { id: String, options: Vec<String>, default: String },
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
    t.get(lang).or_else(|| t.get("en")).or_else(|| t.values().next()).map(String::as_str).unwrap_or("?")
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
    pub file: Option<String>,
    pub preview: Option<String>,
    /// Benötigte Analysen, z. B. "segmentation" für Körpereffekte.
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub params: Vec<ParamSpec>,
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
    #[serde(default)]
    pub permissions: Vec<String>,
    /// WASM-Komponente bzw. CLAP-Bundle relativ zum Plugin-Ordner.
    pub entry: Option<String>,
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
        toml::from_str(src).map_err(|e| PluginError::Parse { path: path.to_path_buf(), msg: e.to_string() })
    }

    /// Prüft das Manifest. `dir` = Plugin-Ordner (für Dateiprüfungen), `None` = nur Syntax.
    pub fn validate(&self, dir: Option<&Path>) -> Result<(), PluginError> {
        let p = &self.plugin;
        let bad = |msg: String| Err(PluginError::Invalid { id: p.id.clone(), msg });
        let id_ok = p.id.split('.').count() >= 2
            && p.id.split('.').all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        if !id_ok {
            return bad("ID muss im Format 'anbieter.name' sein (klein, a-z0-9-)".into());
        }
        if p.version.split('.').count() != 3 || p.version.split('.').any(|n| n.parse::<u32>().is_err()) {
            return bad(format!("Version '{}' ist kein SemVer (x.y.z)", p.version));
        }
        if p.api != API_VERSION {
            return bad(format!("API-Version {} nicht unterstützt (erwartet {API_VERSION})", p.api));
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
                    return bad("Asset-Pakete führen keinen Code aus und dürfen keine Rechte anfordern".into());
                }
                if self.assets.is_empty() {
                    return bad("Asset-Paket ohne Assets".into());
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
            let needs_shader = matches!(a.kind, AssetKind::VideoEffect | AssetKind::BodyEffect | AssetKind::Transition);
            if needs_shader && a.shader.is_none() && a.ffmpeg.is_none() {
                return bad(format!("Asset '{}' braucht einen Shader oder eine FFmpeg-Kette", a.id));
            }
            if a.kind == AssetKind::Filter && a.shader.is_none() && a.lut.is_none() && a.ffmpeg.is_none() {
                return bad(format!("Filter '{}' braucht Shader, LUT oder FFmpeg-Kette", a.id));
            }
            if let Some(f) = &a.ffmpeg {
                // Kette darf keine eigenen Ein-/Ausgänge, Dateien oder Graph-Trenner enthalten
                if f.contains(';') || f.contains('[') || f.contains("movie=") || f.contains("amovie=") || f.contains("file=") {
                    return bad(format!("FFmpeg-Kette von '{}' enthält nicht erlaubte Elemente", a.id));
                }
            }
            let mut pids = std::collections::HashSet::new();
            for prm in &a.params {
                if !pids.insert(prm.id()) {
                    return bad(format!("Parameter '{}' doppelt in '{}'", prm.id(), a.id));
                }
                match prm {
                    ParamSpec::Float { min, max, default, .. } if !(min <= default && default <= max) => {
                        return bad(format!("Standardwert von '{}' außerhalb [{min}, {max}]", prm.id()))
                    }
                    ParamSpec::Int { min, max, default, .. } if !(min <= default && default <= max) => {
                        return bad(format!("Standardwert von '{}' außerhalb [{min}, {max}]", prm.id()))
                    }
                    ParamSpec::Choice { options, default, .. } if !options.contains(default) => {
                        return bad(format!("Standardwert von '{}' ist keine Option", prm.id()))
                    }
                    _ => {}
                }
            }
            if let Some(dir) = dir {
                for f in [&a.shader, &a.lut, &a.file, &a.preview].into_iter().flatten() {
                    if f.contains("..") || Path::new(f).is_absolute() {
                        return bad(format!("Pfad '{f}' muss innerhalb des Plugins liegen"));
                    }
                    if !dir.join(f).is_file() {
                        return bad(format!("Datei '{f}' fehlt"));
                    }
                }
            }
        }
        if let (Some(dir), Some(entry)) = (dir, &p.entry) {
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
    /// Lädt alle Unterordner mit `plugin.toml` aus den angegebenen Wurzeln.
    pub fn scan<'a>(roots: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut reg = Registry::default();
        for root in roots {
            let Ok(rd) = std::fs::read_dir(root) else { continue };
            let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.join(MANIFEST_FILE).is_file()).collect();
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
        if self.plugins.iter().any(|p| p.manifest.plugin.id == m.plugin.id) {
            return Err(PluginError::Invalid { id: m.plugin.id.clone(), msg: "bereits geladen".into() });
        }
        self.plugins.push(LoadedPlugin { dir: dir.to_path_buf(), manifest: m, enabled: true });
        Ok(())
    }

    pub fn assets(&self, kind: AssetKind) -> impl Iterator<Item = (&LoadedPlugin, &AssetEntry)> {
        self.plugins
            .iter()
            .filter(|p| p.enabled)
            .flat_map(move |p| p.manifest.assets.iter().filter(move |a| a.kind == kind).map(move |a| (p, a)))
    }

    /// Alle sofort lauffähigen FFmpeg-Assets: (volle ID `plugin/asset`, Art, Name, Kette).
    pub fn ffmpeg_assets(&self, lang: &str) -> Vec<(String, AssetKind, String, String)> {
        self.plugins
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| {
                p.manifest.assets.iter().filter_map(move |a| {
                    let chain = a.ffmpeg.as_ref()?;
                    // Standardwerte der Parameter einsetzen; {intensity} bleibt für den Host
                    let mut c = chain.clone();
                    for prm in &a.params {
                        let v = match prm {
                            ParamSpec::Float { default, .. } => default.to_string(),
                            ParamSpec::Int { default, .. } => default.to_string(),
                            ParamSpec::Bool { default, .. } => (*default as u8).to_string(),
                            ParamSpec::Color { default, .. } | ParamSpec::Choice { default, .. } => default.clone(),
                        };
                        c = c.replace(&format!("{{{}}}", prm.id()), &v);
                    }
                    Some((format!("{}/{}", p.manifest.plugin.id, a.id), a.kind, localized(&a.name, lang).to_string(), c))
                })
            })
            .collect()
    }

    pub fn asset_count(&self) -> usize {
        self.plugins.iter().filter(|p| p.enabled).map(|p| p.manifest.assets.len()).sum()
    }
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
        assert!(ff.iter().any(|a| a.1 == AssetKind::VideoEffect && a.3.contains("{intensity}")), "{ff:?}");
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
        let err = Manifest::parse(&m, p).unwrap().validate(None).unwrap_err().to_string();
        assert!(err.contains("außerhalb"), "{err}");
        // Asset-Pakete dürfen keine Rechte fordern
        let m = base.replace("license = \"MIT\"", "license = \"MIT\"\npermissions = [\"net\"]")
            + "\n[[asset]]\nid='f'\nkind='filter'\nname={de='F'}\nlut='f.cube'";
        assert!(Manifest::parse(&m, p).unwrap().validate(None).is_err());
        // Unbekannte Felder werden abgelehnt (Tippfehler fallen auf)
        assert!(Manifest::parse(&format!("{base}\nfoo = 1"), p).is_err());
        // WASM ohne entry
        let m = base.replace("asset-pack", "wasm");
        assert!(Manifest::parse(&m, p).unwrap().validate(None).unwrap_err().to_string().contains("entry"));
        // FFmpeg-Ketten dürfen keine Dateien lesen
        let m = format!("{base}
[[asset]]
id='e'
kind='video-effect'
name={{de='E'}}
ffmpeg='movie=x.mp4'");
        assert!(Manifest::parse(&m, p).unwrap().validate(None).is_err());
    }
}
