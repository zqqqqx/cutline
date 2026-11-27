//! Runtime translation of the UI texts.
//!
//! The UI is written in English; `tr()` looks up every visible text in
//! `translations/<locale>/ui.tsv` (`english<TAB>translation`). Placeholders `{}` stand for
//! arbitrary values (numbers, names) and are carried over in the same order;
//! `{1}`, `{2}` … in the translation allow a different order.
//! Icons from the Private Use Area at the start/end stay unchanged.
//! Two texts that are the same in English but not in another language carry a context after
//! `~` in the code („Track“ the noun, „Track~verb“); the context is never shown.

use crate::Lang;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

const DE_TSV: &str = include_str!("../../../translations/de_DE/ui.tsv");
/// The dictionary of versions up to 0.19.3 (`german<TAB>english`), when the code was German:
/// only for converting what those versions stored or what packs made for them say
const OLD_TSV: &str = include_str!("../../../translations/legacy/ui.de-en.tsv");

/// Key names in shortcuts (English, German), in the order of `translations/de_DE/keys.tsv`
fn keys() -> &'static Vec<(&'static str, &'static str)> {
    static K: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    const TAB: char = 9 as char;
    K.get_or_init(|| include_str!("../../../translations/de_DE/keys.tsv").lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once(TAB)).collect())
}

/// A key name as typed or stored in any interface language, in English („Strg“ → „Ctrl“);
/// other names come back unchanged
pub fn key_english(name: &str) -> String {
    keys().iter().find(|(_, de)| de.eq_ignore_ascii_case(name)).map_or_else(|| name.to_string(), |(en, _)| en.to_string())
}

/// English for a whole German text of the interface up to version 0.19.3
pub fn legacy_english(s: &str) -> Option<&'static str> {
    static D: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    const TAB: char = 9 as char;
    D.get_or_init(|| OLD_TSV.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once(TAB)).filter(|(de, _)| !de.contains("{}")).collect())
        .get(s)
        .copied()
}

/// 0 = English (the language of the code), 1 = German, 2 = pseudo
static LANG: AtomicU8 = AtomicU8::new(0);

/// Set the language for `tr()` (called by `I18n::new`/`set_lang`).
pub fn set_ui_lang(lang: Lang) {
    LANG.store(
        match lang {
            Lang::En => 0,
            Lang::De => 1,
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
        1 => Lang::De,
        2 => Lang::Pseudo,
        _ => Lang::En,
    }
}

struct Dict {
    exact: HashMap<&'static str, &'static str>,
    /// (parts between the placeholders, translated template)
    templates: Vec<(Vec<&'static str>, &'static str)>,
}

fn dict() -> &'static Dict {
    static D: OnceLock<Dict> = OnceLock::new();
    D.get_or_init(|| parse(DE_TSV))
}

fn parse(src: &'static str) -> Dict {
    let mut exact = HashMap::new();
    let mut templates = Vec::new();
    for line in src.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((en, to)) = line.split_once('\t') else {
            continue;
        };
        if en.contains("{}") {
            templates.push((en.split("{}").collect(), to));
        } else {
            exact.insert(en, to);
        }
    }
    // longer (more specific) templates first
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

/// Translates a visible UI text into the selected language.
pub fn tr(s: impl AsRef<str>) -> String {
    let s = s.as_ref();
    match ui_lang() {
        Lang::En => plain(s),
        Lang::De => translate(s),
        Lang::Pseudo => crate::pseudo(&plain(s)),
    }
}

/// A text in a given language, whatever the UI language is (search finds commands by both names)
pub fn tr_in(lang: Lang, s: &str) -> String {
    match lang {
        Lang::De => lookup(s),
        _ => plain(s),
    }
}

