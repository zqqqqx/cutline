//! Automatische Updates über GitHub Releases.
//!
//! Ablauf:
//! 1. [`check`] fragt `releases/latest` des Repositorys ab und vergleicht die Version
//!    (SemVer, `v`-Präfix erlaubt) mit der laufenden.
//! 2. [`download`] lädt das passende Paket (`cutline-<os>-<arch>.zip`) und prüft es gegen
//!    die mitveröffentlichte `.sha256`-Datei. Ohne Prüfsumme wird nicht installiert.
//! 3. [`install`] entpackt in den Programmordner. Laufende Dateien (die EXE selbst) werden
//!    vorher in `*.old` umbenannt, das erlaubt Windows auch für laufende Programme.
//!    Beim nächsten Start räumt [`cleanup_old`] diese Reste weg.
//!
//! Es werden keine Daten übertragen außer der normalen HTTP-Anfrage an api.github.com.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// GitHub-Repository mit den Releases.
pub const REPO: &str = "zqqqqx/cutline";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `v1.2.3`, `1.2.3`, `1.2.3-beta.1` (Vorabversionen zählen wie die Basisversion).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let core = s.split(['-', '+']).next()?;
        let mut it = core.split('.').map(|p| p.parse::<u64>());
        let v = Version(
            it.next()?.ok()?,
            it.next().unwrap_or(Ok(0)).ok()?,
            it.next().unwrap_or(Ok(0)).ok()?,
        );
        Some(v)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub name: String,
    pub notes: String,
    pub page: String,
    pub asset_url: String,
    pub asset_size: u64,
    pub sha_url: Option<String>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// Name des Pakets für dieses System.
pub fn asset_name() -> String {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        _ => "linux",
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "x64",
    };
    format!("cutline-{os}-{arch}.zip")
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout_read(std::time::Duration::from_secs(30))
        .build()
}

/// Release-JSON auswerten (eigene Funktion, damit sie ohne Netz testbar ist).
pub fn parse_release(json: &str, current: Version) -> Result<Option<Release>, String> {
    let r: GhRelease =
        serde_json::from_str(json).map_err(|e| format!("Antwort nicht lesbar: {e}"))?;
    if r.draft || r.prerelease {
        return Ok(None);
    }
    let Some(version) = Version::parse(&r.tag_name) else {
        return Err(format!("Unbekanntes Versionsformat: {}", r.tag_name));
    };
    if version <= current {
        return Ok(None);
    }
    let want = asset_name();
    let Some(asset) = r.assets.iter().find(|a| a.name == want) else {
        return Err(format!(
            "Version {version} hat kein Paket für dieses System ({want})"
        ));
    };
    let sha = r
        .assets
        .iter()
        .find(|a| a.name == format!("{want}.sha256"))
        .map(|a| a.browser_download_url.clone());
    Ok(Some(Release {
        version,
        name: r
            .name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| r.tag_name.clone()),
        tag: r.tag_name,
        notes: r.body.unwrap_or_default(),
        page: r.html_url,
        asset_url: asset.browser_download_url.clone(),
        asset_size: asset.size,
        sha_url: sha,
    }))
}

/// Neueste Version abfragen. `Ok(None)` = schon aktuell.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let cur = Version::parse(current).ok_or("Eigene Version unlesbar")?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    match agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "cutline-updater")
        .call()
    {
        Ok(resp) => {
            let body = resp.into_string().map_err(|e| e.to_string())?;
            parse_release(&body, cur)
        }
        Err(ureq::Error::Status(404, _)) => Err("Noch keine öffentlichen Releases gefunden".into()),
        Err(ureq::Error::Status(code, _)) => Err(format!("GitHub antwortet mit {code}")),
        Err(e) => Err(format!("Keine Verbindung zu GitHub ({e})")),
    }
}

