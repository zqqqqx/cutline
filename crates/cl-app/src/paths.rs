use std::path::PathBuf;

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