/// English text as shown: without the contexts (`Track~verb` → `Track`)
fn plain(s: &str) -> String {
    if !s.contains('~') {
        return s.to_string();
    }
    static CTX: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    let ctx = CTX.get_or_init(|| {
        let mut v: Vec<(&str, &str)> = entries().into_iter().filter_map(|(en, _)| Some((en, en.split_once('~')?.0))).collect();
        // longer first, so a context that starts like another one is not cut apart
        v.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
        v
    });
    ctx.iter().fold(s.to_string(), |s, (k, short)| s.replace(k, short))
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
    // split off icons and whitespace at the edges
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
    if let Some(to) = d.exact.get(core) {
        return format!("{pre}{to}{post}");
    }
    for (parts, to) in &d.templates {
        if let Some(vals) = match_template(core, parts) {
            return format!("{pre}{}{post}", fill(to, &vals));
        }
    }
    // multi-line texts line by line
    if core.contains('\n') {
        let lines: Vec<String> = core.split('\n').map(lookup).collect();
        return format!("{pre}{}{post}", lines.join("\n"));
    }
    // translate "part · part" separately
    if core.contains(" · ") {
        let segs: Vec<String> = core.split(" · ").map(lookup).collect();
        return format!("{pre}{}{post}", segs.join(" · "));
    }
    // "text  (shortcut)": both parts separately
    if let (Some(open), true) = (core.rfind(" ("), core.ends_with(')')) {
        let head = core[..open].trim_end();
        let gap = &core[head.len()..open + 1];
        let inner = &core[open + 2..core.len() - 1];
        if !head.is_empty() {
            return format!("{pre}{}{gap}({}){post}", lookup(head), lookup(inner));
        }
    }
    // keyboard shortcuts
    let keys = keys();
    if keys.iter().take(4).any(|(en, _)| core.contains(en)) || core == "Home" {
        let mut out = core.to_string();
        for (en, de) in keys {
            out = out.replace(en, de);
        }
        return format!("{pre}{out}{post}");
    }
    if cfg!(debug_assertions) {
        report_missing(core);
    }
    plain(s)
}

/// Matches `s` against a template with placeholders; returns the inserted values.
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
    // values contain no further words of the template, but must not be empty
    if vals.iter().any(|v| v.is_empty()) {
        return None;
    }
    Some(vals)
}

