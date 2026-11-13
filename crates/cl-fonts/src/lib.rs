//! Schriftarten-Import und -Bibliothek.
//!
//! Cutline importiert Schriftdateien direkt, statt nur systemweit installierte Schriften
//! anzubieten: per Dialog, Drag & Drop oder überwachtem Ordner.
//! Das geht global oder nur für ein Projekt.
//!
//! Unterstützt: TTF, OTF, TTC/OTC (Collections), WOFF (wird beim Import nach TTF/OTF
//! entpackt), Variable Fonts (Achsen werden ausgelesen) und Farb-Fonts (COLR/sbix/CBDT/SVG).
//! WOFF2 wird erkannt und mit klarer Meldung abgelehnt. Die Unterstützung folgt in Phase 1.

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("Keine Schriftdatei (unbekanntes Format)")]
    UnknownFormat,
    #[error("WOFF2 wird noch nicht unterstützt. Bitte als TTF/OTF importieren.")]
    Woff2Unsupported,
    #[error("Schriftdatei ist beschädigt: {0}")]
    Corrupt(String),
    #[error("E/A-Fehler: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, FontError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontFormat {
    TrueType,
    OpenTypeCff,
    Collection,
    Woff,
    Woff2,
}

impl FontFormat {
    pub fn detect(data: &[u8]) -> Option<Self> {
        match data.get(0..4)? {
            [0, 1, 0, 0] | b"true" => Some(Self::TrueType),
            b"OTTO" => Some(Self::OpenTypeCff),
            b"ttcf" => Some(Self::Collection),
            b"wOFF" => Some(Self::Woff),
            b"wOF2" => Some(Self::Woff2),
            _ => None,
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::TrueType => "ttf",
            Self::OpenTypeCff => "otf",
            Self::Collection => "ttc",
            Self::Woff => "woff",
            Self::Woff2 => "woff2",
        }
    }
}

