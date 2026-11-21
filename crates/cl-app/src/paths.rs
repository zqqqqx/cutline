use std::io::Write;
use std::path::{Path, PathBuf};

/// Replace a file without an intermediate state: first write `*.tmp` and flush it to disk,
/// then rename. A crash in the middle of writing thus leaves the old file in place.
/// The PID in the name keeps two Cutline windows running at the same time apart.
pub fn write_atomic(p: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut tmp = p.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(data.as_ref())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, p)
}

#[test]
fn write_atomic_replaces_and_leaves_no_tmp() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.cutline");
    std::fs::write(&p, "alt").unwrap();
    write_atomic(&p, "neu").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "neu");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

/// Read a JSON settings file. If missing: default. If broken or unreadable, it is
/// renamed to `<name>.bad-<seconds>` so the next save does not overwrite it.
pub fn read_json_or_quarantine<T: serde::de::DeserializeOwned + Default>(p: &Path) -> T {
    let r = std::fs::read_to_string(p).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()));
    match r {
        Ok(v) => v,
        Err(_) if !p.exists() => T::default(),
        Err(e) => {
            let mut bad = p.as_os_str().to_owned();
            bad.push(format!(".bad-{}", unix_secs()));
            log::warn!("{}: {e}", p.display());
            let _ = std::fs::rename(p, bad);
            T::default()
        }
    }
}

pub fn unix_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[test]
fn broken_json_is_kept_aside() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("settings.json");
    assert!(read_json_or_quarantine::<Vec<u32>>(&p).is_empty());
    std::fs::write(&p, "[1, 2").unwrap();
    assert!(read_json_or_quarantine::<Vec<u32>>(&p).is_empty());
    assert!(!p.exists());
    let bad: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(bad.len() == 1 && bad[0].starts_with("settings.json.bad-"), "{bad:?}");
    std::fs::write(&p, "[1, 2]").unwrap();
    assert_eq!(read_json_or_quarantine::<Vec<u32>>(&p), vec![1, 2]);
}

/// Path for display, consistently with the system's separator (settings and drafts also
/// return `/` on Windows, appended parts `\` – otherwise both would appear mixed).
pub fn shown(p: &Path) -> String {
    let s = p.display().to_string();
    if cfg!(windows) {
        s.replace('/', "\\")
    } else {
        s
    }
}

#[test]
fn shown_paths_use_one_separator() {
    let p = Path::new("F:/projekte").join("K5_proj").join("a.cutline");
    let s = shown(&p);
    if cfg!(windows) {
        assert_eq!(s, r"F:\projekte\K5_proj\a.cutline");
    } else {
        assert_eq!(s, "F:/projekte/K5_proj/a.cutline");
    }
}

/// File name from a project name: only letters, digits, spaces and `-`
/// At most 100 characters (Windows: 255 per name, plus extension and „ (Kopie 2)“), never empty,
/// no reserved device names (`CON`, `NUL` …).
pub fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    match fit_name(&s) {
        s if s.is_empty() => "Projekt".into(),
        s => s,
    }
}

/// Limits of every file name: at most 100 characters, no leading/trailing spaces, no reserved
/// device names (`CON`, `NUL` …). The caller decides the allowed characters.
pub fn fit_name(s: &str) -> String {
    let s: String = s.chars().take(100).collect();
    let s = s.trim();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&s.to_uppercase().as_str())
        || (s.len() == 4 && ["COM", "LPT"].iter().any(|d| s.to_uppercase().starts_with(d)) && s.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        format!("{s}_")
    } else {
        s.into()
    }
}

#[test]
fn safe_name_is_a_writable_file_name() {
    let d = tempfile::tempdir().unwrap();
    for n in ["", "   ", "CON", "nul", "COM1", "lpt9", &"Ä".repeat(400), "a/b\\c:d*e?f\"g<h>i|j", "Ünïcödé 🎬 Name", "..", "."] {
        let s = safe_name(n);
        assert!(!s.is_empty() && s.chars().count() <= 101, "{n:?} → {s:?}");
        let p = d.path().join(format!("{s} (Kopie 12).cutline"));
        std::fs::write(&p, "{}").unwrap_or_else(|e| panic!("{n:?} → {s:?}: {e}"));
        assert_eq!(p.parent(), Some(d.path()));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{}", "{n:?} → {s:?}");
        std::fs::remove_file(&p).unwrap();
    }
    assert_eq!(safe_name("Urlaub 2026"), "Urlaub 2026");
}

