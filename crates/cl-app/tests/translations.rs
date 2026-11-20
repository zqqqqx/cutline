//! Every fixed UI text in the code has an English translation.
//!
//! Covered are text literals directly in `tr("…")`, `toast("…")`, `toast_err("…")` and the button/
//! section helpers from `theme`/`widgets` (which all translate themselves; list below). A text
//! counts as translated if `tr()` returns something different in English
//! or it is literally in the dictionary (words spelled the same, like „Alt“).
//! Separate test process, because the language is switched globally.

use std::collections::HashSet;

/// Same in every language (abbreviations, font sample, command line)
const SAME: &[&str] = &["OK", "HSL", "LUT", "REC", "Cutline 0123 ÄÖÜ äöü ß", "cutline --mcp projekt.cutline"];

fn literals(src: &str, call: &str) -> Vec<String> {
    let mut out = Vec::new();
    let pat = format!("{call}\"");
    let mut rest = src;
    while let Some(i) = rest.find(&pat) {
        // `crate::tr(` and `self.toast(` count, `xtr(` does not
        let prev = rest[..i].chars().next_back();
        rest = &rest[i + pat.len()..];
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let mut s = String::new();
        let mut it = rest.chars();
        while let Some(c) = it.next() {
            match c {
                '"' => break,
                '\\' => match it.next() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some(c) => s.push(c),
                    None => {}
                },
                c => s.push(c),
            }
        }
        out.push(s);
    }
    out
}

#[test]
fn every_fixed_ui_text_has_english() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let keys: HashSet<&str> = cl_i18n::ui_entries().into_iter().map(|e| e.0).collect();
    cl_i18n::set_ui_lang(cl_i18n::Lang::En);
    let mut missing = Vec::new();
    let mut n = 0;
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == "rs")) {
        let src = std::fs::read_to_string(f).unwrap();
        // Tests at the end of the file check texts that are not in the UI
        let src = src.split("mod tests {").next().unwrap_or(&src);
        for call in ["tr(", "toast(", "toast_err(", "primary(", "secondary(", "danger(", "ghost(", "card_header(ui, ", "section(ui, ", "chip(ui, ", "toggle_row(ui, "] {
            for lit in literals(src, call) {
                if lit.contains('{') || !lit.chars().any(char::is_alphabetic) || SAME.contains(&lit.as_str()) {
                    continue; // template with format! placeholders, only symbols/numbers, language-neutral
                }
                n += 1;
                if cl_i18n::tr(&lit) == lit && !keys.contains(lit.trim()) {
                    missing.push(format!("{}: {lit:?}", f.file_name().unwrap().to_string_lossy()));
                }
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(n > 250, "too few texts found ({n}): check the search patterns");
    assert!(missing.is_empty(), "{} texts without an English line in ui.en.tsv:\n{}", missing.len(), missing.join("\n"));
}
