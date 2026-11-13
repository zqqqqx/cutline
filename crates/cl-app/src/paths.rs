use std::io::Write;
use std::path::{Path, PathBuf};

/// Datei ersetzen ohne Zwischenzustand: erst `*.tmp` schreiben und auf die Platte bringen,
/// dann umbenennen. Ein Absturz mitten im Schreiben lässt so die alte Datei stehen.
pub fn write_atomic(p: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut tmp = p.as_os_str().to_owned();
    tmp.push(".tmp");
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

/// Tests, die `CUTLINE_HOME` setzen, laufen nacheinander.
#[cfg(test)]
pub static TEST_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
