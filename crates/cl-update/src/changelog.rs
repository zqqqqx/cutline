//! Change list („Was ist neu“) from `CHANGELOG.md` or the text of a GitHub release note.
//!
//! Structure: per version `## 0.11.1 – 2026-11-15` (or `## Unreleased`), in it `### English` and
//! `### Deutsch`, groups as a line `**New**`, entries `- **Title.** Sentence.` or `- Sentence.`.
//! Everything else (header, `<details>`, "Full Changelog" link) is skipped.
//!
//! Newer releases carry the same data additionally as the asset `changes.json`
//! (`.github/changes-json.sh`, read by [`parse_json`]); then the release text is not needed.

use serde::Deserialize;

use crate::Version;

/// The change list of this build.
pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// Bold title (empty for entries without a title)
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Group {
    #[serde(rename = "group")]
    pub title: String,
    #[serde(rename = "items")]
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Notes {
    /// `None` for `## Unreleased`
    #[serde(deserialize_with = "version_text")]
    pub version: Option<Version>,
    /// Date as in the text (`YYYY-MM-DD`), otherwise empty
    pub date: String,
    pub en: Vec<Group>,
    pub de: Vec<Group>,
}

impl Notes {
    /// Groups in the requested language; if it is missing, the other one.
    pub fn groups(&self, de: bool) -> &[Group] {
        let (want, other) = if de { (&self.de, &self.en) } else { (&self.en, &self.de) };
        if has_entries(want) { want } else { other }
    }
    pub fn is_empty(&self) -> bool {
        !has_entries(&self.en) && !has_entries(&self.de)
    }
}

fn has_entries(g: &[Group]) -> bool {
    g.iter().any(|g| !g.entries.is_empty())
}

/// `"0.12.1"` → version (unreadable: `None`, the version then drops out of [`between`])
fn version_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Version>, D::Error> {
    Ok(Version::parse(&String::deserialize(d)?))
}

/// `changes.json` of a release (format 1, structure see `.github/changes-json.sh`).
/// `None` for broken JSON or an unknown `format`: then the release text applies.
pub fn parse_json(text: &str) -> Option<Vec<Notes>> {
    #[derive(Deserialize)]
    struct File {
        format: u32,
        versions: Vec<Notes>,
    }
    let f: File = serde_json::from_str(text).ok()?;
    (f.format == 1).then_some(f.versions)
}

/// Reads all versions in text order. Empty groups are kept (template in `Unreleased`).
pub fn parse(text: &str) -> Vec<Notes> {
    let mut out: Vec<Notes> = Vec::new();
    let mut open = false; // inside a version section
    let mut de = false;
    for raw in text.lines() {
        let t = raw.trim();
        if let Some(h) = t.strip_prefix("## ") {
            let (version, date, unreleased) = heading(h);
            open = version.is_some() || unreleased;
            if open {
                out.push(Notes { version, date, ..Default::default() });
                de = false;
            }
            continue;
        }
        let Some(n) = out.last_mut().filter(|_| open) else { continue };
        if let Some(h) = t.strip_prefix("### ") {
            de = matches!(h.trim().to_lowercase().as_str(), "deutsch" | "de" | "german");
            continue;
        }
        let groups = if de { &mut n.de } else { &mut n.en };
        if let Some(g) = group_title(t) {
            groups.push(Group { title: g.into(), entries: Vec::new() });
        } else if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            if groups.is_empty() {
                groups.push(Group::default());
            }
            groups.last_mut().unwrap().entries.push(entry(item.trim()));
        } else if !t.is_empty() && raw.starts_with([' ', '\t']) {
            // indented continuation of the last entry
            if let Some(e) = groups.last_mut().and_then(|g| g.entries.last_mut()) {
                e.text = format!("{} {t}", e.text).trim().to_string();
            }
        }
    }
    out
}

/// `0.11.1 – 2026-11-15`, `[0.11.1] - 2026-11-15`, `v0.11.1`, `Unreleased` → (version, date, unreleased)
fn heading(h: &str) -> (Option<Version>, String, bool) {
    let mut words = h.split_whitespace();
    let first = words.next().unwrap_or("").trim_matches(['[', ']']);
    let unreleased = first.eq_ignore_ascii_case("unreleased");
    let num = first.trim_start_matches(['v', 'V']);
    // only "number.number…" counts as a version (otherwise "## 2026 …" would be one)
    let version = (num.starts_with(|c: char| c.is_ascii_digit()) && num.contains('.'))
        .then(|| Version::parse(num))
        .flatten();
    let date = h
        .split_whitespace()
        .find(|w| is_date(w))
        .unwrap_or("")
        .to_string();
    (version, date, unreleased)
}

