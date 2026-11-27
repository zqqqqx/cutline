//! Automatic updates via GitHub Releases.
//!
//! Flow:
//! 1. [`check`] queries `releases/latest` of the repository and compares the version
//!    (SemVer, `v` prefix allowed) with the running one.
//! 2. [`download`] downloads the matching package (`cutline-<os>-<arch>.zip`) and checks it against
//!    the `.sha256` file published with it. Without a checksum nothing is installed.
//! 3. [`install`] unpacks into the program folder. Running files (the EXE itself) are
//!    renamed to `*.old` first, which Windows allows even for running programs.
//!    On the next start [`cleanup_old`] removes these leftovers.
//!
//! No data is transmitted except the normal HTTP request to api.github.com.

pub mod changelog;
pub mod packs;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Short, understandable message for a raw error text (network library, file system, check).
/// The raw text belongs in the log, not in the UI; „Canceled“ stays unchanged.
pub fn friendly(raw: &str) -> String {
    let l = raw.to_lowercase();
    let has = |k: &[&str]| k.iter().any(|k| l.contains(k));
    // the operating system writes its own messages in its language
    let os = |meaning: &str| {
        const TAB: char = 9 as char;
        include_str!("../../../translations/de_DE/os-errors.tsv").lines().filter_map(|x| x.split_once(TAB)).any(|(word, m)| m == meaning && l.contains(word))
    };
    let m = if l == "canceled" {
        raw
    } else if has(&["no space", "os error 112", "os error 28", "disk full"]) || os("no_space") {
        "Not enough disk space"
    } else if has(&["without checksum"]) {
        "Version without checksum"
    } else if has(&["checksum", "damaged", "incomplete", "too large", "is empty", "unsafe"]) {
        // before "404": the hex characters in the checksum error can contain "404"
        "File damaged (checksum)"
    } else if has(&["404", "not found"]) || os("not_found") {
        "Not found"
    } else if has(&["dns", "connection", "can't reach", "timed out", "timeout", "network", "tls", "os error 100", "os error 110"]) {
        "No connection"
    } else if has(&["responds with", "status", "not readable"]) {
        "Server unreachable"
    } else {
        "Unexpected error"
    };
    m.to_string()
}

/// GitHub repository with the releases.
pub const REPO: &str = "zqqqqx/cutline";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `v1.2.3`, `1.2.3`, `1.2.3-beta.1` (pre-releases count like the base version).
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
    /// Asset `changes.json` (older releases don't have it)
    pub changes_url: Option<String>,
    /// „What's new“ from `changes.json`; `None` = read from the release text (`notes`)
    pub changes: Option<Vec<changelog::Notes>>,
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

/// Name of the package for this system.
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

/// Evaluate the release JSON (own function so it is testable without network).
pub fn parse_release(json: &str, current: Version) -> Result<Option<Release>, String> {
    let r: GhRelease =
        serde_json::from_str(json).map_err(|e| format!("Response not readable: {e}"))?;
    if r.draft || r.prerelease {
        return Ok(None);
    }
    let Some(version) = Version::parse(&r.tag_name) else {
        return Err(format!("Unknown version format: {}", r.tag_name));
    };
    if version <= current {
        return Ok(None);
    }
    let want = asset_name();
    let Some(asset) = r.assets.iter().find(|a| a.name == want) else {
        return Err(format!(
            "Version {version} has no package for this system ({want})"
        ));
    };
    // never installed without a checksum: then don't offer it at all
    let Some(sha) = r.assets.iter().find(|a| a.name == format!("{want}.sha256")) else {
        return Err(format!("Version {version} without checksum"));
    };
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
        sha_url: Some(sha.browser_download_url.clone()),
        changes_url: r.assets.iter().find(|a| a.name == "changes.json").map(|a| a.browser_download_url.clone()),
        changes: None,
    }))
}

/// Maximum size of `changes.json` (all versions of both languages: about 60 KB today)
const MAX_CHANGES: u64 = 2 << 20;

