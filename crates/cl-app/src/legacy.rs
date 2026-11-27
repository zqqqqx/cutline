//! Data written by versions up to 0.19.3, when the code and the bundled content were German.
//!
//! - folders below the data folder got English names: they are renamed once at the first start
//! - the library, the settings and the looks name bundled content by id: converted when read
//!   ([`cl_core::legacy`]), the library also its built-in categories and folders (German words then)
//! - packs from other sources may still name their categories in German: shown through the old
//!   dictionary

use std::path::Path;

use cl_core::legacy::{old, pairs};

/// English text for a German one of the old interface (`None`: not a text of the old interface)
pub fn english(s: &str) -> Option<&'static str> {
    cl_i18n::legacy_english(s).filter(|en| *en != s)
}

/// A category as a pack names it: packs made for older versions use the German word
pub fn category(s: &str) -> String {
    english(s).map_or_else(|| s.to_string(), String::from)
}

/// Keys only the library has: built-in categories (`b:Bewegung`) and the special folders
pub fn library_key(s: &str) -> Option<String> {
    if let Some(cat) = s.strip_prefix("b:") {
        return english(cat).map(|en| format!("b:{en}"));
    }
    // special folders of the library
    pairs("library_folder").into_iter().find(|(old, _)| *old == s).map(|(_, new)| new.to_string())
}

/// Name of a built-in window arrangement as older settings stored it
pub fn layout_name(s: &str) -> Option<&'static str> {
    pairs("layout").into_iter().find(|(old, _)| *old == s).map(|x| x.1)
}

/// Rename a folder of an older version unless the new one is there already
fn rename(dir: &Path, (old, new): (&str, &str)) {
    let (from, to) = (dir.join(old), dir.join(new));
    if from.is_dir() && !to.exists() {
        match std::fs::rename(&from, &to) {
            Ok(()) => log::info!(target: "session", "folder of an older version renamed: {old} -> {new}"),
            Err(e) => log::warn!(target: "session", "folder {old} not renamed to {new}: {e}"),
        }
    }
}

/// Once at the start, before anything reads the data folder: folders of older versions get their
/// new names. Projects that point into them are converted when they are opened.
pub fn rename_data_folders(data: &Path) {
    rename(data, (old("trash_dir"), cl_core::legacy::TRASH_DIR));
    rename(&data.join("Epidemic Sound"), (old("tracks_dir"), cl_core::legacy::TRACKS_DIR));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_folders_move_once_and_never_over_new_ones() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path();
        std::fs::create_dir_all(data.join("Papierkorb/Library")).unwrap();
        std::fs::write(data.join("Papierkorb/Library/a.wav"), b"a").unwrap();
        std::fs::create_dir_all(data.join("Epidemic Sound/Vorrat")).unwrap();
        std::fs::write(data.join("Epidemic Sound/Vorrat/t1.mp3"), b"t").unwrap();
        rename_data_folders(data);
        assert!(data.join("Trash/Library/a.wav").is_file() && !data.join("Papierkorb").exists());
        assert!(data.join("Epidemic Sound/Tracks/t1.mp3").is_file() && !data.join("Epidemic Sound/Vorrat").exists());
        // both there (an older version ran again): nothing is overwritten or merged
        std::fs::create_dir_all(data.join("Papierkorb")).unwrap();
        std::fs::write(data.join("Papierkorb/b.wav"), b"b").unwrap();
        rename_data_folders(data);
        assert!(data.join("Papierkorb/b.wav").is_file() && data.join("Trash/Library/a.wav").is_file());
        // nothing there: nothing happens
        rename_data_folders(&data.join("empty"));
    }

    #[test]
    fn library_keys_and_layout_names_of_older_versions() {
        assert_eq!(library_key("b:Bewegung").as_deref(), Some("b:Motion"));
        assert_eq!(library_key("b:Oberfläche").as_deref(), Some("b:Interface"));
        assert_eq!(library_key("Favoriten").as_deref(), Some("Favorites"));
        // own folders, new keys and other strings stay
        for s in ["p:Bewegung", "v:Alle", "b:Motion", "b:Mein Ordner", "All", "Bewegung", "pack:cutline.sounds/bang"] {
            assert_eq!(library_key(s), None, "{s}");
        }
        assert_eq!(layout_name("Getauscht"), Some("Swapped"));
        assert_eq!(layout_name("Mein Layout"), None);
        assert_eq!(category("Unschärfe"), "Blur");
        assert_eq!(category("Blur"), "Blur");
        assert_eq!(category("Eigene Kategorie XY"), "Eigene Kategorie XY");
    }
}
