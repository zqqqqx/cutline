//! Inhaltspakete nachladen: Liste `index.json` und ZIPs am festen Vorab-Release `packs`.
//!
//! Abgerufen wird nur über HTTPS von der festen Adresse [`BASE`] (GitHub leitet auf seinen
//! Download-Server weiter, auch das nur über HTTPS). Dateinamen aus der Liste sind einfache Namen,
//! nie Pfade oder Adressen; die Größe aus der Liste begrenzt den Download, die Prüfsumme muss passen.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::Version;

/// Feste Adresse des Vorab-Release mit Liste und Paketen
pub const BASE: &str = "https://github.com/zqqqqx/cutline/releases/download/packs/";
pub const INDEX_FILE: &str = "index.json";
/// Unterstütztes Listenformat
pub const FORMAT: u32 = 1;
/// Höchstgröße der Liste
const MAX_INDEX: u64 = 1 << 20;
/// Absolute Obergrenze eines Pakets (wie die Entpack-Grenze des ZIP-Einbaus)
pub const MAX_PACK: u64 = 2 << 30;

/// Ein Paket der Liste
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct IndexPack {
    pub id: String,
    #[serde(default)]
    pub name: BTreeMap<String, String>,
    pub version: String,
    pub api: u32,
    #[serde(default)]
    pub description: BTreeMap<String, String>,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub counts: BTreeMap<String, usize>,
    #[serde(default)]
    pub license: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexError {
    /// Server/Ordner nicht erreichbar
    Unreachable(String),
    /// Unbekanntes `format`: Cutline ist zu alt
    Newer,
    /// Liste nicht lesbar
    Broken(String),
}

/// `file` aus der Liste: einfacher Dateiname ohne Pfad, Adresse oder Sonderzeichen.
pub fn valid_file(f: &str) -> bool {
    !f.is_empty()
        && f.len() <= 128
        && !f.starts_with('.')
        && f.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Paket-ID: wie ein Ordnername unter `Plugins` (keine Pfade)
fn valid_id(id: &str) -> bool {
    valid_file(id) && !id.contains("..")
}

fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Liste lesen. Einzelne unvollständige oder unsichere Einträge fallen weg, der Rest bleibt.
pub fn parse_index(json: &str) -> Result<Vec<IndexPack>, IndexError> {
    #[derive(Deserialize)]
    struct Raw {
        format: u32,
        #[serde(default)]
        packs: Vec<serde_json::Value>,
    }
    let raw: Raw = serde_json::from_str(json).map_err(|e| IndexError::Broken(e.to_string()))?;
    if raw.format != FORMAT {
        return Err(IndexError::Newer);
    }
    Ok(raw
        .packs
        .into_iter()
        .filter_map(|v| serde_json::from_value::<IndexPack>(v).ok())
        .filter(|p| {
            valid_id(&p.id) && valid_file(&p.file) && valid_sha(&p.sha256) && Version::parse(&p.version).is_some()
        })
        .collect())
}

/// Größe, ab der ein Download abgebrochen wird: Größe aus der Liste plus kleine Toleranz.
pub fn byte_limit(size: u64) -> u64 {
    size.saturating_add(size / 100 + 4096).min(MAX_PACK)
}

/// Lokalisierter Text mit Rückfall (Englisch, erster, `fallback`)
pub fn text<'a>(t: &'a BTreeMap<String, String>, lang: &str, fallback: &'a str) -> &'a str {
    t.get(lang).or_else(|| t.get("en")).or_else(|| t.values().next()).map_or(fallback, String::as_str)
}

/// Ein installiertes Paket: ID, Version, eigenes (nachgeladenes) Paket?
pub struct Installed<'a> {
    pub id: &'a str,
    pub version: &'a str,
    pub own: bool,
}

/// Was die Seite zeigt: nicht installierte Pakete (nach Name sortiert, ohne zu alte API) und
/// Aktualisierungen eigener Pakete (ID → Eintrag). Mitgelieferte gewinnen beim Laden, für sie
/// gibt es keine Aktualisierung.
pub fn plan<'a>(index: &'a [IndexPack], installed: &[Installed], api: u32, lang: &str) -> (Vec<&'a IndexPack>, BTreeMap<String, &'a IndexPack>) {
    let mut avail: Vec<&IndexPack> =
        index.iter().filter(|p| p.api >= api && !installed.iter().any(|i| i.id == p.id)).collect();
    avail.sort_by_cached_key(|p| text(&p.name, lang, &p.id).to_lowercase());
    avail.dedup_by(|a, b| a.id == b.id);
    let updates = installed
        .iter()
        .filter(|i| i.own)
        .filter_map(|i| {
            let have = Version::parse(i.version)?;
            let p = index.iter().filter(|p| p.id == i.id && p.api == api).max_by_key(|p| Version::parse(&p.version))?;
            (Version::parse(&p.version)? > have).then(|| (i.id.to_string(), p))
        })
        .collect();
    (avail, updates)
}