fn fill(to: &str, vals: &[&str]) -> String {
    let mut out = String::new();
    let mut next = 0;
    let mut rest = to;
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

/// All pairs (English text in the code, German) of the dictionary.
pub fn entries() -> Vec<(&'static str, &'static str)> {
    DE_TSV
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
        assert_eq!(translate("Cancel"), "Abbrechen");
        assert_eq!(translate("\u{e0f0}  Cancel"), "\u{e0f0}  Abbrechen");
        assert_eq!(fill("{2} of {1}", &["a", "b"]), "b of a");
        assert_eq!(match_template("3 clips deleted", &["", " clips deleted"]), Some(vec!["3"]));
        assert_eq!(translate("unknown text"), "unknown text");
    }

    /// The same English word with two German ones: the context picks the German, and is not shown
    #[test]
    fn contexts_pick_the_german_and_stay_hidden() {
        assert_eq!((translate("Track"), translate("Track~verb")), ("Spur".to_string(), "Verfolgen".to_string()));
        assert_eq!(plain("Track~verb"), "Track");
        assert_eq!(plain("\u{e0f0}  Track~verb (T)"), "\u{e0f0}  Track (T)");
        assert_eq!(plain("None~without · None~m"), "None · None");
        // a tilde that is no context of the dictionary stays
        assert_eq!(plain("a~b ~ Track~other"), "a~b ~ Track~other");
        // every context of the dictionary is hidden, also on a text without a German line
        for (en, _) in entries().into_iter().filter(|(en, _)| en.contains('~')) {
            assert!(!plain(en).contains('~'), "{en}");
            assert!(!translate(&format!("xyzzy {en}")).contains('~'), "{en}");
        }
    }

    #[test]
    fn dictionary_is_clean() {
        let mut seen = std::collections::HashSet::new();
        let mut german = std::collections::HashMap::new();
        for (en, de) in entries() {
            assert!(seen.insert(en), "twice: {en}");
            assert!(!de.trim().is_empty(), "empty: {en}");
            assert_eq!(en.matches("{}").count(), de.matches('{').count(), "placeholders: {en}");
            // one spelling: American (color, gray, canceled …)
            let low = en.to_lowercase();
            for brit in ["colour", "grey", "centre", "cancelled", "favourite", "behaviour", "licence"] {
                assert!(!low.contains(brit), "British „{brit}“: {en}");
            }
            // without its context an English text has one German wording only
            let short = en.split_once('~').map_or(en, |p| p.0);
            if !en.contains('~') {
                assert!(german.insert(short, de).is_none(), "{en}");
            }
        }
    }

    #[test]
    fn singular_counts_translate() {
        assert_eq!(translate("1 track"), "1 Spur");
        assert_eq!(translate("1 highlight added as a marker"), "1 Highlight als Marker gesetzt");
        assert_eq!(translate("1 pause removed (2.5 s)"), "1 Pause entfernt (2.5 s)");
        assert_eq!(translate("3 min ago"), "vor 3 Min.");
        // status line: „Saved “ + an already translated time
        assert_eq!(translate(&format!("\u{e0f0} Saved {}", translate("1 min ago"))), "\u{e0f0} Gespeichert vor 1 Min.");
        assert_eq!(translate(&format!("Saved {}", translate("just now"))), "Gespeichert gerade eben");
        assert_eq!(
            translate("1 item is missing: Würfel – turn on its pack in Settings → Content"),
            "1 Inhalt fehlt: Würfel – Paket in Einstellungen → Inhalte einschalten"
        );
    }

    /// Odd input: no panic, known parts translate, unknown text and the icon/space frame stay.
    #[test]
    fn odd_strings_never_panic() {
        for s in ["", " ", "\u{e0f0}", "\u{e0f0}  ", "✓", "()", " ()", "a ()", "(", ")", " · ", "·", "\n", "🎬", "مرحبا", "Ä (", "x (y)", "{}", "{", "}", "{1}", "%s"] {
            assert_eq!(translate(s), s);
        }
        for (s, want) in [("Cancel\n", "Abbrechen\n"), ("\nCancel", "\nAbbrechen"), ("Cancel · Cancel", "Abbrechen · Abbrechen"), ("Ctrl+Del", "Strg+Entf"), ("Home", "Pos1")] {
            assert_eq!(translate(s), want, "{s:?}");
        }
        // leading/trailing icons and spaces come back unchanged; text without letters stays as it is
        let check = |s: &str| {
            let out = translate(s);
            let edge = |c: char| is_icon(c) || c.is_whitespace();
            let start = s.len() - s.trim_start_matches(edge).len();
            let end = s.trim_end_matches(edge).len().max(start);
            assert!(out.starts_with(&s[..start]) && out.ends_with(&s[end..]), "{s:?} → {out:?}");
            assert!(s.chars().any(char::is_alphabetic) || out == s, "{s:?} → {out:?}");
        };
        // random strings from dictionary parts, multibyte characters and special characters
        let parts: Vec<&str> = entries().iter().take(400).map(|(en, _)| *en).collect();
        let extra = ["🎬", "ä", " ", "  ", "\n", " · ", " (", ")", "{}", "\u{e0f0}", "3", "‰", "\u{202e}"];
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..3000 {
            let mut s = String::new();
            for _ in 0..(x % 5 + 1) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                if x.is_multiple_of(3) {
                    s.push_str(extra[(x / 3 % extra.len() as u64) as usize]);
                } else {
                    let p = parts[(x / 3 % parts.len() as u64) as usize];
                    // also cut in the middle of a word (at character boundaries)
                    let cut = p.char_indices().nth((x % 7) as usize).map(|(i, _)| i).unwrap_or(p.len());
                    s.push_str(if x.is_multiple_of(2) { p } else { &p[..cut] });
                }
            }
            check(&s);
        }
    }

    #[test]
    fn templates_with_multibyte_values() {
        assert_eq!(match_template("🎬 clips deleted", &["", " clips deleted"]), Some(vec!["🎬"]));
        assert_eq!(match_template(" clips deleted", &["", " clips deleted"]), None, "empty value");
        assert_eq!(match_template("x", &["xy", "z"]), None);
        assert_eq!(match_template("aXbYc", &["a", "b", "c"]), Some(vec!["X", "Y"]));
        assert_eq!(match_template("abc", &["a", "", "c"]), None);
        assert_eq!(fill("{3} {} {x} {", &["a"]), "{3} a {x} {");
        assert_eq!(fill("{0}", &["a"]), "a", "0 counts like 1");
        assert_eq!(fill("", &["a"]), "");
        assert_eq!(translate("\u{e0f0}"), "\u{e0f0}");
        assert_eq!(translate("  Cancel  "), "  Abbrechen  ");
        assert_eq!(translate("12:30"), "12:30");
    }
}
