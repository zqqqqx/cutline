//! Conversion of files saved by versions up to 0.19.3.
//!
//! Until then bundled content had German ids, folders and file names (`cutline.uebergaenge/wuerfel`,
//! `plugins/cutline-elemente/elemente/glocke-gelb.png`). Projects, the library and the settings name
//! such content by id or by path, so they are converted once when a newer version opens them:
//! every string (and object key) that is an old id, ends in one (`pack:<id>`) or is a path into an
//! old pack folder gets its new name. Everything else stays as it is.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Folder of downloaded Epidemic Sound tracks below the data folder (old name: `old("tracks_dir")`)
pub const TRACKS_DIR: &str = "Tracks";
/// Name of the track captions go to; the app finds the track by it (old: `old("caption_track")`)
pub const CAPTION_TRACK: &str = "Captions";
/// Trash folder below the data folder (old name: `old("trash_dir")`)
pub const TRASH_DIR: &str = "Trash";

/// Ids and files of bundled content, old and new (`kind<TAB>old<TAB>new`)
const IDS_TSV: &str = include_str!("../../../translations/legacy/ids.tsv");
/// Other names older versions wrote to disk (`key<TAB>old<TAB>new`)
const NAMES_TSV: &str = include_str!("../../../translations/legacy/names.tsv");

fn rows(src: &'static str, key: &str) -> Vec<(&'static str, &'static str)> {
    src.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut p = l.split('\t');
            (p.next()? == key).then_some(())?;
            Some((p.next()?, p.next()?))
        })
        .collect()
}

/// (old, new) names under `key` in `translations/legacy/names.tsv`
pub fn pairs(key: &str) -> &'static [(&'static str, &'static str)] {
    // parsed once: the log check at startup asks for old markers on every line
    static M: OnceLock<HashMap<&str, Vec<(&str, &str)>>> = OnceLock::new();
    let m = M.get_or_init(|| {
        let mut m: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for l in NAMES_TSV.lines().filter(|l| !l.starts_with('#')) {
            let mut p = l.split('\t');
            if let (Some(k), Some(old), Some(new)) = (p.next(), p.next(), p.next()) {
                m.entry(k).or_default().push((old, new));
            }
        }
        m
    });
    m.get(key).map_or(&[], Vec::as_slice)
}

/// The name versions up to 0.19.3 used for `key` (empty: no such key)
pub fn old(key: &str) -> &'static str {
    pairs(key).first().map_or("", |p| p.0)
}

fn ids() -> &'static HashMap<&'static str, &'static str> {
    static M: OnceLock<HashMap<&str, &str>> = OnceLock::new();
    M.get_or_init(|| rows(IDS_TSV, "id").into_iter().collect())
}

fn files() -> &'static HashMap<&'static str, &'static str> {
    static M: OnceLock<HashMap<&str, &str>> = OnceLock::new();
    M.get_or_init(|| rows(IDS_TSV, "file").into_iter().collect())
}

/// Old pack folders (`cutline-effekte`) with their new names
fn folders() -> &'static Vec<(&'static str, &'static str)> {
    static V: OnceLock<Vec<(&str, &str)>> = OnceLock::new();
    V.get_or_init(|| {
        let mut v: Vec<(&str, &str)> = rows(IDS_TSV, "file").iter().filter_map(|(a, b)| Some((a.split_once('/')?.0, b.split_once('/')?.0))).filter(|(a, b)| a != b).collect();
        v.extend(pairs("pack_folder"));
        v.sort();
        v.dedup();
        v
    })
}

/// New id of bundled content, if `id` is an old one
pub fn id(id: &str) -> Option<&'static str> {
    ids().get(id).copied()
}

/// A path into an old pack folder or the old folder of downloaded tracks, with its new name.
/// The separators of the path stay as they are.
fn path(s: &str) -> Option<String> {
    if !s.contains(['/', '\\']) {
        return None;
    }
    let parts: Vec<&str> = s.split(['/', '\\']).collect();
    let sep = |i: usize| {
        // the separator after part `i`, as written
        let at: usize = parts[..=i].iter().map(|p| p.len() + 1).sum::<usize>() - 1;
        &s[at..at + 1]
    };
    let join = |head: usize, rest: &str| {
        let mut out = String::new();
        for (i, p) in parts[..head].iter().enumerate() {
            out.push_str(p);
            out.push_str(sep(i));
        }
        // the new part uses the separator the path used at this place
        let s0 = sep(head - 1);
        out.push_str(&rest.replace('/', s0));
        out
    };
    for i in 0..parts.len().saturating_sub(1) {
        if parts[i].eq_ignore_ascii_case("plugins") {
            let rel = parts[i + 1..].join("/");
            if let Some(new) = files().get(rel.as_str()) {
                return Some(join(i + 1, new));
            }
            if let Some((_, new)) = folders().iter().find(|(old, _)| *old == parts[i + 1]) {
                return Some(join(i + 1, &format!("{new}/{}", parts[i + 2..].join("/")).trim_end_matches('/').to_string()));
            }
        }
        if parts[i] == "Epidemic Sound" && parts[i + 1] == old("tracks_dir") {
            return Some(join(i + 1, &format!("{TRACKS_DIR}/{}", parts[i + 2..].join("/")).trim_end_matches('/').to_string()));
        }
    }
    None
}

/// One string of an older file with its new value (`None`: nothing to change)
pub fn text(s: &str, extra: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if let Some(new) = id(s) {
        return Some(new.to_string());
    }
    // `pack:<id>`, `sfx:pack:<id>`: the id is what follows the last colon
    if let Some((head, tail)) = s.rsplit_once(':') {
        if let Some(new) = id(tail) {
            return Some(format!("{head}:{new}"));
        }
    }
    path(s).or_else(|| extra(s))
}