/// Woher Liste und Pakete kommen
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Feste Adresse auf GitHub
    Github,
    /// Lokaler Ordner (Tests, Klicktests ohne Netz)
    Dir(PathBuf),
}

type Stream = (Box<dyn Read + Send>, Option<u64>);

impl Source {
    /// Datei öffnen: Leser und – falls bekannt – Länge.
    pub fn open(&self, file: &str) -> Result<Stream, String> {
        if !valid_file(file) {
            return Err(format!("Ungültiger Dateiname: {file}"));
        }
        match self {
            Source::Dir(d) => {
                let f = std::fs::File::open(d.join(file)).map_err(|e| format!("{file}: {e}"))?;
                let len = f.metadata().ok().map(|m| m.len());
                Ok((Box::new(f), len))
            }
            Source::Github => {
                let agent = ureq::AgentBuilder::new()
                    .https_only(true)
                    .timeout_connect(std::time::Duration::from_secs(8))
                    .timeout_read(std::time::Duration::from_secs(30))
                    .build();
                match agent.get(&format!("{BASE}{file}")).set("User-Agent", "cutline").call() {
                    Ok(r) => {
                        let len = r.header("Content-Length").and_then(|v| v.parse().ok());
                        Ok((Box::new(r.into_reader()), len))
                    }
                    Err(ureq::Error::Status(code, _)) => Err(format!("GitHub antwortet mit {code}")),
                    Err(e) => Err(format!("Keine Verbindung zu GitHub ({e})")),
                }
            }
        }
    }
}

/// Liste abrufen und lesen.
pub fn fetch_index(src: &Source) -> Result<Vec<IndexPack>, IndexError> {
    let (r, _) = src.open(INDEX_FILE).map_err(IndexError::Unreachable)?;
    let mut s = String::new();
    r.take(MAX_INDEX + 1).read_to_string(&mut s).map_err(|e| IndexError::Unreachable(e.to_string()))?;
    if s.len() as u64 > MAX_INDEX {
        return Err(IndexError::Broken("Liste zu groß".into()));
    }
    parse_index(&s)
}

