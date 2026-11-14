use std::io::Write;
use std::path::{Path, PathBuf};

/// Datei ersetzen ohne Zwischenzustand: erst `*.tmp` schreiben und auf die Platte bringen,
/// dann umbenennen. Ein Absturz mitten im Schreiben lässt so die alte Datei stehen.
/// Die PID im Namen hält zwei gleichzeitig laufende Cutline-Fenster auseinander.
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

/// JSON-Einstellungsdatei lesen. Fehlt sie: Standard. Ist sie kaputt oder unlesbar, wird sie
/// zu `<name>.bad-<Sekunden>` umbenannt, damit das nächste Speichern sie nicht überschreibt.
pub fn read_json_or_quarantine<T: serde::de::DeserializeOwned + Default>(p: &Path) -> T {
    let r = std::fs::read_to_string(p).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()));
    match r {
        Ok(v) => v,
        Err(_) if !p.exists() => T::default(),
        Err(e) => {
            let mut bad = p.as_os_str().to_owned();
            bad.push(format!(".bad-{}", unix_secs()));
            eprintln!("{}: {e}", p.display());
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

/// Dateiname aus einem Projektnamen: nur Buchstaben, Ziffern, Leerzeichen und `-`
pub fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    s.trim().to_string()
}

/// Ordner für erzeugte Dateien, auf die Projekte zeigen (Freistell-Masken, SVG-Bilder).
/// Liegt bewusst nicht im Cache, den „Cache leeren“ löscht.
pub fn generated_dir() -> PathBuf {
    data_dir().join("Generated")
}

/// Unterordner von `cache`, auf die ältere Projekte noch zeigen – Aufräumen lässt sie stehen
pub const CACHE_KEEP: &[&str] = &["matte", "svg"];

/// Inhalt von `dir` löschen, außer den Einträgen in `skip`
pub fn clear_dir(dir: &Path, skip: &[&str]) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if skip.iter().any(|s| e.file_name() == *s) {
            continue;
        }
        let _ = if e.file_type().is_ok_and(|t| t.is_dir()) { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
    }
}

/// Tests, die CUTLINE_HOME setzen (prozessweit), laufen nacheinander
#[cfg(test)]
pub static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

static PROJECTS_DIR: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// Eigener Projektordner aus den Einstellungen (`None` = Standard im Datenordner)
pub fn set_projects_dir(p: Option<PathBuf>) {
    if let Ok(mut d) = PROJECTS_DIR.write() {
        *d = p;
    }
}

/// Ordner, in dem neue Projekte landen und den die Projektübersicht zeigt
pub fn projects_dir() -> PathBuf {
    PROJECTS_DIR
        .read()
        .ok()
        .and_then(|d| d.clone())
        .unwrap_or_else(|| data_dir().join("Projects"))
}

/// Größe eines Ordners samt Unterordnern in Bytes
/// Dateien unter `dir` löschen, die länger als `age` nicht geändert wurden. Gibt die Anzahl zurück.
/// Einträge in `skip` (nur direkt unter `dir`) bleiben; Verknüpfungen auf Ordner werden nicht verfolgt.
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
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(remove_older(d.path(), std::time::Duration::from_millis(10), &[]), 1);
}

#[test]
fn cache_cleanup_keeps_project_files() {
    let d = tempfile::tempdir().unwrap();
    for f in ["matte/a.mkv", "svg/b.png", "proxies/c.mp4", "x.bin"] {
        std::fs::create_dir_all(d.path().join(f).parent().unwrap()).unwrap();
        std::fs::write(d.path().join(f), b"x").unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(remove_older(d.path(), std::time::Duration::from_millis(10), CACHE_KEEP), 2);
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
    // anderes Laufwerk: kopieren, dann löschen
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

/// Alle Projekte (`*.cutline`) und ihre `…-media`-Ordner von `old` nach `new` verschieben.
/// Medienpfade, die in den alten Ordner zeigen, werden angepasst. Ein Fehler hält den Rest nicht
/// auf: zurück kommen die umgezogenen Projekte (alt, neu) und die Fehlermeldungen.
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
    // erst die Medienordner, dann die Projekte (deren Pfade auf die neuen Ordner zeigen).
    // Schon bei einem früheren Versuch umgezogene Ordner zählen mit, sonst blieben ihre Pfade alt.
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
                // nur Pfade in wirklich umgezogenen Medienordnern umschreiben; anderes
                // Material im alten Ordner bleibt, wo es ist
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
            // alte Datei bleibt maßgeblich, die Kopie wieder weg
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
    // in sich selbst verschieben ist verboten
    assert!(!move_projects(&new, &new.join("sub")).1.is_empty());
    // Wiederholung nach Abbruch: Medienordner ist schon drüben, das Projekt noch im alten Ordner
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

/// Mitgelieferte Schriften (OFL/Apache, siehe `assets/fonts/FONTS.md`) für die Textvorlagen
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
];

/// Mitgelieferte Schriften in den Schriftordner legen (fehlende werden ergänzt)
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

/// Benutzerdaten-Ordner: %APPDATA%\Cutline bzw. ~/.local/share/cutline
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CUTLINE_HOME") {
        return PathBuf::from(d);
    }
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

pub fn font_dir() -> PathBuf {
    data_dir().join("Fonts")
}

/// Mitgelieferte Plugins (neben der EXE oder im Repo) + Benutzer-Plugins.
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

/// Ordner für lokale KI-Modelle (Spracherkennung, Freistellen) und ONNX Runtime.
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
    // Systemschriften (Windows) sind nicht mitgeliefert
    let system = ["Georgia", "Impact", "Arial", "Segoe UI", "Courier New", "Times New Roman"];
    for t in cl_fx::text::TEMPLATES {
        if let Some(f) = t.make().font_family {
            assert!(system.contains(&f.as_str()) || fams.contains_key(&f), "{}: Schrift {f} fehlt", t.id);
        }
    }
}