/// Load `changes.json`; `None` = unreachable, too large or unreadable (then the release text applies).
pub fn fetch_changes(url: &str) -> Option<Vec<changelog::Notes>> {
    let resp = agent().get(url).set("User-Agent", "cutline-updater").call().ok()?;
    let mut text = String::new();
    resp.into_reader().take(MAX_CHANGES + 1).read_to_string(&mut text).ok()?;
    if text.len() as u64 > MAX_CHANGES {
        return None;
    }
    changelog::parse_json(&text)
}

/// Query the newest version. `Ok(None)` = already up to date.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let cur = Version::parse(current).ok_or("Own version not readable")?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    match agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "cutline-updater")
        .call()
    {
        Ok(resp) => {
            let body = resp.into_string().map_err(|e| e.to_string())?;
            let mut r = parse_release(&body, cur)?;
            if let Some(rel) = &mut r {
                rel.changes = rel.changes_url.as_deref().and_then(fetch_changes);
            }
            Ok(r)
        }
        Err(ureq::Error::Status(404, _)) => Err("No public releases found yet".into()),
        Err(ureq::Error::Status(code, _)) => Err(format!("GitHub responds with {code}")),
        Err(e) => Err(format!("Can't reach GitHub ({e})")),
    }
}

/// SHA-256 as hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let d = Sha256::digest(data);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Expected checksum from a `.sha256` file (`<hex>  <file>` or just `<hex>`).
pub fn parse_sha_file(s: &str) -> Option<String> {
    let h = s.split_whitespace().next()?.to_ascii_lowercase();
    (h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// Largest update package accepted (today about 150 MB): it is held in memory until checked
const MAX_PACKAGE: u64 = 1 << 30;

/// Download and check the package. Returns the path of the checked ZIP file.
pub fn download(rel: &Release, dir: &Path, progress: impl FnMut(u64, u64), cancel: &AtomicBool) -> Result<PathBuf, String> {
    download_max(rel, dir, progress, cancel, MAX_PACKAGE)
}

fn download_max(
    rel: &Release,
    dir: &Path,
    mut progress: impl FnMut(u64, u64),
    cancel: &AtomicBool,
    max: u64,
) -> Result<PathBuf, String> {
    let sha_url = rel
        .sha_url
        .as_ref()
        .ok_or("Release without checksum – installation canceled for safety")?;
    let expected = agent()
        .get(sha_url)
        .set("User-Agent", "cutline-updater")
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let expected = parse_sha_file(&expected).ok_or("Checksum file not readable")?;
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
    // Content-Length comes from the server: don't reserve blindly up front (crash with huge values)
    let mut data = Vec::with_capacity(total.min(256 << 20) as usize);
    let mut buf = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Canceled".into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if data.len() as u64 > max {
            return Err("Package is too large".into());
        }
        progress(data.len() as u64, total);
    }
    let got = sha256_hex(&data);
    if got != expected {
        return Err(format!(
            "Checksum does not match (expected {}…, got {}…)",
            &expected[..12],
            &got[..12]
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let out = dir.join(asset_name());
    std::fs::write(&out, &data).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Unpack the package into `target`. Existing files are renamed to `*.old`
/// (works for the running EXE too). Returns the number of files.
pub fn install(zip_path: &Path, target: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("Package damaged: {e}"))?;
    let mut left = MAX_UNPACKED;
    // first check everything and unpack into an intermediate folder, then swap:
    // so on an error the old installation stays untouched.
    let staging = target.join(".update-staging");
    let _ = std::fs::remove_dir_all(&staging);
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("Unsafe path in the pack: {}", entry.name().unwrap_or_default()));
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
        copy_capped(&mut entry, &mut out, &mut left)?;
        out.flush().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode));
        }
        files.push(rel);
    }
    if files.is_empty() {
        return Err("Package is empty".into());
    }
    // list of renamed files: cleanup_old deletes only these, never foreign `*.old`.
    let mut list = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(target.join(OLD_LIST))
        .map_err(|e| e.to_string())?;
    // swapped files (target, was there an old version?): roll back on an error
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