/// Dateiendungen für den Import-Dialog.
pub const FONT_EXTENSIONS: &[&str] = &["ttf", "otf", "ttc", "otc", "woff", "woff2"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariationAxis {
    pub tag: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Embedding {
    Installable,
    Editable,
    PreviewAndPrint,
    /// Lizenz verbietet Einbettung (Hinweis in der UI, Export nur als gerenderte Pixel).
    Restricted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceInfo {
    pub index: u32,
    pub family: String,
    pub style: String,
    pub full_name: String,
    pub weight: u16,
    pub italic: bool,
    pub monospace: bool,
    pub color: bool,
    pub axes: Vec<VariationAxis>,
    pub embedding: Embedding,
    pub glyph_count: u16,
}

impl FaceInfo {
    pub fn is_variable(&self) -> bool {
        !self.axes.is_empty()
    }
}

/// Liest alle Schriftschnitte einer Datei (auch Collections und WOFF).
pub fn inspect(data: &[u8]) -> Result<Vec<FaceInfo>> {
    let owned;
    let sfnt: &[u8] = match FontFormat::detect(data).ok_or(FontError::UnknownFormat)? {
        FontFormat::Woff2 => return Err(FontError::Woff2Unsupported),
        FontFormat::Woff => {
            owned = woff_to_sfnt(data)?;
            &owned
        }
        _ => data,
    };
    let count = ttf_parser::fonts_in_collection(sfnt).unwrap_or(1);
    let mut faces = Vec::new();
    for index in 0..count {
        let face =
            ttf_parser::Face::parse(sfnt, index).map_err(|e| FontError::Corrupt(e.to_string()))?;
        faces.push(face_info(&face, index));
    }
    if faces.is_empty() {
        return Err(FontError::Corrupt("keine Schnitte".into()));
    }
    Ok(faces)
}

fn best_name(face: &ttf_parser::Face, ids: &[u16]) -> Option<String> {
    for &id in ids {
        let mut fallback = None;
        for n in face.names().into_iter().filter(|n| n.name_id == id) {
            if let Some(s) = n.to_string() {
                if n.language_id == 0x0409 {
                    return Some(s);
                }
                fallback.get_or_insert(s);
            }
        }
        if fallback.is_some() {
            return fallback;
        }
    }
    None
}

fn face_info(face: &ttf_parser::Face, index: u32) -> FaceInfo {
    use ttf_parser::name_id;
    let family = best_name(face, &[name_id::TYPOGRAPHIC_FAMILY, name_id::FAMILY])
        .unwrap_or_else(|| "Unbenannt".into());
    let style = best_name(face, &[name_id::TYPOGRAPHIC_SUBFAMILY, name_id::SUBFAMILY])
        .unwrap_or_else(|| "Regular".into());
    let full_name =
        best_name(face, &[name_id::FULL_NAME]).unwrap_or_else(|| format!("{family} {style}"));
    let t = face.tables();
    let axes = face
        .variation_axes()
        .into_iter()
        .map(|a| VariationAxis {
            tag: String::from_utf8_lossy(&a.tag.to_bytes()).into_owned(),
            min: a.min_value,
            default: a.def_value,
            max: a.max_value,
        })
        .collect();
    let embedding = match face.permissions() {
        Some(ttf_parser::Permissions::Restricted) => Embedding::Restricted,
        Some(ttf_parser::Permissions::PreviewAndPrint) => Embedding::PreviewAndPrint,
        Some(ttf_parser::Permissions::Editable) => Embedding::Editable,
        _ => Embedding::Installable,
    };
    FaceInfo {
        index,
        family,
        style,
        full_name,
        weight: face.weight().to_number(),
        italic: face.is_italic(),
        monospace: face.is_monospaced(),
        color: t.colr.is_some() || t.sbix.is_some() || t.cbdt.is_some() || t.svg.is_some(),
        axes,
        embedding,
        glyph_count: face.number_of_glyphs(),
    }
}

fn be_u16(d: &[u8], o: usize) -> Result<u16> {
    d.get(o..o + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or_else(|| FontError::Corrupt("WOFF zu kurz".into()))
}
fn be_u32(d: &[u8], o: usize) -> Result<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| FontError::Corrupt("WOFF zu kurz".into()))
}

/// Entpackt WOFF 1.0 in eine normale SFNT-Datei (TTF/OTF).
pub fn woff_to_sfnt(d: &[u8]) -> Result<Vec<u8>> {
    if d.get(0..4) != Some(b"wOFF") {
        return Err(FontError::UnknownFormat);
    }
    let flavor = be_u32(d, 4)?;
    let num = be_u16(d, 12)? as usize;
    let mut tables = Vec::with_capacity(num);
    for i in 0..num {
        let e = 44 + i * 20;
        let tag = be_u32(d, e)?;
        let off = be_u32(d, e + 4)? as usize;
        let comp = be_u32(d, e + 8)? as usize;
        let orig = be_u32(d, e + 12)? as usize;
        let csum = be_u32(d, e + 16)?;
        let src = d
            .get(off..off + comp)
            .ok_or_else(|| FontError::Corrupt("WOFF-Tabelle außerhalb".into()))?;
        let data = if comp < orig {
            let mut out = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(src)
                .read_to_end(&mut out)
                .map_err(|e| FontError::Corrupt(format!("zlib: {e}")))?;
            if out.len() != orig {
                return Err(FontError::Corrupt("WOFF-Länge stimmt nicht".into()));
            }
            out
        } else {
            src.to_vec()
        };
        tables.push((tag, csum, data));
    }
    tables.sort_by_key(|t| t.0);
    let mut pow = 1usize;
    let mut sel = 0u16;
    while pow * 2 <= num {
        pow *= 2;
        sel += 1;
    }
    let search = (pow * 16) as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num as u16).to_be_bytes());
    out.extend_from_slice(&search.to_be_bytes());
    out.extend_from_slice(&sel.to_be_bytes());
    out.extend_from_slice(&((num * 16) as u16 - search).to_be_bytes());
    let mut offset = 12 + num * 16;
    for (tag, csum, data) in &tables {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&csum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += (data.len() + 3) & !3;
    }
    for (_, _, data) in &tables {
        out.extend_from_slice(data);
        out.resize((out.len() + 3) & !3, 0);
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryFace {
    pub path: PathBuf,
    pub info: FaceInfo,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub stored_as: Option<PathBuf>,
    pub added: Vec<FaceInfo>,
    pub duplicate: bool,
}

/// Verzeichnis mit importierten Schriften (global oder projektbezogen).
#[derive(Debug, Default)]
pub struct FontLibrary {
    root: PathBuf,
    faces: Vec<LibraryFace>,
    hashes: HashSet<u64>,
    /// Dateien, die beim Scannen nicht gelesen werden konnten (Pfad, Grund).
    pub errors: Vec<(PathBuf, String)>,
}

/// Inhaltliche Identität einer Schriftdatei: gleiche Schnitte = Duplikat,
/// auch wenn die Datei anders verpackt ist (z. B. WOFF statt TTF).
fn identity(infos: &[FaceInfo]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for i in infos {
        (
            &i.full_name,
            &i.family,
            &i.style,
            i.weight,
            i.italic,
            i.glyph_count,
        )
            .hash(&mut h);
    }
    h.finish()
}

fn sanitize(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "font".into()
    } else {
        s
    }
}

impl FontLibrary {
    /// Öffnet (und legt bei Bedarf an) eine Bibliothek und liest alle Schriften darin.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        let mut lib = Self {
            root,
            ..Default::default()
        };
        lib.rescan()?;
        Ok(lib)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Ordner neu einlesen (für Hot-Reload, wenn Dateien von außen hineinkopiert wurden).
    pub fn rescan(&mut self) -> Result<()> {
        self.faces.clear();
        self.hashes.clear();
        self.errors.clear();
        let mut entries: Vec<_> = std::fs::read_dir(&self.root)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries.into_iter().filter(|p| p.is_file()) {
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(e) => {
                    self.errors.push((path, e.to_string()));
                    continue;
                }
            };
            match inspect(&data) {
                Ok(infos) => {
                    self.hashes.insert(identity(&infos));
                    self.faces.extend(infos.into_iter().map(|info| LibraryFace {
                        path: path.clone(),
                        info,
                    }));
                }
                Err(FontError::UnknownFormat) => {} // fremde Dateien ignorieren
                Err(e) => self.errors.push((path, e.to_string())),
            }
        }
        Ok(())
    }

    /// Importiert eine Schriftdatei: prüfen, WOFF entpacken, Duplikate erkennen, ablegen.
    pub fn import_file(&mut self, src: &Path) -> Result<ImportReport> {
        let data = std::fs::read(src)?;
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.import_bytes(&data, &stem)
    }

    pub fn import_bytes(&mut self, data: &[u8], name_hint: &str) -> Result<ImportReport> {
        let format = FontFormat::detect(data).ok_or(FontError::UnknownFormat)?;
        let (bytes, format) = match format {
            FontFormat::Woff2 => return Err(FontError::Woff2Unsupported),
            FontFormat::Woff => {
                let sfnt = woff_to_sfnt(data)?;
                let f = FontFormat::detect(&sfnt).ok_or(FontError::UnknownFormat)?;
                (sfnt, f)
            }
            f => (data.to_vec(), f),
        };
        let infos = inspect(&bytes)?;
        let hash = identity(&infos);
        if self.hashes.contains(&hash) {
            return Ok(ImportReport {
                stored_as: None,
                added: Vec::new(),
                duplicate: true,
            });
        }
        let base = if infos.len() == 1 {
            sanitize(&format!("{}-{}", infos[0].family, infos[0].style))
        } else {
            sanitize(&format!(
                "{}-collection",
                if name_hint.is_empty() {
                    &infos[0].family
                } else {
                    name_hint
                }
            ))
        };
        let mut target = self.root.join(format!("{base}.{}", format.extension()));
        let mut n = 2;
        while target.exists() {
            target = self.root.join(format!("{base}-{n}.{}", format.extension()));
            n += 1;
        }
        std::fs::write(&target, &bytes)?;
        self.hashes.insert(hash);
        self.faces
            .extend(infos.iter().cloned().map(|info| LibraryFace {
                path: target.clone(),
                info,
            }));
        Ok(ImportReport {
            stored_as: Some(target),
            added: infos,
            duplicate: false,
        })
    }

    pub fn faces(&self) -> &[LibraryFace] {
        &self.faces
    }

    /// Familien → Schnitte, alphabetisch.
    pub fn families(&self) -> BTreeMap<String, Vec<&LibraryFace>> {
        let mut m: BTreeMap<String, Vec<&LibraryFace>> = BTreeMap::new();
        for f in &self.faces {
            m.entry(f.info.family.clone()).or_default().push(f);
        }
        for v in m.values_mut() {
            v.sort_by_key(|f| (f.info.italic, f.info.weight));
        }
        m
    }

    /// Bevorzugter Schnitt einer Familie (Regular/400, nicht kursiv).
    pub fn regular_face(&self, family: &str) -> Option<&LibraryFace> {
        self.faces
            .iter()
            .filter(|f| f.info.family == family)
            .min_by_key(|f| (f.info.italic, (f.info.weight as i32 - 400).abs()))
    }

    /// Entfernt alle Dateien einer Familie aus der Bibliothek.
    pub fn remove_family(&mut self, family: &str) -> Result<usize> {
        let paths: HashSet<PathBuf> = self
            .faces
            .iter()
            .filter(|f| f.info.family == family)
            .map(|f| f.path.clone())
            .collect();
        for p in &paths {
            std::fs::remove_file(p)?;
        }
        self.rescan()?;
        Ok(paths.len())
    }

    /// Welche der benötigten Familien fehlen (für den Dialog „Fehlende Schriften“)?
    pub fn missing<'a>(
        &self,
        needed: impl IntoIterator<Item = &'a str>,
        system: &HashSet<String>,
    ) -> Vec<String> {
        let have: HashSet<&str> = self.faces.iter().map(|f| f.info.family.as_str()).collect();
        let mut v: Vec<String> = needed
            .into_iter()
            .filter(|n| !have.contains(n) && !system.contains(*n))
            .map(str::to_string)
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Misst Textbreite in Pixeln (Advance-Breiten, ohne Kerning) – für Zeilenumbruch beim Export.
pub fn text_width(data: &[u8], index: u32, size: f32, text: &str) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    let upem = face.units_per_em() as f32;
    let fallback = face
        .glyph_hor_advance(face.glyph_index(' ').unwrap_or_default())
        .unwrap_or(0) as f32;
    Some(
        text.chars()
            .map(|c| {
                face.glyph_index(c)
                    .and_then(|g| face.glyph_hor_advance(g))
                    .map(|a| a as f32)
                    .unwrap_or(fallback)
            })
            .sum::<f32>()
            * size
            / upem,
    )
}

