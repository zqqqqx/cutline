//! Every fixed UI text in the code has a German translation.
//!
//! Covered are text literals directly in `tr("…")`, `toast("…")`, `toast_err("…")` and the button/
//! section helpers from `theme`/`widgets` (which all translate themselves; list below), the first
//! literal of a `format!(` directly in one of these calls (placeholders count as `{}`, a leading
//! icon placeholder is left out) and both texts of `count_text(`. A text counts as translated if
//! `tr()` returns something different in German or it is literally in the dictionary (words spelled
//! the same, like „Alt“).
//! Separate test process, because the language is switched globally.

use std::collections::HashSet;

/// Same in every language (abbreviations, font sample, command line)
const SAME: &[&str] = &["OK", "HSL", "LUT", "REC", "cutline --mcp project.cutline", "{} fps", "{} (R)"];

/// The literal starting at `rest` (just after its opening quote) and the code after it
fn literal(rest: &str) -> (String, &str) {
    let mut s = String::new();
    let mut it = rest.char_indices();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return (s, &rest[i + 1..]),
            '\\' => match it.next().map(|x| x.1) {
                Some('n') => s.push('\n'),
                Some('t') => s.push('\t'),
                Some(c) => s.push(c),
                None => {}
            },
            c => s.push(c),
        }
    }
    (s, "")
}

/// Literals right after `pat` (`tr("`, `toast(format!("` …): the text and the code after it
fn literals<'a>(src: &'a str, pat: &str) -> Vec<(String, &'a str)> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find(pat) {
        // `crate::tr(` and `self.toast(` count, `xtr(` does not
        let prev = rest[..i].chars().next_back();
        rest = &rest[i + pat.len()..];
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let (s, after) = literal(rest);
        out.push((s, after));
    }
    out
}

/// `format!` placeholders (`{n}`, `{:.1}`) as the dictionary writes them: `{}`
fn placeholders(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        match c {
            '{' => inside = true,
            '}' if inside => {
                inside = false;
                out.push_str("{}");
            }
            _ if inside => {}
            c => out.push(c),
        }
    }
    out
}

/// The texts of one source file that must have a German line
fn texts(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for call in ["tr(", "toast(", "toast_err(", "primary(", "secondary(", "danger(", "ghost(", "card_header(ui, ", "section(ui, ", "chip(ui, ", "toggle_row(ui, "] {
        for (lit, _) in literals(src, &format!("{call}\"")) {
            // a literal with braces here is no template of `format!`
            if !lit.contains('{') {
                out.push(lit);
            }
        }
        for (lit, after) in literals(src, &format!("{call}format!(\"")) {
            let t = placeholders(&lit);
            // „{}  Text“ or „Text  {}“ with an icon: tr() splits icons off, the icon is no word
            let icon = after.trim_start().trim_start_matches(',').trim_start().starts_with("icon::");
            out.push(match (icon, t.strip_prefix("{}"), t.strip_suffix("{}")) {
                (true, Some(rest), _) => rest.trim_start().to_string(),
                (true, None, Some(rest)) => rest.trim_end().to_string(),
                _ => t,
            });
        }
    }
    let mut rest = src;
    while let Some(i) = rest.find("count_text(") {
        rest = &rest[i + "count_text(".len()..];
        // the definition (`fn count_text(n: usize, …)`) has no texts
        let Some(q) = rest.find('"').filter(|q| !rest[..*q].contains(')')) else { continue };
        let (one, after) = literal(&rest[q + 1..]);
        let Some(q) = after.find('"') else { continue };
        let (many, _) = literal(&after[q + 1..]);
        out.extend([placeholders(&one), placeholders(&many)]);
    }
    out
}

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for p in std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()) {
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn every_fixed_ui_text_has_german() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let keys: HashSet<&str> = cl_i18n::ui_entries().into_iter().map(|e| e.0).collect();
    cl_i18n::set_ui_lang(cl_i18n::Lang::De);
    let mut missing = Vec::new();
    let mut n = 0;
    let mut files = Vec::new();
    rs_files(&dir, &mut files);
    files.sort();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        // Tests at the end of the file check texts that are not in the UI
        let src = src.split("mod tests {").next().unwrap_or(&src);
        for lit in texts(src) {
            if !lit.replace("{}", "").chars().any(char::is_alphabetic) || SAME.contains(&lit.as_str()) {
                continue; // only placeholders/symbols/numbers, language-neutral
            }
            n += 1;
            if cl_i18n::tr(&lit) == lit && !keys.contains(lit.trim()) {
                missing.push(format!("{}: {lit:?}", f.strip_prefix(&dir).unwrap().display()));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(n > 250, "too few texts found ({n}): check the search patterns");
    assert!(missing.is_empty(), "{} texts without a German line in translations/de_DE/ui.tsv:\n{}", missing.len(), missing.join("\n"));
}

/// The search finds the shapes of code the app uses for its texts
#[test]
fn texts_of_templates_and_counts_are_found() {
    let src = r#"self.toast(format!("{n} installed")); crate::tr(format!("Project {n}"));
        theme::primary(format!("{}  New project", icon::PLUS)); self.toast(format!("{} saved", icon::CHECK)); primary(format!("Next  {}", icon::ARROW_RIGHT));
        count_text(n, "{} filter", "{n} filters"); pub fn count_text(n: usize, one: &str) {}"#;
    assert_eq!(texts(src), ["Project {}", "{} installed", "saved", "New project", "Next", "{} filter", "{} filters"]);
}