/// Paket in `dir` laden (Zwischendatei `<pid>-<file>.part`) und gegen die Liste prüfen.
/// Gibt die geprüfte Datei zurück; bei jedem Fehler und bei Abbruch bleibt nichts liegen.
pub fn download_pack(
    src: &Source,
    p: &IndexPack,
    dir: &Path,
    mut progress: impl FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let limit = byte_limit(p.size);
    let (mut r, len) = src.open(&p.file)?;
    if len.is_some_and(|l| l > limit) {
        return Err("Paket ist größer als angegeben".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{}-{}.part", std::process::id(), p.file));
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let res = (|| -> Result<(), String> {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Abgebrochen".into());
            }
            let n = r.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > limit {
                return Err("Paket ist größer als angegeben".into());
            }
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            hasher.update(&buf[..n]);
            progress(done, p.size.max(1));
        }
        f.flush().map_err(|e| e.to_string())?;
        let got: String = hasher.finalize_reset().iter().map(|b| format!("{b:02x}")).collect();
        if !got.eq_ignore_ascii_case(&p.sha256) {
            return Err("Prüfsumme stimmt nicht".into());
        }
        Ok(())
    })();
    drop(f);
    match res {
        Ok(()) => Ok(part),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Zwischendateien beendeter Instanzen löschen (`alive`: läuft der Prozess noch?).
pub fn cleanup_parts(dir: &Path, alive: impl Fn(u32) -> bool) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let pid = name.split_once('-').and_then(|(p, _)| p.parse::<u32>().ok());
        if name.ends_with(".part") && !pid.is_some_and(&alive) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "name": {"de": format!("Name {id}"), "en": format!("Name {id} en")}, "version": version,
            "api": 1, "description": {"de": "d"}, "file": format!("{id}.zip"), "size": 10,
            "sha256": "ab".repeat(32), "counts": {"sound": 3}, "license": "CC0-1.0"
        })
    }

    fn index(packs: Vec<serde_json::Value>) -> String {
        serde_json::json!({ "format": 1, "packs": packs }).to_string()
    }

    #[test]
    fn index_is_read_leniently_but_safely() {
        let mut extra = entry("a.b", "1.0.0");
        extra["neu"] = "egal".into();
        let mut minimal = entry("c.d", "2.0");
        for k in ["name", "description", "counts", "license"] {
            minimal.as_object_mut().unwrap().remove(k);
        }
        let mut no_sha = entry("e.f", "1.0.0");
        no_sha.as_object_mut().unwrap().remove("sha256");
        let mut bad = vec![no_sha];
        for (k, v) in [("file", "../x.zip"), ("file", "https://x/y.zip"), ("file", "a/b.zip"), ("file", r"a\b.zip"), ("file", ""), ("sha256", "xyz"), ("version", "neu"), ("id", "../x")] {
            let mut e = entry("g.h", "1.0.0");
            e[k] = v.into();
            bad.push(e);
        }
        let mut all = vec![extra, minimal];
        all.extend(bad);
        let got = parse_index(&index(all)).unwrap();
        assert_eq!(got.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["a.b", "c.d"]);
        assert_eq!(got[1].license, "");
        assert_eq!(text(&got[1].name, "de", "c.d"), "c.d", "ohne Name: ID");
        assert_eq!(text(&got[0].name, "fr", ""), "Name a.b en");
        assert_eq!(parse_index(&serde_json::json!({"format": 2, "packs": []}).to_string()), Err(IndexError::Newer));
        assert!(matches!(parse_index("<html>"), Err(IndexError::Broken(_))));
        assert!(matches!(parse_index("{\"packs\": []}"), Err(IndexError::Broken(_))));
        assert_eq!(parse_index("{\"format\": 1}"), Ok(vec![]));
    }

    #[test]
    fn file_names_are_plain() {
        for ok in ["kenney-emotes.zip", "a_b.1.zip"] {
            assert!(valid_file(ok), "{ok}");
        }
        for bad in ["", ".zip", "..", "../a.zip", "a/b", "C:x.zip", "a b.zip", "http://x", "ä.zip", &"a".repeat(200)] {
            assert!(!valid_file(bad), "{bad}");
        }
    }

    #[test]
    fn limit_allows_a_little_more() {
        assert_eq!(byte_limit(0), 4096);
        assert_eq!(byte_limit(100_000), 105_096);
        assert_eq!(byte_limit(u64::MAX), MAX_PACK);
    }

    #[test]
    fn plan_lists_missing_and_updates_own_packs() {
        let mut list = parse_index(&index(vec![entry("z.z", "1.0.0"), entry("a.a", "1.2.0"), entry("m.m", "2.0.0"), entry("b.b", "1.0.0")])).unwrap();
        list[0].name.insert("de".into(), "Anfang".into());
        list.push(IndexPack { api: 2, id: "neu.api".into(), ..list[0].clone() });
        list.push(IndexPack { api: 0, id: "alt.api".into(), ..list[0].clone() });
        let inst = [
            Installed { id: "a.a", version: "1.1.9", own: true },
            Installed { id: "m.m", version: "2.0.0", own: true },
            Installed { id: "b.b", version: "0.1.0", own: false },
        ];
        let (avail, upd) = plan(&list, &inst, 1, "de");
        let ids: Vec<_> = avail.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["z.z", "neu.api"], "nach Name; zu alte API fehlt");
        assert_eq!(upd.keys().collect::<Vec<_>>(), ["a.a"], "gleich neu und mitgeliefert: nichts");
        let (_, upd) = plan(&list, &[Installed { id: "a.a", version: "kaputt", own: true }], 1, "de");
        assert!(upd.is_empty());
    }

    fn pack(file: &str, data: &[u8]) -> IndexPack {
        IndexPack {
            id: "t.t".into(),
            name: BTreeMap::new(),
            version: "1.0.0".into(),
            api: 1,
            description: BTreeMap::new(),
            file: file.into(),
            size: data.len() as u64,
            sha256: crate::sha256_hex(data),
            counts: BTreeMap::new(),
            license: String::new(),
        }
    }

    #[test]
    fn download_checks_size_sha_and_cancel() {
        let src = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7) as u8).collect();
        std::fs::write(src.path().join("p.zip"), &data).unwrap();
        let s = Source::Dir(src.path().into());
        let no = AtomicBool::new(false);
        let empty = |d: &Path| std::fs::read_dir(d).map(|r| r.count()).unwrap_or(0) == 0;
        // richtig, mit Fortschritt
        let mut seen = Vec::new();
        let f = download_pack(&s, &pack("p.zip", &data), tmp.path(), |d, t| seen.push((d, t)), &no).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), data);
        assert_eq!(seen.last(), Some(&(300_000, 300_000)));
        std::fs::remove_file(f).unwrap();
        // falsche Prüfsumme
        let mut p = pack("p.zip", &data);
        p.sha256 = crate::sha256_hex(b"anders");
        assert!(download_pack(&s, &p, tmp.path(), |_, _| {}, &no).unwrap_err().contains("Prüfsumme"));
        assert!(empty(tmp.path()));
        // größer als angegeben
        let p = pack("p.zip", &data[..1000]);
        assert!(download_pack(&s, &p, tmp.path(), |_, _| {}, &no).unwrap_err().contains("größer"));
        assert!(empty(tmp.path()));
        // Abbruch mitten im Download
        let cancel = AtomicBool::new(false);
        let r = download_pack(&s, &pack("p.zip", &data), tmp.path(), |_, _| cancel.store(true, Ordering::Relaxed), &cancel);
        assert_eq!(r.unwrap_err(), "Abgebrochen");
        assert!(empty(tmp.path()));
        // Dateiname aus der Liste darf nicht aus dem Ordner führen
        assert!(download_pack(&s, &pack("../p.zip", &data), tmp.path(), |_, _| {}, &no).is_err());
        // nicht erreichbar
        assert!(matches!(fetch_index(&Source::Dir(src.path().join("fehlt"))), Err(IndexError::Unreachable(_))));
    }

    #[test]
    fn leftover_parts_of_dead_instances_are_removed() {
        let d = tempfile::tempdir().unwrap();
        for n in ["1-a.zip.part", "2-b.zip.part", "x-c.zip.part", "3-fremd.txt"] {
            std::fs::write(d.path().join(n), b"x").unwrap();
        }
        cleanup_parts(d.path(), |pid| pid == 2);
        let mut left: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["2-b.zip.part", "3-fremd.txt"]);
        cleanup_parts(&d.path().join("fehlt"), |_| false);
    }

    /// Die mitgelieferte Testkopie der Liste passt zu ihren ZIPs (Größe, SHA-256).
    #[test]
    fn real_index_matches_its_files() {
        let dir = Path::new("F:/cutline-pakete/fertig");
        if !dir.join(INDEX_FILE).is_file() {
            return;
        }
        let s = Source::Dir(dir.into());
        let list = fetch_index(&s).unwrap();
        assert!(!list.is_empty());
        let tmp = tempfile::tempdir().unwrap();
        for p in &list {
            assert_eq!(std::fs::metadata(dir.join(&p.file)).unwrap().len(), p.size, "{}", p.file);
            let f = download_pack(&s, p, tmp.path(), |_, _| {}, &AtomicBool::new(false)).unwrap();
            std::fs::remove_file(f).unwrap();
        }
    }
}