/// Is the process with this PID still running? (detect the lock file of a crashed window)
/// shortcut: a PID reassigned in the meantime counts as running (only one warning too many), check the program name if that bothers.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
            fn GetExitCodeProcess(h: isize, code: *mut u32) -> i32;
            fn CloseHandle(h: isize) -> i32;
        }
        const QUERY_LIMITED: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: only queries on our own handle, which is closed afterward
        unsafe {
            let h = OpenProcess(QUERY_LIMITED, 0, pid);
            if h == 0 {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            ok && code == STILL_ACTIVE
        }
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

pub fn lock_path(p: &Path) -> PathBuf {
    let mut l = p.as_os_str().to_owned();
    l.push(".lock");
    PathBuf::from(l)
}

/// May this window write the project file `target`? Not while another running window holds its
/// lock, and never the file it was locked out of when opening (its state would be stale).
pub fn may_write(target: &Path, locked_out: Option<&Path>, me: u32) -> bool {
    locked_out != Some(target) && !lock_held_by_other(&lock_path(target), me)
}

/// Does the lock file belong to another Cutline window that is still running? Leftover
/// locks (crash, hard kill) and locks older than 12 h do not count.
pub fn lock_held_by_other(lock: &Path, me: u32) -> bool {
    let fresh = std::fs::metadata(lock)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|d| d < std::time::Duration::from_secs(12 * 3600));
    fresh
        && std::fs::read_to_string(lock)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .is_some_and(|pid| pid != me && pid_alive(pid))
}

#[test]
fn stale_lock_of_dead_process_is_taken_over() {
    let d = tempfile::tempdir().unwrap();
    let lock = d.path().join("a.cutline.lock");
    let me = std::process::id();
    assert!(!lock_held_by_other(&lock, me), "keine Sperre");
    // another running window: this test process as seen from another PID
    std::fs::write(&lock, me.to_string()).unwrap();
    assert!(lock_held_by_other(&lock, me + 1));
    assert!(!lock_held_by_other(&lock, me), "eigene Sperre");
    // process has ended (hard kill): the lock no longer applies
    let mut c = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/c", "exit"]).spawn().unwrap()
    } else {
        std::process::Command::new("true").spawn().unwrap()
    };
    let dead = c.id();
    c.wait().unwrap();
    assert!(!pid_alive(dead));
    std::fs::write(&lock, dead.to_string()).unwrap();
    assert!(!lock_held_by_other(&lock, me), "liegengebliebene Sperre");
    for junk in ["", "abc", "-5", "99999999999"] {
        std::fs::write(&lock, junk).unwrap();
        assert!(!lock_held_by_other(&lock, me), "{junk:?}");
    }
}

/// Folder for generated files that projects point to (cutout masks, SVG images).
/// Deliberately not in the cache that „Cache leeren“ deletes.
pub fn generated_dir() -> PathBuf {
    data_dir().join("Generated")
}

/// Subfolders of `cache` that older projects still point to – cleanup leaves them in place
pub const CACHE_KEEP: &[&str] = &["matte", "svg"];

/// Delete the contents of `dir`, except the entries in `skip`
pub fn clear_dir(dir: &Path, skip: &[&str]) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if skip.iter().any(|s| e.file_name() == *s) {
            continue;
        }
        let _ = if e.file_type().is_ok_and(|t| t.is_dir()) { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
    }
}

/// Tests that set CUTLINE_HOME (process-wide) run one after another
#[cfg(test)]
pub static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test data folder: holds `HOME_LOCK`, points `CUTLINE_HOME` at a fresh temp folder and on drop
/// (also after a panic) clears the variables first, then deletes the folder, then releases the lock.
#[cfg(test)]
pub struct TestHome {
    dir: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl TestHome {
    pub fn new() -> Self {
        let _lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", dir.path());
        TestHome { dir, _lock }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// Test: set the modification time of a file or folder `secs` into the past (instead of sleeping
/// and hoping the clock has moved on)
#[cfg(test)]
pub fn set_age(p: &Path, secs: u64) {
    let mut o = std::fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES; FILE_FLAG_BACKUP_SEMANTICS opens folders too
        o.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    o.read(true);
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    o.open(p).and_then(|f| f.set_modified(t)).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
}

#[cfg(test)]
impl Drop for TestHome {
    fn drop(&mut self) {
        for v in ["CUTLINE_HOME", "CUTLINE_NO_UPDATE", "CUTLINE_NO_ONBOARDING"] {
            std::env::remove_var(v);
        }
    }
}

static PROJECTS_DIR: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// Own project folder from the settings (`None` = default in the data folder)
pub fn set_projects_dir(p: Option<PathBuf>) {
    if let Ok(mut d) = PROJECTS_DIR.write() {
        *d = p;
    }
}

/// Folder where new projects go and which the project overview shows
pub fn projects_dir() -> PathBuf {
    PROJECTS_DIR
        .read()
        .ok()
        .and_then(|d| d.clone())
        .unwrap_or_else(|| data_dir().join("Projects"))
}

/// Size of a folder including subfolders in bytes
/// Delete files under `dir` not modified for longer than `age`. Returns the count.
/// Entries in `skip` (only directly under `dir`) stay; links to folders are not followed.
pub fn remove_older(dir: &Path, age: std::time::Duration, skip: &[&str]) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        if skip.iter().any(|s| e.file_name() == *s) {
            continue;
        }
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            n += remove_older(&p, age, &[]);
        } else if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d > age)
            && std::fs::remove_file(&p).is_ok()
        {
            n += 1;
        }
    }
    n
}