/// Convert every string and object key of an older file. `extra` converts what only the caller
/// knows (the library: its category keys). Returns whether anything changed.
pub fn upgrade(v: &mut Value, extra: &dyn Fn(&str) -> Option<String>) -> bool {
    match v {
        Value::String(s) => match text(s, extra) {
            Some(new) => {
                *s = new;
                true
            }
            None => false,
        },
        Value::Array(a) => a.iter_mut().fold(false, |c, x| upgrade(x, extra) | c),
        Value::Object(o) => {
            let mut changed = false;
            let keys: Vec<String> = o.keys().filter(|k| text(k, extra).is_some()).cloned().collect();
            for k in keys {
                if let (Some(new), Some(val)) = (text(&k, extra), o.remove(&k)) {
                    // an entry under the new name already (written by a newer version) wins
                    o.entry(new).or_insert(val);
                    changed = true;
                }
            }
            o.values_mut().fold(changed, |c, x| upgrade(x, extra) | c)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn old_ids_keys_and_paths_get_their_new_names() {
        let mut v = json!({
            "fx": ["cutline.uebergaenge/wuerfel", "cutline.effekte/super8", "fade", "cutline.basis/lomo"],
            "favorites": ["pack:cutline.elemente/glocke-gelb", "sfx:pack:cutline.sounds/knall", "builtin:st:heart"],
            "cutline.sounds/tuer-klopfen": { "secs": 1.0 },
            "media": [
                { "path": "C:\\Programs\\Cutline\\plugins\\cutline-sounds\\audio\\whoosh-lang.ogg" },
                { "path": "/opt/cutline/plugins/cutline-elemente/elemente/glocke-gelb.png" },
                { "path": "D:/app/plugins/cutline-emoji/bewegt/rocket.png" },
                { "path": "C:\\Users\\a\\AppData\\Roaming\\Cutline\\Epidemic Sound\\Vorrat\\t1.mp3" },
                { "path": "C:\\Videos\\wuerfel.mp4" },
            ],
            "text": "Mein Würfel-Video: cutline.effekte ist toll",
            "n": 3,
        });
        assert!(upgrade(&mut v, &none));
        assert_eq!(v["fx"], json!(["cutline.transitions/cube", "cutline.effects/super8", "fade", "cutline.basis/lomo"]));
        assert_eq!(v["favorites"], json!(["pack:cutline.elements/bell-yellow", "sfx:pack:cutline.sounds/bang", "builtin:st:heart"]));
        assert!(v.get("cutline.sounds/tuer-klopfen").is_none() && v["cutline.sounds/door-knock"]["secs"] == 1.0);
        let paths: Vec<&str> = v["media"].as_array().unwrap().iter().map(|m| m["path"].as_str().unwrap()).collect();
        assert_eq!(
            paths,
            [
                "C:\\Programs\\Cutline\\plugins\\cutline-sounds\\audio\\long-whoosh.ogg",
                "/opt/cutline/plugins/cutline-elements/elements/bell-yellow.png",
                "D:/app/plugins/cutline-emoji/animated/rocket.png",
                "C:\\Users\\a\\AppData\\Roaming\\Cutline\\Epidemic Sound\\Tracks\\t1.mp3",
                "C:\\Videos\\wuerfel.mp4",
            ]
        );
        assert_eq!(v["text"], "Mein Würfel-Video: cutline.effekte ist toll", "free text is no id");
        // a second run changes nothing: the new names are no old ones
        let once = v.clone();
        assert!(!upgrade(&mut v, &none));
        assert_eq!(v, once);
    }

    #[test]
    fn every_old_name_leads_to_an_existing_new_one() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let (ids, files) = (rows(IDS_TSV, "id"), rows(IDS_TSV, "file"));
        for (old, new) in &files {
            assert!(root.join(new).is_file(), "{old} -> {new}: file missing");
            assert!(!root.join(old).exists() || old == new, "{old} still there");
        }
        let manifests: String = std::fs::read_dir(&root).unwrap().flatten().filter_map(|d| std::fs::read_to_string(d.path().join("plugin.toml")).ok()).collect();
        let mut seen = std::collections::HashSet::new();
        for (old, new) in &ids {
            assert!(seen.insert(old), "{old} twice");
            assert!(id(new).is_none(), "{new} is an old id too: a second conversion would change it");
            let item = new.rsplit_once('/').map_or(*new, |x| x.1);
            assert!(manifests.contains(&format!("id = \"{item}\"")), "{old} -> {new}: no such id in the bundled packs");
        }
        assert!(ids.len() > 200 && files.len() > 150);
        // every key the code asks for has its old name
        for key in ["tracks_dir", "trash_dir", "caption_track", "archive_media", "archive_fonts", "parts_ext", "source_file", "export_part", "log_session", "log_crash", "log_start", "log_end", "log_panic", "log_sent", "log_graphics"] {
            assert!(!super::old(key).is_empty(), "{key}");
        }
        assert_eq!((pairs("pack_folder").len(), pairs("library_folder").len(), pairs("layout").len()), (3, 3, 4));
    }

    #[test]
    fn the_callers_own_names_are_converted_too() {
        let extra = |s: &str| (s == "b:Bewegung").then(|| "b:Motion".to_string());
        let mut v = json!({ "hidden": ["b:Bewegung", "p:Mein Ordner"], "b:Bewegung": 1 });
        assert!(upgrade(&mut v, &extra));
        assert_eq!(v, json!({ "hidden": ["b:Motion", "p:Mein Ordner"], "b:Motion": 1 }));
    }
}