/// Download a file (for models). Writes `*.part` first and renames at the end.
/// If the URL ends in `#sha256=<hex>`, the file is checked against this checksum first.
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
                return Err("Canceled".into());
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
    // abort, read or write error: don't leave a half `.part` file behind
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if total > 0 && done != total {
        let _ = std::fs::remove_file(&part);
        return Err("Download incomplete".into());
    }
    if let Some(want) = want {
        let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if !got.eq_ignore_ascii_case(want) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{url}: checksum does not match"));
        }
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

/// `url#sha256=<hex>` → (url, Some(hex)); otherwise (url, None).
fn split_sha(url: &str) -> (&str, Option<&str>) {
    match url.split_once("#sha256=") {
        Some((u, h)) => (u, Some(h)),
        None => (url, None),
    }
}

/// At most this much is unpacked from an archive (update, runtimes for models).
const MAX_UNPACKED: u64 = 4 << 30;

/// Copy an entry without writing more than `left` bytes: the size fields in the ZIP are
/// not trustworthy (packed "bombs" would otherwise fill the disk).
fn copy_capped(r: impl Read, w: &mut impl Write, left: &mut u64) -> Result<(), String> {
    let n = std::io::copy(&mut r.take(*left + 1), w).map_err(|e| e.to_string())?;
    if n > *left {
        return Err("Archive is too large when unpacked".into());
    }
    *left -= n;
    Ok(())
}

/// Unpack a ZIP archive into a folder (unsafe paths are rejected).
pub fn extract_zip(zip_path: &Path, dir: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    let mut n = 0;
    let mut left = MAX_UNPACKED;
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = e.enclosed_name() else {
            return Err(format!("Unsafe path: {}", e.name().unwrap_or_default()));
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
        copy_capped(&mut e, &mut out, &mut left)?;
        n += 1;
    }
    Ok(n)
}

/// File in the program folder with the `*.old` files renamed by [`install`].
const OLD_LIST: &str = ".update-old.txt";

