//! Laufzeit-Übersetzung der Oberflächentexte.
//!
//! Die Oberfläche ist auf Deutsch geschrieben; `tr()` schlägt jeden sichtbaren Text in
//! `locales/ui.en.tsv` nach (`deutsch<TAB>englisch`). Platzhalter `{}` stehen für
//! beliebige Werte (Zahlen, Namen) und werden in derselben Reihenfolge übernommen;
//! `{1}`, `{2}` … im englischen Text erlauben eine andere Reihenfolge.
//! Icons aus der Private Use Area am Anfang/Ende bleiben unverändert.

use crate::Lang;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

const EN_TSV: &str = include_str!("../locales/ui.en.tsv");

static LANG: AtomicU8 = AtomicU8::new(0);

/// Sprache für `tr()` setzen (wird von `I18n::new`/`set_lang` aufgerufen).
pub fn set_ui_lang(lang: Lang) {
    LANG.store(
        match lang {
            Lang::De => 0,
            Lang::En => 1,
            Lang::Pseudo => 2,
        },
        Ordering::Relaxed,
    );
    if let Some(c) = CACHE.get() {
        c.lock().unwrap().clear();
    }
}

pub fn ui_lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::Pseudo,
        _ => Lang::De,
    }
}

struct Dict {
    exact: HashMap<&'static str, &'static str>,
    /// (Teile zwischen den Platzhaltern, englische Vorlage)
    templates: Vec<(Vec<&'static str>, &'static str)>,
}

fn dict() -> &'static Dict {
    static D: OnceLock<Dict> = OnceLock::new();
    D.get_or_init(|| parse(EN_TSV))
}

fn parse(src: &'static str) -> Dict {
    let mut exact = HashMap::new();
    let mut templates = Vec::new();
    for line in src.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((de, en)) = line.split_once('\t') else {
            continue;
        };
        if de.contains("{}") {
            templates.push((de.split("{}").collect(), en));
        } else {
            exact.insert(de, en);
        }
    }
    // Längere (spezifischere) Vorlagen zuerst
    templates.sort_by_key(|(parts, _): &(Vec<&str>, &str)| {
        std::cmp::Reverse(parts.iter().map(|p| p.len()).sum::<usize>())
    });
    Dict { exact, templates }
}

static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn is_icon(c: char) -> bool {
    matches!(c, '◇' | '◆' | '●' | '○' | '•' | '✓')
        || ('\u{E000}'..='\u{F8FF}').contains(&c)
        || ('\u{F0000}'..='\u{FFFFD}').contains(&c)
}

/// Übersetzt einen sichtbaren Oberflächentext in die eingestellte Sprache.
pub fn tr(s: impl AsRef<str>) -> String {
    let s = s.as_ref();
    match ui_lang() {
        Lang::De => s.to_string(),
        Lang::En => translate(s),
        Lang::Pseudo => crate::pseudo(&translate(s)),
    }
}

fn translate(s: &str) -> String {
    if s.is_empty() || !s.chars().any(|c| c.is_alphabetic()) {
        return s.to_string();
    }
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().unwrap().get(s) {
        return v.clone();
    }
    let out = lookup(s);
    let mut c = cache.lock().unwrap();
    if c.len() > 20_000 {
        c.clear();
    }
    c.insert(s.to_string(), out.clone());
    out
}