/// Bricht Text wortweise auf `max_width` um (bestehende Zeilenumbrüche bleiben erhalten).
pub fn wrap_text(data: &[u8], index: u32, size: f32, text: &str, max_width: f32) -> String {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty()
                && text_width(data, index, size, &candidate).unwrap_or(0.0) > max_width
            {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// Typische System-Schriftordner (nur zum Auflisten, nicht zum Kopieren).
pub fn system_font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        if let Some(w) = std::env::var_os("WINDIR") {
            v.push(PathBuf::from(w).join("Fonts"));
        }
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            v.push(
                PathBuf::from(l)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts"),
            );
        }
    } else if cfg!(target_os = "macos") {
        v.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join("Library/Fonts"));
        }
    } else {
        v.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join(".local/share/fonts"));
        }
    }
    v
}

/// Familiennamen aller Systemschriften (rekursiv, Fehler werden übersprungen).
pub fn system_families() -> HashSet<String> {
    fn walk(dir: &Path, out: &mut HashSet<String>, depth: u8) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 4 {
                walk(&p, out, depth + 1);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| FONT_EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
            {
                if let Ok(d) = std::fs::read(&p) {
                    if let Ok(infos) = inspect(&d) {
                        out.extend(infos.into_iter().map(|i| i.family));
                    }
                }
            }
        }
    }
    let mut out = HashSet::new();
    for d in system_font_dirs() {
        walk(&d, &mut out, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eine echte Systemschrift für Tests (Windows: Arial/Impact, Linux: DejaVu).
    fn sample_font() -> Option<PathBuf> {
        let candidates = [
            "C:/Windows/Fonts/impact.ttf",
            "C:/Windows/Fonts/arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
        ];
        candidates.iter().map(PathBuf::from).find(|p| p.is_file())
    }

    /// Baut aus einer SFNT-Datei ein WOFF 1.0 (zum Testen des Entpackens).
    fn sfnt_to_woff(s: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let num = u16::from_be_bytes([s[4], s[5]]) as usize;
        let mut entries = Vec::new();
        for i in 0..num {
            let r = 12 + i * 16;
            let tag = &s[r..r + 4];
            let csum = &s[r + 4..r + 8];
            let off = u32::from_be_bytes(s[r + 8..r + 12].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(s[r + 12..r + 16].try_into().unwrap()) as usize;
            let raw = &s[off..off + len];
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(raw).unwrap();
            let comp = enc.finish().unwrap();
            let data = if comp.len() < raw.len() {
                comp
            } else {
                raw.to_vec()
            };
            entries.push((tag.to_vec(), csum.to_vec(), len, data));
        }
        let mut out = vec![0u8; 44 + num * 20];
        out[0..4].copy_from_slice(b"wOFF");
        out[4..8].copy_from_slice(&s[0..4]);
        out[12..14].copy_from_slice(&(num as u16).to_be_bytes());
        for (i, (tag, csum, orig, data)) in entries.iter().enumerate() {
            let off = out.len();
            let e = 44 + i * 20;
            out[e..e + 4].copy_from_slice(tag);
            out[e + 4..e + 8].copy_from_slice(&(off as u32).to_be_bytes());
            out[e + 8..e + 12].copy_from_slice(&(data.len() as u32).to_be_bytes());
            out[e + 12..e + 16].copy_from_slice(&(*orig as u32).to_be_bytes());
            out[e + 16..e + 20].copy_from_slice(csum);
            out.extend_from_slice(data);
            out.resize((out.len() + 3) & !3, 0);
        }
        let total = out.len() as u32;
        out[8..12].copy_from_slice(&total.to_be_bytes());
        out
    }

    #[test]
    fn import_ttf_dedupe_woff_and_remove() {
        let Some(src) = sample_font() else {
            eprintln!("Keine Testschrift gefunden, übersprungen");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mut lib = FontLibrary::open(dir.path()).unwrap();

        let rep = lib.import_file(&src).unwrap();
        assert!(!rep.duplicate);
        let family = rep.added[0].family.clone();
        assert!(rep.stored_as.as_ref().unwrap().exists());
        assert!(lib.families().contains_key(&family));
        assert!(rep.added[0].glyph_count > 100);

        // Zweiter Import derselben Datei: Duplikat, nichts kopiert.
        assert!(lib.import_file(&src).unwrap().duplicate);

        // Als WOFF verpackt → wird erkannt, entpackt und ist wieder ein Duplikat.
        let woff = sfnt_to_woff(&std::fs::read(&src).unwrap());
        assert_eq!(FontFormat::detect(&woff), Some(FontFormat::Woff));
        assert_eq!(inspect(&woff).unwrap()[0].family, family);
        assert!(lib.import_bytes(&woff, "x").unwrap().duplicate);

        // Neu öffnen (Hot-Reload) findet die Schrift wieder.
        let lib2 = FontLibrary::open(dir.path()).unwrap();
        assert!(lib2.regular_face(&family).is_some());

        let empty = HashSet::new();
        assert_eq!(
            lib.missing([family.as_str(), "Gibt Es Nicht"], &empty),
            vec!["Gibt Es Nicht".to_string()]
        );

        assert_eq!(lib.remove_family(&family).unwrap(), 1);
        assert!(lib.faces().is_empty());
    }

    #[test]
    fn measure_and_wrap() {
        let Some(src) = sample_font() else { return };
        let d = std::fs::read(src).unwrap();
        let w1 = text_width(&d, 0, 100.0, "Hallo").unwrap();
        let w2 = text_width(&d, 0, 200.0, "Hallo").unwrap();
        assert!(w1 > 100.0 && (w2 / w1 - 2.0).abs() < 0.01);
        let wrapped = wrap_text(&d, 0, 100.0, "eins zwei drei vier fünf sechs", w1 * 1.5);
        assert!(wrapped.lines().count() >= 3, "{wrapped}");
        assert_eq!(wrapped.replace('\n', " "), "eins zwei drei vier fünf sechs");
    }

    #[test]
    fn rejects_garbage_and_woff2() {
        assert!(matches!(
            inspect(b"hallo welt"),
            Err(FontError::UnknownFormat)
        ));
        assert!(matches!(
            inspect(b"wOF2\0\0\0\0"),
            Err(FontError::Woff2Unsupported)
        ));
        assert!(matches!(inspect(b"OTTO\0\0"), Err(FontError::Corrupt(_))));
    }
}