/// Remove leftovers of an earlier update: only the files listed in [`OLD_LIST`].
pub fn cleanup_old(dir: &Path) {
    if let Ok(list) = std::fs::read_to_string(dir.join(OLD_LIST)) {
        for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let rel = Path::new(line);
            // only relative paths without `..` – delete nothing outside the program folder.
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
    #[test]
    fn raw_errors_become_short_messages() {
        let f = super::friendly;
        assert_eq!(f("Can't reach GitHub (https://x/a.zip: Dns Failed: failed to lookup)"), "No connection");
        assert_eq!(f("https://x/p.zip: Connection Failed: refused"), "No connection");
        assert_eq!(f("GitHub responds with 503"), "Server unreachable");
        assert_eq!(f("https://x/p.zip: checksum does not match"), "File damaged (checksum)");
        assert_eq!(f("Checksum does not match (expected ab…, got cd…)"), "File damaged (checksum)");
        assert_eq!(f("Es ist nicht genug Speicherplatz vorhanden. (os error 112)"), "Not enough disk space");
        assert_eq!(f("GitHub responds with 404"), "Not found");
        assert_eq!(f("Canceled"), "Canceled");
        // hex characters with "404" in the checksum error: stays a checksum error
        assert_eq!(f("Checksum does not match (expected 4041ab…, got cd…)"), "File damaged (checksum)");
        assert_eq!(f("Release without checksum – installation canceled for safety"), "Version without checksum");
        assert_eq!(f("irgendwas"), "Unexpected error");
    }

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
        serde_json::json!({ "tag_name": tag, "name": "Cutline", "body": "New: everything", "html_url": "https://github.com/x", "assets": assets }).to_string()
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
        assert_eq!(r.notes, "New: everything");
        let mut j: serde_json::Value = serde_json::from_str(&release_json("v0.3.0", true)).unwrap();
        j["prerelease"] = true.into();
        assert!(
            parse_release(&j.to_string(), cur).unwrap().is_none(),
            "pre-releases are ignored"
        );
        j["prerelease"] = false.into();
        j["assets"] = serde_json::json!([]);
        assert!(
            parse_release(&j.to_string(), cur).is_err(),
            "no package for this system"
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
        assert_eq!(parse_sha_file("broken"), None);
    }

    #[test]
    fn download_verifies_sha_from_url() {
        // small local HTTP server, always delivers "hello"
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in l.incoming().take(2) {
                let mut c = c.unwrap();
                let mut b = [0u8; 1024];
                let _ = c.read(&mut b);
                let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let url = format!("http://127.0.0.1:{port}/f");
        let good = format!("{url}#sha256={}", sha256_hex(b"hello"));
        let bad = format!("{url}#sha256={}", sha256_hex(b"different"));
        let no = AtomicBool::new(false);
        assert!(download_to(&bad, &dir.path().join("b"), |_, _| {}, &no).is_err());
        assert!(!dir.path().join("b").exists());
        download_to(&good, &dir.path().join("g"), |_, _| {}, &no).unwrap();
        assert_eq!(std::fs::read(dir.path().join("g")).unwrap(), b"hello");
    }

    #[test]
    fn install_replaces_files_and_keeps_old_until_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        std::fs::create_dir_all(app.join("plugins/basis")).unwrap();
        std::fs::write(app.join("cutline.exe"), b"old").unwrap();
        std::fs::write(app.join("plugins/basis/plugin.toml"), b"old").unwrap();
        // build the package
        let zp = dir.path().join("pkg.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("cutline.exe", o).unwrap();
            w.write_all(b"new").unwrap();
            w.start_file("plugins/basis/plugin.toml", o).unwrap();
            w.write_all(b"new").unwrap();
            w.start_file("ffmpeg/ffmpeg.exe", o).unwrap();
            w.write_all(b"ff").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(install(&zp, &app).unwrap(), 3);
        assert_eq!(std::fs::read(app.join("cutline.exe")).unwrap(), b"new");
        assert_eq!(std::fs::read(app.join("cutline.exe.old")).unwrap(), b"old");
        assert_eq!(std::fs::read(app.join("ffmpeg/ffmpeg.exe")).unwrap(), b"ff");
        assert!(!app.join(".update-staging").exists());
        std::fs::write(app.join("other.old"), b"x").unwrap();
        std::fs::write(dir.path().join("x.old"), b"x").unwrap();
        // manipulated list: a path outside the folder is ignored.
        let mut l = std::fs::OpenOptions::new().append(true).open(app.join(OLD_LIST)).unwrap();
        writeln!(l, "../x.old").unwrap();
        drop(l);
        cleanup_old(&app);
        assert!(app.join("other.old").exists());
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
        std::fs::write(app.join("cutline.exe"), b"old").unwrap();
        // "b" is a file: the folder for b/c.txt cannot be created
        std::fs::write(app.join("b"), b"x").unwrap();
        let zp = dir.path().join("pkg.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            for (name, data) in [("cutline.exe", b"new"), ("new.dll", b"new"), ("b/c.txt", b"new")] {
                w.start_file(name, o).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        assert!(install(&zp, &app).is_err());
        assert_eq!(std::fs::read(app.join("cutline.exe")).unwrap(), b"old");
        assert!(!app.join("cutline.exe.old").exists());
        assert!(!app.join("new.dll").exists());
    }

    #[test]
    fn unpacking_stops_at_the_size_cap() {
        let dir = tempfile::tempdir().unwrap();
        let zp = dir.path().join("bombe.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("null.bin", o).unwrap();
            w.write_all(&vec![0u8; 1 << 20]).unwrap();
            w.finish().unwrap();
        }
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&zp).unwrap()).unwrap();
        let mut out = Vec::new();
        let mut left = 1000;
        assert!(copy_capped(&mut zip.by_index(0).unwrap(), &mut out, &mut left).is_err());
        assert!(out.len() <= 1001, "does not write past the limit");
        let (mut out, mut left) = (Vec::new(), 2 << 20);
        copy_capped(&mut zip.by_index(0).unwrap(), &mut out, &mut left).unwrap();
        assert_eq!((out.len(), left), (1 << 20, 1 << 20));
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

    /// Small HTTP server: answers one connection after another with the raw response.
    /// Each request line and the answer it got go to stderr, so a failing test shows when the
    /// code asked in a different order than the canned answers expect.
    fn serve(answers: Vec<Vec<u8>>) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (i, a) in answers.into_iter().enumerate() {
                let Ok((mut c, _)) = l.accept() else { return };
                let mut b = [0u8; 2048];
                let n = c.read(&mut b).unwrap_or(0);
                let req = String::from_utf8_lossy(&b[..n]);
                let body = a.windows(4).position(|w| w == b"\r\n\r\n").map_or(&a[..], |p| &a[p + 4..]);
                let body = String::from_utf8_lossy(&body[..body.len().min(30)]);
                eprintln!("serve: request {i} {:?} gets body {body:?}", req.lines().next().unwrap_or(""));
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
        // a pre-release counts like the base version: no "update" from 1.0.0-beta to 1.0.0
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
        let e = parse_release(&j.to_string(), cur).unwrap_err();
        assert_eq!(friendly(&e), "Version without checksum", "{e}");
        let r = parse_release(&release_json("v2.0.0", true).replace("\"Cutline\"", "\"\""), cur).unwrap().unwrap();
        assert_eq!(r.name, "v2.0.0", "empty name → tag");
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
            changes_url: None,
            changes: None,
        }
    }

    #[test]
    fn changes_json_is_found_and_fetched() {
        let mut j: serde_json::Value = serde_json::from_str(&release_json("v0.3.0", true)).unwrap();
        assert_eq!(parse_release(&j.to_string(), Version(0, 2, 0)).unwrap().unwrap().changes_url, None, "older release");
        j["assets"].as_array_mut().unwrap().push(serde_json::json!({ "name": "changes.json", "browser_download_url": "https://x/changes.json" }));
        let r = parse_release(&j.to_string(), Version(0, 2, 0)).unwrap().unwrap();
        assert_eq!(r.changes_url.as_deref(), Some("https://x/changes.json"));
        let good = br#"{"format": 1, "versions": [{"version": "0.3.0", "en": [{"group": "New", "items": [{"text": "x"}]}]}]}"#;
        let big = format!(r#"{{"format": 1, "versions": [], "x": "{}"}}"#, "a".repeat(MAX_CHANGES as usize));
        let base = serve(vec![
            ok(good, None),
            ok(br#"{"format": 9, "versions": []}"#, None),
            ok(big.as_bytes(), None),
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        ]);
        let got = fetch_changes(&format!("{base}/c")).expect("read");
        assert_eq!(got[0].version, Some(Version(0, 3, 0)));
        assert!(fetch_changes(&format!("{base}/c")).is_none(), "unbekanntes Format");
        assert!(fetch_changes(&format!("{base}/c")).is_none(), "too large");
        assert!(fetch_changes(&format!("{base}/c")).is_none(), "missing");
    }

    #[test]
    fn download_checks_sha_and_writes_nothing_on_failure() {
        let no = AtomicBool::new(false);
        let dir = tempfile::tempdir().unwrap();
        // wrong checksum
        let base = serve(vec![ok(sha256_hex(b"different").as_bytes(), None), ok(b"hello", None)]);
        assert!(download(&release(&base), dir.path(), |_, _| {}, &no).unwrap_err().contains("hecksum"));
        // without checksum don't even download
        let mut r = release(&base);
        r.sha_url = None;
        assert!(download(&r, dir.path(), |_, _| {}, &no).is_err());
        // cancelled
        let base = serve(vec![ok(sha256_hex(b"hello").as_bytes(), None), ok(b"hello", None)]);
        assert_eq!(download(&release(&base), dir.path(), |_, _| {}, &AtomicBool::new(true)).unwrap_err(), "Canceled");
        // broken checksum file
        let base = serve(vec![ok(b"<html>404</html>", None)]);
        assert!(download(&release(&base), dir.path(), |_, _| {}, &no).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing written");
        // correct
        let base = serve(vec![ok(sha256_hex(b"hello").as_bytes(), None), ok(b"hello", None)]);
        let p = download(&release(&base), dir.path(), |_, _| {}, &no).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), b"hello");
    }

    /// Bug (audit of 0.19.0): the package was read into memory without a limit.
    #[test]
    fn download_stops_at_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);
        let base = serve(vec![ok(sha256_hex(b"hello").as_bytes(), None), ok(b"hello", None)]);
        assert_eq!(download_max(&release(&base), dir.path(), |_, _| {}, &no, 4).unwrap_err(), "Package is too large");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        let base = serve(vec![ok(sha256_hex(b"hello").as_bytes(), None), ok(b"hello", None)]);
        assert!(download_max(&release(&base), dir.path(), |_, _| {}, &no, 5).is_ok());
    }

    /// Bug: a huge Content-Length reserved that much memory up front (crash).
    #[test]
    fn download_survives_absurd_content_length() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(vec![ok(sha256_hex(b"hello").as_bytes(), None), ok(b"hello", Some(1 << 62))]);
        let r = download(&release(&base), dir.path(), |_, _| {}, &AtomicBool::new(false));
        assert!(r.is_err(), "{r:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Bug: a cancelled model download left a half `.part` file behind.
    #[test]
    fn truncated_model_download_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("modell.onnx");
        let base = serve(vec![ok(b"hello", Some(100))]);
        let r = download_to(&format!("{base}/m"), &dest, |_, _| {}, &AtomicBool::new(false));
        assert!(r.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "no leftovers");
        // read error in the middle of the stream (broken chunk encoding)
        let base = serve(vec![b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhallo\r\nZZ\r\n".to_vec()]);
        assert!(download_to(&format!("{base}/m"), &dest, |_, _| {}, &AtomicBool::new(false)).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "no leftovers after read error");
        // cancelled
        let base = serve(vec![ok(b"hello", None)]);
        assert!(download_to(&format!("{base}/m"), &dest, |_, _| {}, &AtomicBool::new(true)).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        // server answers 404
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
        std::fs::write(app.join("cutline.exe"), b"old").unwrap();
        let garbage = dir.path().join("broken.zip");
        std::fs::write(&garbage, b"PK\x03\x04 no zip").unwrap();
        assert!(install(&garbage, &app).is_err());
        assert!(install(&dir.path().join("missing.zip"), &app).is_err());
        assert!(install(&zip_with(dir.path(), &[]), &app).unwrap_err().contains("empty"));
        for evil in ["a/../../x.exe", "../x.exe"] {
            let zp = zip_with(dir.path(), &[("ok.txt", b"1"), (evil, b"x")]);
            assert!(install(&zp, &app).is_err(), "{evil}");
            assert!(extract_zip(&zp, &dir.path().join("x")).is_err(), "{evil}");
        }
        assert!(!dir.path().join("x.exe").exists() && !dir.path().join("abs.exe").exists());
        assert_eq!(std::fs::read(app.join("cutline.exe")).unwrap(), b"old", "old installation untouched");
        assert!(!app.join("ok.txt").exists());
        // absolute paths land at most in the target folder, never next to it
        for abs in ["/abs.exe", "C:/abs.exe", r"\\server\share\abs.exe"] {
            let zp = zip_with(dir.path(), &[(abs, b"x")]);
            let _ = install(&zp, &app);
            let _ = extract_zip(&zp, &dir.path().join("z"));
            assert!(!dir.path().join("abs.exe").exists() && !std::path::Path::new("C:/abs.exe").exists(), "{abs}");
        }
        // umlauts and subfolders are unpacked correctly
        let zp = zip_with(dir.path(), &[("Font/Ä ö 🎬.ttf", b"f")]);
        assert_eq!(extract_zip(&zp, &dir.path().join("y")).unwrap(), 1);
        assert!(dir.path().join("y/Font/Ä ö 🎬.ttf").exists());
        // cleanup without a list and with an absolute path in the list
        cleanup_old(&dir.path().join("gibtsnicht"));
        let victim = dir.path().join("opfer.txt");
        std::fs::write(&victim, b"x").unwrap();
        std::fs::write(app.join(OLD_LIST), format!("{}\n\n", victim.display())).unwrap();
        cleanup_old(&app);
        assert!(victim.exists());
    }
}