fn is_date(w: &str) -> bool {
    let b = w.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// `**New**` or `#### New` → "New"
fn group_title(t: &str) -> Option<&str> {
    let inner = t
        .strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        .filter(|s| !s.contains("**"))
        .or_else(|| t.strip_prefix("#### "))?;
    let inner = inner.trim().trim_end_matches(':').trim();
    (!inner.is_empty()).then_some(inner)
}

/// `**Title.** Sentence.` → (Title., Sentence.); otherwise text only
fn entry(item: &str) -> Entry {
    if let Some(rest) = item.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            return Entry {
                title: rest[..end].trim().to_string(),
                text: rest[end + 2..].trim().to_string(),
            };
        }
    }
    Entry { title: String::new(), text: item.to_string() }
}

/// Versions after `installed` up to and including `target`, with content, newest first, each once.
pub fn between(all: &[Notes], installed: Version, target: Version) -> Vec<&Notes> {
    let mut v: Vec<&Notes> = all
        .iter()
        .filter(|n| n.version.is_some_and(|v| v > installed && v <= target) && !n.is_empty())
        .collect();
    v.sort_by(|a, b| b.version.cmp(&a.version));
    v.dedup_by(|a, b| a.version == b.version);
    v
}

/// What is offered once as „Was ist neu“ after the start. `last`: last started version
/// (`None` = never remembered), `known_user`: not a fresh installation.
pub fn news_after_update(all: &[Notes], last: Option<Version>, current: Version, known_user: bool) -> Vec<&Notes> {
    if !known_user {
        return Vec::new();
    }
    match last {
        Some(l) => between(all, l, current),
        // update from a version that did not remember this yet: show only the current one
        None => all.iter().filter(|n| n.version == Some(current) && !n.is_empty()).take(1).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_changelog_is_complete() {
        let all = parse(CHANGELOG);
        assert!(all.len() >= 5, "{}", all.len());
        assert_eq!(all[0].version, None, "oben steht Unreleased");
        for n in &all {
            let name = n.version.map(|v| v.to_string()).unwrap_or("Unreleased".into());
            assert!(!n.en.is_empty() && !n.de.is_empty(), "{name}: en and de");
            assert_eq!(n.en.len(), n.de.len(), "{name}: gleiche Gruppen in beiden Sprachen");
            if n.version.is_some() {
                assert!(n.date.len() == 10, "{name}: Datum");
                for g in n.en.iter().chain(&n.de) {
                    assert!(!g.title.is_empty(), "{name}: group without title");
                    assert!(!g.entries.is_empty(), "{name}: leere Gruppe {}", g.title);
                    for e in &g.entries {
                        assert!(!e.text.is_empty() || !e.title.is_empty(), "{name}: leerer Eintrag");
                    }
                }
            }
        }
        // versions descending, none twice
        let vs: Vec<Version> = all.iter().filter_map(|n| n.version).collect();
        assert!(vs.windows(2).all(|w| w[0] > w[1]), "{vs:?}");
        let n = all.iter().find(|n| n.version == Some(Version(0, 11, 1))).unwrap();
        assert_eq!(n.de[0].title, "Neu");
        assert_eq!(n.de[0].entries[0].title, "Pakete ausblenden.");
        assert!(n.de[0].entries[0].text.starts_with("Unter Einstellungen"));
        assert_eq!(n.en[1].title, "Improved");
        assert_eq!(n.en[1].entries[0].title, "", "entry without bold title");
    }

    #[test]
    fn missing_language_falls_back() {
        let n = &parse("## 1.0.0\n### English\n**New**\n- **A.** b\n")[0];
        assert_eq!(n.groups(true)[0].entries[0].title, "A.");
        let n = &parse("## 1.0.0\n### Deutsch\n**Neu**\n- x\n### English\n**New**\n")[0];
        assert_eq!(n.groups(false)[0].title, "Neu", "an empty language counts as missing");
        // without a language heading: English
        let n = &parse("## v2.1 - 2026-01-02\n- nur Text\n")[0];
        assert_eq!((n.version, n.date.as_str()), (Some(Version(2, 1, 0)), "2026-01-02"));
        assert_eq!(n.groups(true)[0].entries[0].text, "nur Text");
    }

    #[test]
    fn broken_input_is_tolerated() {
        for s in ["", "kein Changelog", "## \n###\n- \n**\n****", "- vor jeder Version\n**New**", "## 2026 Pläne\n- x", "## [🎬] – \n- **offen"] {
            let all = parse(s);
            assert!(all.iter().all(|n| n.is_empty() || n.version.is_some()), "{s:?}");
        }
        assert!(parse("## 2026 Pläne\n- x").is_empty(), "a year is not a version");
        let n = &parse("## [1.2.3] - 2026-02-03\n### English\n#### Fixed:\n- **Kein Ende\n  zweite Zeile\n")[0];
        assert_eq!(n.en[0].title, "Fixed");
        assert_eq!(n.en[0].entries[0], Entry { title: String::new(), text: "**Kein Ende zweite Zeile".into() });
        // release note with header, collapsed older versions and GitHub asset
        let body = "## 1.1.0 – 2026-03-01\r\n### English\r\n**New**\r\n- **N.** x\r\n\r\n<details><summary>Earlier</summary>\r\n\r\n## 1.0.1\r\n**Fixed**\r\n- y\r\n</details>\r\n\r\n**Full Changelog**: https://github.com/x/compare/v1.0.0...v1.1.0\r\n";
        let all = parse(body);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].en.len(), 1, "\"Full Changelog\" is not a group");
        assert_eq!(all[1].en[0].entries.len(), 1);
    }

    #[test]
    fn versions_between_installed_and_new() {
        let all = parse(CHANGELOG);
        let v = |s: &str| Version::parse(s).unwrap();
        let got: Vec<Version> = between(&all, v("0.9.1"), v("0.11.1")).iter().filter_map(|n| n.version).collect();
        assert_eq!(got, [v("0.11.1"), v("0.11.0"), v("0.10.0")]);
        assert!(between(&all, v("0.11.1"), v("0.11.1")).is_empty());
        assert!(between(&all, v("0.11.1"), v("0.10.0")).is_empty(), "downgrade");
        let all = parse("## 1.0.0\n- a\n## 1.0.0\n- b\n## 0.9.0\n## 0.8.0\n- c\n");
        let got = between(&all, v("0.1"), v("1.0"));
        assert_eq!(got.len(), 2, "duplicate version once, empty 0.9.0 not at all");
        assert_eq!(got[0].en[0].entries[0].text, "a");
    }

    #[test]
    fn news_once_after_update() {
        let all = parse(CHANGELOG);
        let v = |s: &str| Version::parse(s).unwrap();
        let cur = v("0.11.1");
        assert!(news_after_update(&all, None, cur, false).is_empty(), "frische Installation");
        assert!(news_after_update(&all, Some(cur), cur, true).is_empty(), "gleiche Version");
        assert!(news_after_update(&all, Some(v("0.12.0")), cur, true).is_empty(), "older version started");
        let got: Vec<_> = news_after_update(&all, Some(v("0.10.0")), cur, true).iter().filter_map(|n| n.version).collect();
        assert_eq!(got, [v("0.11.1"), v("0.11.0")], "higher version: everything since the last one");
        let got: Vec<_> = news_after_update(&all, None, cur, true).iter().filter_map(|n| n.version).collect();
        assert_eq!(got, [cur], "update from a version without the marker");
        assert!(news_after_update(&all, Some(v("0.11.0")), v("9.0.0"), true).len() >= 1);
        assert!(news_after_update(&all, None, v("9.0.0"), true).is_empty(), "no note for this version");
    }

    #[test]
    fn changes_json_is_read_or_rejected() {
        let ok = r#"{"format": 1, "versions": [
            {"version": "1.2.0", "date": "2026-03-01",
             "en": [{"group": "New", "items": [{"title": "A.", "text": "b"}, {"text": "nur Satz"}]}],
             "de": [{"group": "Neu", "items": [{"title": "A.", "text": "b \"zitiert\""}]}],
             "später": true},
            {"version": "1.1.0", "en": [{"group": "Fixed", "items": [{"text": "x"}]}]}]}"#;
        let all = parse_json(ok).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].version, all[0].date.as_str()), (Some(Version(1, 2, 0)), "2026-03-01"));
        assert_eq!(all[0].en[0].entries[1], Entry { title: String::new(), text: "nur Satz".into() });
        assert_eq!(all[0].groups(true)[0].entries[0].text, "b \"zitiert\"");
        // missing language: the other one
        assert_eq!(all[1].groups(true)[0].title, "Fixed");
        assert_eq!(all[1].date, "");
        let got: Vec<_> = between(&all, Version(1, 0, 0), Version(1, 2, 0)).iter().filter_map(|n| n.version).collect();
        assert_eq!(got, [Version(1, 2, 0), Version(1, 1, 0)]);
        // unknown format, broken JSON, wrong shape: fall back to the release text
        assert!(parse_json(&ok.replace(r#""format": 1"#, r#""format": 2"#)).is_none());
        assert!(parse_json(&ok[..ok.len() - 3]).is_none());
        for bad in ["", "null", "[]", "{}", r#"{"format": 1}"#, r#"{"format": "1", "versions": []}"#, r#"{"format": 1, "versions": [{"en": "x"}]}"#] {
            assert!(parse_json(bad).is_none(), "{bad}");
        }
        assert_eq!(parse_json(r#"{"format": 1, "versions": []}"#), Some(Vec::new()));
    }

    /// `changes-json.sh` from the release workflow yields the same data as `parse` from
    /// `CHANGELOG.md` – for the version and all earlier ones. Runs only where `sh` and `awk` exist.
    #[test]
    fn changes_json_script_matches_the_changelog() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let run = |ver: &str, file: &std::path::Path| {
            std::process::Command::new("sh")
                .arg(root.join(".github/changes-json.sh"))
                .args([ver, &file.to_string_lossy()])
                .output()
        };
        let Ok(out) = run("v0.11.0", &root.join("CHANGELOG.md")) else {
            eprintln!("no sh – skipped");
            return;
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let got = parse_json(&String::from_utf8(out.stdout).unwrap()).expect("valid changes.json");
        let want: Vec<Notes> = parse(CHANGELOG).into_iter().filter(|n| n.version.is_some_and(|v| v <= Version(0, 11, 0))).collect();
        assert!(want.len() >= 4);
        assert_eq!(got, want);
        // small example: special characters, continuation lines, entry without group, pre-release
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("CHANGELOG.md");
        let text = "# Changelog\r\n\r\n## Unreleased\r\n### English\r\n**New**\r\n- offen\r\n\r\n## [1.1.0] - 2026-02-03\r\n### English\r\n- ohne Gruppe\r\n#### Fixed:\r\n- **Pfad \"C:\\x\".**\tTab\r\n  zweite Zeile\r\n### Deutsch\r\n**Behoben**\r\n* Ä ö 🎬\r\n\r\n## 1.0.0\r\n**New**\r\n- alt\r\n";
        std::fs::write(&f, text).unwrap();
        let out = run("1.1.0-rc.1", &f).unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let got = parse_json(&String::from_utf8(out.stdout).unwrap()).expect("valid");
        assert_eq!(got, parse(text)[1..]);
        assert_eq!(got[0].en[1].entries[0], Entry { title: "Pfad \"C:\\x\".".into(), text: "Tab zweite Zeile".into() });
        // a missing section aborts without output
        let out = run("9.9.9", &f).unwrap();
        assert!(!out.status.success() && out.stdout.is_empty());
    }

    /// The script from the release workflow cuts out exactly the version's section, and the
    /// app reads the result back. Runs only where `sh` and `awk` exist (CI, Git Bash).
    #[test]
    fn release_notes_script_cuts_the_version() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let run = |ver: &str| {
            std::process::Command::new("sh")
                .arg(root.join(".github/release-notes.sh"))
                .args([ver, &root.join("CHANGELOG.md").to_string_lossy()])
                .output()
        };
        let Ok(out) = run("v0.11.0") else {
            eprintln!("no sh – skipped");
            return;
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8(out.stdout).unwrap();
        let notes = parse(&text);
        let vs: Vec<_> = notes.iter().filter_map(|n| n.version).collect();
        assert_eq!(vs, [Version(0, 11, 0), Version(0, 10, 0), Version(0, 9, 1), Version(0, 9, 0)]);
        let all = parse(CHANGELOG);
        assert_eq!(notes[0], *all.iter().find(|n| n.version == Some(vs[0])).unwrap(), "adopted unchanged");
        assert!(text.contains("<details>") && !text.contains("Unreleased"));
        // a pre-release takes the section of the base version; a missing section aborts
        assert!(run("0.11.0-rc.1").unwrap().status.success());
        let out = run("9.9.9").unwrap();
        assert!(!out.status.success() && out.stdout.is_empty());
    }
}