#[test]
fn remove_older_keeps_fresh_files() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("sub")).unwrap();
    std::fs::write(d.path().join("sub/neu.bin"), b"x").unwrap();
    assert_eq!(remove_older(d.path(), std::time::Duration::from_secs(3600), &[]), 0);
    assert!(d.path().join("sub/neu.bin").is_file());
    set_age(&d.path().join("sub/neu.bin"), 7200);
    assert_eq!(remove_older(d.path(), std::time::Duration::from_secs(3600), &[]), 1);
}

#[test]
fn cache_cleanup_keeps_project_files() {
    let d = tempfile::tempdir().unwrap();
    for f in ["matte/a.mkv", "svg/b.png", "proxies/c.mp4", "x.bin"] {
        std::fs::create_dir_all(d.path().join(f).parent().unwrap()).unwrap();
        std::fs::write(d.path().join(f), b"x").unwrap();
        set_age(&d.path().join(f), 7200);
    }
    assert_eq!(remove_older(d.path(), std::time::Duration::from_secs(3600), CACHE_KEEP), 2);
    std::fs::write(d.path().join("proxies/c.mp4"), b"x").unwrap();
    clear_dir(d.path(), CACHE_KEEP);
    assert!(d.path().join("matte/a.mkv").is_file() && d.path().join("svg/b.png").is_file());
    assert!(!d.path().join("proxies").exists());
}

pub fn dir_size(p: &Path) -> u64 {
    std::fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

pub fn move_path(a: &Path, b: &Path) -> std::io::Result<()> {
    if std::fs::rename(a, b).is_ok() {
        return Ok(());
    }
    // other drive: copy, then delete
    if a.is_dir() {
        std::fs::create_dir_all(b)?;
        for e in std::fs::read_dir(a)? {
            let e = e?;
            move_path(&e.path(), &b.join(e.file_name()))?;
        }
        std::fs::remove_dir(a)
    } else {
        std::fs::copy(a, b)?;
        std::fs::remove_file(a)
    }
}

/// Move all projects (`*.cutline`) and their `…-media` folders from `old` to `new`.
/// Media paths pointing into the old folder are adjusted. An error does not stop the
/// rest: returned are the moved projects (old, new) and the error messages.
pub fn move_projects(old: &Path, new: &Path) -> (Vec<(PathBuf, PathBuf)>, Vec<String>) {
    let mut moved = Vec::new();
    let mut errs = Vec::new();
    if new.starts_with(old) || old.starts_with(new) {
        return (moved, vec![crate::tr("Der neue Ordner darf nicht im alten liegen (oder umgekehrt)")]);
    }
    if let Err(e) = std::fs::create_dir_all(new) {
        return (moved, vec![e.to_string()]);
    }
    let Ok(rd) = std::fs::read_dir(old) else { return (moved, errs) };
    let entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    let is_media = |p: &Path| p.is_dir() && p.to_string_lossy().ends_with("-media");
    // first the media folders, then the projects (whose paths point to the new folders).
    // Folders already moved in an earlier attempt count too, otherwise their paths would stay old.
    let mut moved_dirs: Vec<PathBuf> = std::fs::read_dir(new)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| is_media(&e.path()) && !old.join(e.file_name()).exists())
        .map(|e| old.join(e.file_name()))
        .collect();
    for p in entries.iter().filter(|p| is_media(p)) {
        let to = new.join(p.file_name().unwrap_or_default());
        if to.exists() {
            continue;
        }
        match move_path(p, &to) {
            Ok(()) => moved_dirs.push(p.clone()),
            Err(e) => errs.push(format!("{}: {e}", p.display())),
        }
    }
    for p in entries.iter().filter(|p| p.extension().is_some_and(|x| x == "cutline")) {
        let stem = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let mut to = new.join(format!("{stem}.cutline"));
        let mut k = 2;
        while to.exists() {
            to = new.join(format!("{stem} ({k}).cutline"));
            k += 1;
        }
        let text = match std::fs::read_to_string(p) {
            Ok(t) => t,
            Err(e) => {
                errs.push(format!("{}: {e}", p.display()));
                continue;
            }
        };
        let text = match cl_core::Project::from_json(&text) {
            Ok(mut proj) => {
                // only rewrite paths in media folders that were really moved; other
                // material in the old folder stays where it is
                for m in &mut proj.media {
                    if moved_dirs.iter().any(|d| m.path.starts_with(d)) {
                        if let Ok(rest) = m.path.strip_prefix(old) {
                            m.path = new.join(rest);
                        }
                    }
                }
                proj.to_json()
            }
            Err(_) => text,
        };
        if let Err(e) = write_atomic(&to, text) {
            errs.push(format!("{}: {e}", to.display()));
            continue;
        }
        if let Err(e) = std::fs::remove_file(p) {
            // the old file stays authoritative, the copy goes away again
            let _ = std::fs::remove_file(&to);
            errs.push(format!("{}: {e}", p.display()));
            continue;
        }
        moved.push((p.clone(), to));
    }
    (moved, errs)
}

