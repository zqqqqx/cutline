//! Jeder feste Oberflächentext im Code hat eine englische Übersetzung.
//!
//! Erfasst werden Textliterale direkt in `tr("…")`, `toast("…")`, `toast_err("…")` und den Knopf-/
//! Abschnitts-Helfern aus `theme`/`widgets` (die alle selbst übersetzen; Liste unten). Ein Text
//! gilt als übersetzt, wenn `tr()` auf Englisch etwas anderes liefert
//! oder er wörtlich im Wörterbuch steht (gleich lautende Wörter wie „Alt“).
//! Eigener Testprozess, weil die Sprache global umgestellt wird.

use std::collections::HashSet;

/// In jeder Sprache gleich (Abkürzungen, Schriftprobe, Befehlszeile)
const SAME: &[&str] = &["OK", "HSL", "LUT", "REC", "Cutline 0123 ÄÖÜ äöü ß", "cutline --mcp projekt.cutline"];

fn literals(src: &str, call: &str) -> Vec<String> {
    let mut out = Vec::new();
    let pat = format!("{call}\"");
    let mut rest = src;
    while let Some(i) = rest.find(&pat) {
        // `crate::tr(` und `self.toast(` zählen, `xtr(` nicht
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
        // Tests am Dateiende prüfen Texte, die nicht in der Oberfläche stehen
        let src = src.split("mod tests {").next().unwrap_or(&src);
        for call in ["tr(", "toast(", "toast_err(", "primary(", "secondary(", "danger(", "ghost(", "card_header(ui, ", "section(ui, ", "chip(ui, ", "toggle_row(ui, "] {
            for lit in literals(src, call) {
                if lit.contains('{') || !lit.chars().any(char::is_alphabetic) || SAME.contains(&lit.as_str()) {
                    continue; // Vorlage mit format!-Platzhaltern, nur Zeichen/Zahlen, sprachneutral
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
    assert!(n > 250, "zu wenige Texte gefunden ({n}): Suchmuster prüfen");
    assert!(missing.is_empty(), "{} Texte ohne englische Zeile in ui.en.tsv:\n{}", missing.len(), missing.join("\n"));
}