fn lookup(s: &str) -> String {
    // Icons und Leerraum am Rand abtrennen
    let start = s
        .char_indices()
        .find(|(_, c)| !is_icon(*c) && !c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let end = s
        .char_indices()
        .rev()
        .find(|(_, c)| !is_icon(*c) && !c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(start);
    if start >= end {
        return s.to_string();
    }
    let (pre, core, post) = (&s[..start], &s[start..end], &s[end..]);
    let d = dict();
    if let Some(en) = d.exact.get(core) {
        return format!("{pre}{en}{post}");
    }
    for (parts, en) in &d.templates {
        if let Some(vals) = match_template(core, parts) {
            return format!("{pre}{}{post}", fill(en, &vals));
        }
    }
    // Mehrzeilige Texte zeilenweise
    if core.contains('\n') {
        let lines: Vec<String> = core.split('\n').map(lookup).collect();
        return format!("{pre}{}{post}", lines.join("\n"));
    }
    // „Teil · Teil“ einzeln übersetzen
    if core.contains(" · ") {
        let segs: Vec<String> = core.split(" · ").map(lookup).collect();
        return format!("{pre}{}{post}", segs.join(" · "));
    }
    // „Text  (Kürzel)“: beide Teile einzeln
    if let (Some(open), true) = (core.rfind(" ("), core.ends_with(')')) {
        let head = core[..open].trim_end();
        let gap = &core[head.len()..open + 1];
        let inner = &core[open + 2..core.len() - 1];
        if !head.is_empty() {
            return format!("{pre}{}{gap}({}){post}", lookup(head), lookup(inner));
        }
    }
    // Tastenkürzel
    const KEYS: [(&str, &str); 8] = [
        ("Strg", "Ctrl"),
        ("Umschalt", "Shift"),
        ("Entf", "Del"),
        ("Leertaste", "Space"),
        ("Pos1", "Home"),
        ("Ende", "End"),
        ("Mausrad", "wheel"),
        ("Klick", "click"),
    ];
    if KEYS[..4].iter().any(|(de, _)| core.contains(de)) || core == "Pos1" {
        let mut out = core.to_string();
        for (de, en) in KEYS {
            out = out.replace(de, en);
        }
        return format!("{pre}{out}{post}");
    }
    if cfg!(debug_assertions) {
        report_missing(core);
    }
    s.to_string()
}

/// Passt `s` auf eine Vorlage mit Platzhaltern; gibt die eingesetzten Werte zurück.
fn match_template<'a>(s: &'a str, parts: &[&str]) -> Option<Vec<&'a str>> {
    let first = parts[0];
    let last = parts[parts.len() - 1];
    if !s.starts_with(first) || s.len() < first.len() + last.len() || !s.ends_with(last) {
        return None;
    }
    let mut vals = Vec::new();
    let mut pos = first.len();
    let stop = s.len() - last.len();
    for (i, p) in parts.iter().enumerate().skip(1) {
        if i == parts.len() - 1 {
            if pos > stop {
                return None;
            }
            vals.push(&s[pos..stop]);
        } else {
            if p.is_empty() {
                return None;
            }
            let rel = s[pos..stop].find(p)?;
            vals.push(&s[pos..pos + rel]);
            pos += rel + p.len();
        }
    }
    // Werte enthalten keine weiteren Wörter der Vorlage, dürfen aber nicht leer sein
    if vals.iter().any(|v| v.is_empty()) {
        return None;
    }
    Some(vals)
}

fn fill(en: &str, vals: &[&str]) -> String {
    let mut out = String::new();
    let mut next = 0;
    let mut rest = en;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        };
        let inner = &after[..j];
        let idx = if inner.is_empty() {
            next += 1;
            Some(next - 1)
        } else {
            inner.parse::<usize>().ok().map(|n| n.saturating_sub(1))
        };
        match idx.and_then(|k| vals.get(k)) {
            Some(v) => out.push_str(&translate(v)),
            None => out.push_str(&rest[i..i + j + 2]),
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

fn report_missing(s: &str) {
    static SEEN: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let Ok(path) = std::env::var("CUTLINE_MISSING_TR") else {
        return;
    };
    let seen = SEEN.get_or_init(Default::default);
    if seen.lock().unwrap().insert(s.to_string()) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{}", s.replace('\n', "\\n"));
        }
    }
}

/// Alle deutschen Schlüssel (für Tests).
pub fn entries() -> Vec<(&'static str, &'static str)> {
    EN_TSV
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_and_icons() {
        assert_eq!(translate("Abbrechen"), "Cancel");
        assert_eq!(translate("\u{e0f0}  Abbrechen"), "\u{e0f0}  Cancel");
        assert_eq!(fill("{2} of {1}", &["a", "b"]), "b of a");
        assert_eq!(
            match_template("3 Clips gelöscht", &["", " Clips gelöscht"]),
            Some(vec!["3"])
        );
        assert_eq!(translate("unbekannter Text"), "unbekannter Text");
    }

    #[test]
    fn dictionary_is_clean() {
        let mut seen = std::collections::HashSet::new();
        for (de, en) in entries() {
            assert!(seen.insert(de), "doppelt: {de}");
            assert!(!en.trim().is_empty(), "leer: {de}");
            assert_eq!(
                de.matches("{}").count(),
                en.matches('{').count(),
                "Platzhalter: {de}"
            );
        }
    }
}