#[test]
fn moves_projects_and_media_folders() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let new = b.path().join("Meine Projekte");
    let mut proj = cl_core::Project::new("Clip");
    std::fs::create_dir_all(a.path().join("Clip-media")).unwrap();
    std::fs::write(a.path().join("Clip-media").join("x.mp3"), b"x").unwrap();
    let mut m = cl_core::MediaRef {
        id: cl_core::MediaId::new(),
        path: a.path().join("Clip-media").join("x.mp3"),
        kind: cl_core::MediaKind::Audio,
        duration: cl_core::Time::from_secs_f64(1.0),
        width: 0,
        height: 0,
        fps: None,
        has_audio: true,
        audio_streams: 1,
        video_codec: None,
        audio_codec: None,
    };
    proj.media.push(m.clone());
    std::fs::write(a.path().join("Clip.cutline"), proj.to_json()).unwrap();
    std::fs::write(a.path().join("notes.txt"), "bleibt").unwrap();
    let (moved, errs) = move_projects(a.path(), &new);
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(moved.len(), 1);
    assert!(a.path().join("notes.txt").exists());
    assert!(!a.path().join("Clip.cutline").exists());
    let back = cl_core::Project::from_json(&std::fs::read_to_string(new.join("Clip.cutline")).unwrap()).unwrap();
    m.path = new.join("Clip-media").join("x.mp3");
    assert_eq!(back.media[0].path, m.path);
    assert!(m.path.is_file());
    assert_eq!(moved[0].1, new.join("Clip.cutline"));
    // moving into itself is forbidden
    assert!(!move_projects(&new, &new.join("sub")).1.is_empty());
    // Retry after an abort: the media folder is already over there, the project still in the old folder
    let c = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(c.path().join("Neu-media")).unwrap();
    m.path = a.path().join("Neu-media").join("y.mp3");
    std::fs::write(c.path().join("Neu-media").join("y.mp3"), b"y").unwrap();
    proj.media = vec![m.clone()];
    std::fs::write(a.path().join("Neu.cutline"), proj.to_json()).unwrap();
    std::fs::rename(c.path().join("Neu-media"), new.join("Neu-media")).unwrap();
    let (moved, errs) = move_projects(a.path(), &new);
    assert!(errs.is_empty() && moved.len() == 1, "{errs:?}");
    let back = cl_core::Project::from_json(&std::fs::read_to_string(new.join("Neu.cutline")).unwrap()).unwrap();
    assert_eq!(back.media[0].path, new.join("Neu-media").join("y.mp3"));
}