/// SHA-256 als Hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let d = Sha256::digest(data);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Erwartete Prüfsumme aus einer `.sha256`-Datei (`<hex>  <datei>` oder nur `<hex>`).
pub fn parse_sha_file(s: &str) -> Option<String> {
    let h = s.split_whitespace().next()?.to_ascii_lowercase();
    (h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// Paket laden und prüfen. Gibt den Pfad der geprüften ZIP-Datei zurück.
pub fn download(
    rel: &Release,
    dir: &Path,
    mut progress: impl FnMut(f32),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let sha_url = rel
        .sha_url
        .as_ref()
        .ok_or("Release ohne Prüfsumme – Installation aus Sicherheitsgründen abgebrochen")?;
    let expected = agent()
        .get(sha_url)
        .set("User-Agent", "cutline-updater")
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let expected = parse_sha_file(&expected).ok_or("Prüfsummendatei unlesbar")?;
    let resp = agent()
        .get(&rel.asset_url)
        .set("User-Agent", "cutline-updater")
        .call()
        .map_err(|e| e.to_string())?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(rel.asset_size)
        .max(1);
    let mut reader = resp.into_reader();
    // Content-Length kommt vom Server: nicht blind vorab reservieren (Absturz bei Riesenwerten)
    let mut data = Vec::with_capacity(total.min(256 << 20) as usize);
    let mut buf = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen".into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        progress((data.len() as f64 / total as f64).min(1.0) as f32);
    }
    let got = sha256_hex(&data);
    if got != expected {
        return Err(format!(
            "Prüfsumme stimmt nicht (erwartet {}…, erhalten {}…)",
            &expected[..12],
            &got[..12]
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let out = dir.join(asset_name());
    std::fs::write(&out, &data).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Paket in `target` entpacken. Vorhandene Dateien werden zu `*.old` umbenannt
/// (funktioniert auch für die laufende EXE). Gibt die Zahl der Dateien zurück.
pub fn install(zip_path: &Path, target: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("Paket beschädigt: {e}"))?;
    // Erst alles prüfen und in einen Zwischenordner entpacken, dann austauschen:
    // so bleibt bei einem Fehler die alte Installation unangetastet.
    let staging = target.join(".update-staging");
    let _ = std::fs::remove_dir_all(&staging);
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("Unsicherer Pfad im Paket: {}", entry.name().unwrap_or_default()));
        };
        let dst = staging.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&dst).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode));
        }
        files.push(rel);
    }
    if files.is_empty() {
        return Err("Paket ist leer".into());
    }
    // Liste der umbenannten Dateien: cleanup_old löscht nur diese, nie fremde `*.old`.
    let mut list = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(target.join(OLD_LIST))
        .map_err(|e| e.to_string())?;
    // getauschte Dateien (Ziel, gab es eine alte Fassung?): bei einem Fehler zurückdrehen
    let mut done: Vec<(PathBuf, bool)> = Vec::new();
    let mut swap = |rel: &PathBuf, done: &mut Vec<(PathBuf, bool)>| -> Result<(), String> {
        let dst = target.join(rel);
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let had = dst.exists();
        if had {
            let old = old_name(&dst);
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&dst, &old).map_err(|e| format!("{}: {e}", dst.display()))?;
            writeln!(list, "{}", old_name(rel).to_string_lossy().replace('\\', "/"))
                .map_err(|e| e.to_string())?;
        }
        done.push((dst.clone(), had));
        std::fs::rename(staging.join(rel), &dst).map_err(|e| format!("{}: {e}", dst.display()))
    };
    for rel in &files {
        if let Err(e) = swap(rel, &mut done) {
            for (dst, had) in done.iter().rev() {
                let _ = std::fs::remove_file(dst);
                if *had {
                    let _ = std::fs::rename(old_name(dst), dst);
                }
            }
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(files.len())
}

fn old_name(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Datei laden (für Modelle). Schreibt erst `*.part` und benennt am Ende um.
/// Endet die URL auf `#sha256=<hex>`, wird die Datei vorher gegen diese Prüfsumme geprüft.
pub fn download_to(
    url: &str,
    dest: &Path,
    mut progress: impl FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let (url, want) = split_sha(url);
    let resp = agent()
        .get(url)
        .set("User-Agent", "cutline")
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut r = resp.into_reader();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut hasher = Sha256::new();
    let copied = (|| -> Result<(), String> {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Abgebrochen".into());
            }
            let n = r.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            hasher.update(&buf[..n]);
            done += n as u64;
            progress(done, total);
        }
        f.flush().map_err(|e| e.to_string())
    })();
    drop(f);
    // Abbruch, Lese- oder Schreibfehler: keine halbe `.part`-Datei liegen lassen
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if total > 0 && done != total {
        let _ = std::fs::remove_file(&part);
        return Err("Download unvollständig".into());
    }
    if let Some(want) = want {
        let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if !got.eq_ignore_ascii_case(want) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{url}: Prüfsumme stimmt nicht"));
        }
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

/// `url#sha256=<hex>` → (url, Some(hex)); sonst (url, None).
fn split_sha(url: &str) -> (&str, Option<&str>) {
    match url.split_once("#sha256=") {
        Some((u, h)) => (u, Some(h)),
        None => (url, None),
    }
}

/// ZIP-Archiv in einen Ordner entpacken (unsichere Pfade werden abgelehnt).
pub fn extract_zip(zip_path: &Path, dir: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    let mut n = 0;
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = e.enclosed_name() else {
            return Err(format!("Unsicherer Pfad: {}", e.name().unwrap_or_default()));
        };
        let dst = dir.join(rel);
        if e.is_dir() {
            std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&dst).map_err(|e| e.to_string())?;
        std::io::copy(&mut e, &mut out).map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(n)
}

/// Datei im Programmordner mit den von [`install`] umbenannten `*.old`-Dateien.
const OLD_LIST: &str = ".update-old.txt";

/// Reste eines früheren Updates entfernen: nur die in [`OLD_LIST`] verzeichneten Dateien.
pub fn cleanup_old(dir: &Path) {
    if let Ok(list) = std::fs::read_to_string(dir.join(OLD_LIST)) {
        for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let rel = Path::new(line);
            // Nur relative Pfade ohne `..` – nichts außerhalb des Programmordners löschen.
            if rel
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                let _ = std::fs::remove_file(dir.join(rel));
            }
        }
        let _ = std::fs::remove_file(dir.join(OLD_LIST));
    }
    let _ = std::fs::remove_dir_all(dir.join(".update-staging"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert_eq!(Version::parse("v0.2.0"), Some(Version(0, 2, 0)));
        assert_eq!(Version::parse("1.10"), Some(Version(1, 10, 0)));
        assert_eq!(Version::parse("2.0.1-beta.3"), Some(Version(2, 0, 1)));
        assert!(Version::parse("v0.10.0").unwrap() > Version::parse("0.9.9").unwrap());
        assert!(Version::parse("abc").is_none());
    }

    fn release_json(tag: &str, with_sha: bool) -> String {
        let a = asset_name();
        let mut assets = vec![
            serde_json::json!({ "name": a, "browser_download_url": "https://x/pkg.zip", "size": 10 }),
        ];
        if with_sha {
            assets.push(serde_json::json!({ "name": format!("{a}.sha256"), "browser_download_url": "https://x/pkg.sha256", "size": 1 }));
        }
        serde_json::json!({ "tag_name": tag, "name": "Cutline", "body": "Neu: alles", "html_url": "https://github.com/x", "assets": assets }).to_string()
    }

    #[test]
    fn release_selection() {
        let cur = Version(0, 2, 0);
        assert!(
            parse_release(&release_json("v0.2.0", true), cur)
                .unwrap()
                .is_none(),
            "gleiche Version"
        );
        let r = parse_release(&release_json("v0.3.0", true), cur)
            .unwrap()
            .unwrap();
        assert_eq!(r.version, Version(0, 3, 0));
        assert!(r.sha_url.is_some());
        assert_eq!(r.notes, "Neu: alles");
        let mut j: serde_json::Value = serde_json::from_str(&release_json("v0.3.0", true)).unwrap();
        j["prerelease"] = true.into();
        assert!(
            parse_release(&j.to_string(), cur).unwrap().is_none(),
            "Vorabversionen werden ignoriert"
        );
        j["prerelease"] = false.into();
        j["assets"] = serde_json::json!([]);
        assert!(
            parse_release(&j.to_string(), cur).is_err(),
            "kein Paket für dieses System"
        );
    }

    #[test]
    fn sha_files() {
        let h = sha256_hex(b"cutline");
        assert_eq!(h.len(), 64);
        assert_eq!(
            parse_sha_file(&format!("{h}  cutline-windows-x64.zip\n")),
            Some(h.clone())
        );
        assert_eq!(parse_sha_file(&h.to_uppercase()), Some(h));
        assert_eq!(parse_sha_file("kaputt"), None);
    }

    #[test]
    fn download_verifies_sha_from_url() {
        // kleiner lokaler HTTP-Server, liefert immer „hallo“
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in l.incoming().take(2) {
                let mut c = c.unwrap();
                let mut b = [0u8; 1024];
                let _ = c.read(&mut b);
                let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhallo");
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let url = format!("http://127.0.0.1:{port}/f");
        let good = format!("{url}#sha256={}", sha256_hex(b"hallo"));
        let bad = format!("{url}#sha256={}", sha256_hex(b"anders"));
        let no = AtomicBool::new(false);
        assert!(download_to(&bad, &dir.path().join("b"), |_, _| {}, &no).is_err());
        assert!(!dir.path().join("b").exists());
        download_to(&good, &dir.path().join("g"), |_, _| {}, &no).unwrap();
        assert_eq!(std::fs::read(dir.path().join("g")).unwrap(), b"hallo");
    }

    #[test]
    fn install_replaces_files_and_keeps_old_until_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        std::fs::create_dir_all(app.join("plugins/basis")).unwrap();
        std::fs::write(app.join("cutline.exe"), b"alt").unwrap();
        std::fs::write(app.join("plugins/basis/plugin.toml"), b"alt").unwrap();
        // Paket bauen
        let zp = dir.path().join("pkg.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("cutline.exe", o).unwrap();
            w.write_all(b"neu").unwrap();
            w.start_file("plugins/basis/plugin.toml", o).unwrap();
            w.write_all(b"neu").unwrap();
            w.start_file("ffmpeg/ffmpeg.exe", o).unwrap();
            w.write_all(b"ff").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(install(&zp, &app).unwrap(), 3);
        assert_eq!(std::fs::read(app.join("cutline.exe")).unwrap(), b"neu");
        assert_eq!(std::fs::read(app.join("cutline.exe.old")).unwrap(), b"alt");
        assert_eq!(std::fs::read(app.join("ffmpeg/ffmpeg.exe")).unwrap(), b"ff");
        assert!(!app.join(".update-staging").exists());
        std::fs::write(app.join("fremd.old"), b"x").unwrap();
        std::fs::write(dir.path().join("x.old"), b"x").unwrap();
        // Manipulierte Liste: Pfad außerhalb des Ordners wird ignoriert.
        let mut l = std::fs::OpenOptions::new().append(true).open(app.join(OLD_LIST)).unwrap();
        writeln!(l, "../x.old").unwrap();
        drop(l);
        cleanup_old(&app);
        assert!(app.join("fremd.old").exists());
        assert!(dir.path().join("x.old").exists());
        assert!(!app.join(OLD_LIST).exists());
        assert!(!app.join("cutline.exe.old").exists());
        assert!(!app.join("plugins/basis/plugin.toml.old").exists());
        assert!(app.join("cutline.exe").exists());
    }

    #[test]
    fn failed_swap_restores_already_replaced_files() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("cutline.exe"), b"alt").unwrap();
        // „b“ ist eine Datei: der Ordner für b/c.txt lässt sich nicht anlegen
        std::fs::write(app.join("b"), b"x").unwrap();
        let zp = dir.path().join("pkg.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            for (name, data) in [("cutline.exe", b"neu"), ("neu.dll", b"neu"), ("b/c.txt", b"neu")] {
                w.start_file(name, o).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        assert!(install(&zp, &app).is_err());
        assert_eq!(std::fs::read(app.join("cutline.exe")).unwrap(), b"alt");
        assert!(!app.join("cutline.exe.old").exists());
        assert!(!app.join("neu.dll").exists());
    }

    #[test]
    fn rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let zp = dir.path().join("evil.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            w.start_file("../boese.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let app = dir.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        assert!(install(&zp, &app).is_err());
        assert!(!dir.path().join("boese.exe").exists());
    }

    /// Kleiner HTTP-Server: beantwortet der Reihe nach je eine Verbindung mit der Rohantwort.
    fn serve(answers: Vec<Vec<u8>>) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for a in answers {
                let Ok((mut c, _)) = l.accept() else { return };
                let mut b = [0u8; 2048];
                let _ = c.read(&mut b);
                let _ = c.write_all(&a);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn ok(body: &[u8], len: Option<u64>) -> Vec<u8> {
        let mut v = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            len.unwrap_or(body.len() as u64)
        )
        .into_bytes();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn version_parsing_edge_cases() {
        for (s, v) in [
            ("01.002.0003", Version(1, 2, 3)),
            (" V2 ", Version(2, 0, 0)),
            ("1.2.3.4", Version(1, 2, 3)),
            ("1.2.3+build.7", Version(1, 2, 3)),
            ("0.10.0-rc.1", Version(0, 10, 0)),
        ] {
            assert_eq!(Version::parse(s), Some(v), "{s}");
        }
        for bad in ["", "v", "1..2", "-1.0.0", "1.2.x", "99999999999999999999.0.0", "١.٢.٣", "🎬"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        // Vorabversion zählt wie die Basisversion: kein „Update“ von 1.0.0-beta auf 1.0.0
        assert_eq!(Version::parse("1.0.0-beta"), Version::parse("1.0.0"));
        assert!(Version(0, 9, 10) < Version(0, 10, 0));
        assert_eq!(Version(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn release_json_garbage_and_old_versions() {
        let cur = Version(1, 0, 0);
        assert!(parse_release("", cur).is_err());
        assert!(parse_release("{\"tag_name\": 5}", cur).is_err());
        assert!(parse_release(&release_json("neueste", true), cur).is_err());
        assert!(parse_release(&release_json("v0.9.9", true), cur).unwrap().is_none());
        let mut j: serde_json::Value = serde_json::from_str(&release_json("v2.0.0", false)).unwrap();
        j["draft"] = true.into();
        assert!(parse_release(&j.to_string(), cur).unwrap().is_none());
        j["draft"] = false.into();
        j["name"] = "".into();
        let r = parse_release(&j.to_string(), cur).unwrap().unwrap();
        assert_eq!(r.name, "v2.0.0", "leerer Name → Tag");
        assert!(r.sha_url.is_none());
    }

    #[test]
    fn sha_file_edge_cases() {
        let h = sha256_hex(b"x");
        assert_eq!(parse_sha_file(&format!("\r\n  {h}\r\n")), Some(h.clone()));
        assert_eq!(parse_sha_file(&h[..63]), None);
        assert_eq!(parse_sha_file(&format!("{h}0")), None);
        assert_eq!(parse_sha_file(&format!("{}g", &h[..63])), None);
        assert_eq!(parse_sha_file(""), None);
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    fn release(base: &str) -> Release {
        Release {
            version: Version(9, 0, 0),
            tag: "v9".into(),
            name: "v9".into(),
            notes: String::new(),
            page: String::new(),
            asset_url: format!("{base}/pkg.zip"),
            asset_size: 5,
            sha_url: Some(format!("{base}/pkg.sha256")),
        }
    }

    #[test]
    fn download_checks_sha_and_writes_nothing_on_failure() {
        let no = AtomicBool::new(false);
        let dir = tempfile::tempdir().unwrap();
        // falsche Prüfsumme
        let base = serve(vec![ok(sha256_hex(b"anders").as_bytes(), None), ok(b"hallo", None)]);
        assert!(download(&release(&base), dir.path(), |_| {}, &no).unwrap_err().contains("Prüfsumme"));
        // ohne Prüfsumme gar nicht erst laden
        let mut r = release(&base);
        r.sha_url = None;
        assert!(download(&r, dir.path(), |_| {}, &no).is_err());
        // abgebrochen
        let base = serve(vec![ok(sha256_hex(b"hallo").as_bytes(), None), ok(b"hallo", None)]);
        assert_eq!(download(&release(&base), dir.path(), |_| {}, &AtomicBool::new(true)).unwrap_err(), "Abgebrochen");
        // kaputte Prüfsummendatei
        let base = serve(vec![ok(b"<html>404</html>", None)]);
        assert!(download(&release(&base), dir.path(), |_| {}, &no).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nichts geschrieben");
        // richtig
        let base = serve(vec![ok(sha256_hex(b"hallo").as_bytes(), None), ok(b"hallo", None)]);
        let p = download(&release(&base), dir.path(), |_| {}, &no).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), b"hallo");
    }

    /// Fehler: eine riesige Content-Length reservierte vorab so viel Speicher (Absturz).
    #[test]
    fn download_survives_absurd_content_length() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(vec![ok(sha256_hex(b"hallo").as_bytes(), None), ok(b"hallo", Some(1 << 62))]);
        let r = download(&release(&base), dir.path(), |_| {}, &AtomicBool::new(false));
        assert!(r.is_err(), "{r:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Fehler: ein abgebrochener Modell-Download ließ eine halbe `.part`-Datei liegen.
    #[test]
    fn truncated_model_download_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("modell.onnx");
        let base = serve(vec![ok(b"hallo", Some(100))]);
        let r = download_to(&format!("{base}/m"), &dest, |_, _| {}, &AtomicBool::new(false));
        assert!(r.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "keine Reste");
        // Lesefehler mitten im Strom (kaputte Chunk-Kodierung)
        let base = serve(vec![b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhallo\r\nZZ\r\n".to_vec()]);
        assert!(download_to(&format!("{base}/m"), &dest, |_, _| {}, &AtomicBool::new(false)).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "keine Reste nach Lesefehler");
        // abgebrochen
        let base = serve(vec![ok(b"hallo", None)]);
        assert!(download_to(&format!("{base}/m"), &dest, |_, _| {}, &AtomicBool::new(true)).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        // Server antwortet 404
        let base = serve(vec![b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()]);
        assert!(download_to(&format!("{base}/m"), &dest, |_, _| {}, &AtomicBool::new(false)).is_err());
        assert!(!dest.exists());
    }

    fn zip_with(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let zp = dir.join("p.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        zp
    }

    #[test]
    fn install_rejects_broken_empty_and_escaping_packages() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("cutline.exe"), b"alt").unwrap();
        let garbage = dir.path().join("kaputt.zip");
        std::fs::write(&garbage, b"PK\x03\x04 kein zip").unwrap();
        assert!(install(&garbage, &app).is_err());
        assert!(install(&dir.path().join("fehlt.zip"), &app).is_err());
        assert!(install(&zip_with(dir.path(), &[]), &app).unwrap_err().contains("leer"));
        for evil in ["a/../../x.exe", "../x.exe"] {
            let zp = zip_with(dir.path(), &[("ok.txt", b"1"), (evil, b"x")]);
            assert!(install(&zp, &app).is_err(), "{evil}");
            assert!(extract_zip(&zp, &dir.path().join("x")).is_err(), "{evil}");
        }
        assert!(!dir.path().join("x.exe").exists() && !dir.path().join("abs.exe").exists());
        assert_eq!(std::fs::read(app.join("cutline.exe")).unwrap(), b"alt", "alte Installation unberührt");
        assert!(!app.join("ok.txt").exists());
        // absolute Pfade landen höchstens im Zielordner, nie daneben
        for abs in ["/abs.exe", "C:/abs.exe", r"\\server\share\abs.exe"] {
            let zp = zip_with(dir.path(), &[(abs, b"x")]);
            let _ = install(&zp, &app);
            let _ = extract_zip(&zp, &dir.path().join("z"));
            assert!(!dir.path().join("abs.exe").exists() && !std::path::Path::new("C:/abs.exe").exists(), "{abs}");
        }
        // Umlaute und Unterordner werden korrekt entpackt
        let zp = zip_with(dir.path(), &[("Schrift/Ä ö 🎬.ttf", b"f")]);
        assert_eq!(extract_zip(&zp, &dir.path().join("y")).unwrap(), 1);
        assert!(dir.path().join("y/Schrift/Ä ö 🎬.ttf").exists());
        // Aufräumen ohne Liste und mit absolutem Pfad in der Liste
        cleanup_old(&dir.path().join("gibtsnicht"));
        let victim = dir.path().join("opfer.txt");
        std::fs::write(&victim, b"x").unwrap();
        std::fs::write(app.join(OLD_LIST), format!("{}\n\n", victim.display())).unwrap();
        cleanup_old(&app);
        assert!(victim.exists());
    }
}