/// Bundled fonts (OFL/Apache, see `assets/fonts/FONTS.md`) for the text templates
const BUNDLED_FONTS: &[(&str, &[u8])] = &[
    ("Anton-Regular.ttf", include_bytes!("../assets/fonts/Anton-Regular.ttf")),
    ("Bangers-Regular.ttf", include_bytes!("../assets/fonts/Bangers-Regular.ttf")),
    ("BebasNeue-Regular.ttf", include_bytes!("../assets/fonts/BebasNeue-Regular.ttf")),
    ("BlackOpsOne-Regular.ttf", include_bytes!("../assets/fonts/BlackOpsOne-Regular.ttf")),
    ("Bungee-Regular.ttf", include_bytes!("../assets/fonts/Bungee-Regular.ttf")),
    ("LilitaOne-Regular.ttf", include_bytes!("../assets/fonts/LilitaOne-Regular.ttf")),
    ("LuckiestGuy-Regular.ttf", include_bytes!("../assets/fonts/LuckiestGuy-Regular.ttf")),
    ("Poppins-ExtraBold.ttf", include_bytes!("../assets/fonts/Poppins-ExtraBold.ttf")),
    ("PressStart2P-Regular.ttf", include_bytes!("../assets/fonts/PressStart2P-Regular.ttf")),
    ("RussoOne-Regular.ttf", include_bytes!("../assets/fonts/RussoOne-Regular.ttf")),
    ("TitanOne-Regular.ttf", include_bytes!("../assets/fonts/TitanOne-Regular.ttf")),
    ("ArchivoBlack-Regular.ttf", include_bytes!("../assets/fonts/ArchivoBlack-Regular.ttf")),
    ("DMSerifDisplay-Regular.ttf", include_bytes!("../assets/fonts/DMSerifDisplay-Regular.ttf")),
    ("FjallaOne-Regular.ttf", include_bytes!("../assets/fonts/FjallaOne-Regular.ttf")),
    ("Pacifico-Regular.ttf", include_bytes!("../assets/fonts/Pacifico-Regular.ttf")),
    ("PermanentMarker-Regular.ttf", include_bytes!("../assets/fonts/PermanentMarker-Regular.ttf")),
    ("Righteous-Regular.ttf", include_bytes!("../assets/fonts/Righteous-Regular.ttf")),
    ("SpaceMono-Regular.ttf", include_bytes!("../assets/fonts/SpaceMono-Regular.ttf")),
    ("VarelaRound-Regular.ttf", include_bytes!("../assets/fonts/VarelaRound-Regular.ttf")),
];

/// Put the bundled fonts into the font folder (missing ones are added)
pub fn install_bundled_fonts() {
    let d = font_dir();
    let _ = std::fs::create_dir_all(&d);
    for (n, b) in BUNDLED_FONTS {
        let p = d.join(n);
        if std::fs::metadata(&p).map(|m| m.len()).ok() != Some(b.len() as u64) {
            let _ = std::fs::write(p, b);
        }
    }
}

/// User data folder: %APPDATA%\Cutline or ~/.local/share/cutline
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CUTLINE_HOME") {
        return PathBuf::from(d);
    }
    // tests without a `TestHome` never touch the real data folder
    #[cfg(test)]
    return std::env::temp_dir().join(format!("cutline-test-{}", std::process::id()));
    #[allow(unreachable_code)]
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            return PathBuf::from(a).join("Cutline");
        }
    }
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h).join(".local/share/cutline");
    }
    PathBuf::from(".cutline")
}

/// Log files (`applog`)
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn font_dir() -> PathBuf {
    data_dir().join("Fonts")
}

/// Bundled plugins (next to the EXE or in the repo) + user plugins.
pub fn plugin_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            v.push(d.join("plugins"));
        }
    }
    v.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins"));
    v.push(data_dir().join("Plugins"));
    v
}

/// Folder for local AI models (speech recognition, cutout) and ONNX Runtime.
pub fn models_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CUTLINE_MODELS") {
        return PathBuf::from(d);
    }
    data_dir().join("Models")
}


#[test]
fn bundled_fonts_cover_all_template_fonts() {
    let d = tempfile::tempdir().unwrap();
    for (n, b) in BUNDLED_FONTS {
        std::fs::write(d.path().join(n), b).unwrap();
    }
    let lib = cl_fonts::FontLibrary::open(d.path()).unwrap();
    let fams = lib.families();
    // System fonts (Windows) are not bundled
    let system = ["Georgia", "Impact", "Arial", "Segoe UI", "Courier New", "Times New Roman"];
    for t in cl_fx::text::TEMPLATES {
        if let Some(f) = t.make().font_family {
            assert!(system.contains(&f.as_str()) || fams.contains_key(&f), "{}: font {f} missing", t.id);
        }
    }
    for p in crate::captions::PRESETS {
        if let Some(f) = (p.style)().font_family {
            assert!(system.contains(&f.as_str()) || fams.contains_key(&f), "{}: font {f} missing", p.id);
        }
    }
}
