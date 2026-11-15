//! Änderungsliste („Was ist neu“) aus `CHANGELOG.md` oder dem Text einer GitHub-Release-Notiz.
//!
//! Aufbau: je Version `## 0.11.1 – 2026-11-15` (oder `## Unreleased`), darin `### English` und
//! `### Deutsch`, Gruppen als Zeile `**New**`, Einträge `- **Titel.** Satz.` oder `- Satz.`.
//! Alles andere (Kopf, `<details>`, „Full Changelog“-Link) wird übergangen.

use crate::Version;

/// Die Änderungsliste dieses Builds.
pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    /// Fetter Titel (leer bei Einträgen ohne Titel)
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Group {
    pub title: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Notes {
    /// `None` bei `## Unreleased`
    pub version: Option<Version>,
    /// Datum wie im Text (`JJJJ-MM-TT`), sonst leer
    pub date: String,
    pub en: Vec<Group>,
    pub de: Vec<Group>,
}

impl Notes {
    /// Gruppen in der gewünschten Sprache; fehlt sie, die andere.
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

/// Liest alle Versionen in Textreihenfolge. Leere Gruppen bleiben erhalten (Vorlage in `Unreleased`).
pub fn parse(text: &str) -> Vec<Notes> {
    let mut out: Vec<Notes> = Vec::new();
    let mut open = false; // innerhalb eines Versionsabschnitts
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
            // eingerückte Fortsetzung des letzten Eintrags
            if let Some(e) = groups.last_mut().and_then(|g| g.entries.last_mut()) {
                e.text = format!("{} {t}", e.text).trim().to_string();
            }
        }
    }
    out
}

/// `0.11.1 – 2026-11-15`, `[0.11.1] - 2026-11-15`, `v0.11.1`, `Unreleased` → (Version, Datum, unveröffentlicht)
fn heading(h: &str) -> (Option<Version>, String, bool) {
    let mut words = h.split_whitespace();
    let first = words.next().unwrap_or("").trim_matches(['[', ']']);
    let unreleased = first.eq_ignore_ascii_case("unreleased");
    let num = first.trim_start_matches(['v', 'V']);
    // nur „Zahl.Zahl…“ gilt als Version (sonst wäre „## 2026 …“ eine)
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

/// `**New**` oder `#### New` → „New“
fn group_title(t: &str) -> Option<&str> {
    let inner = t
        .strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        .filter(|s| !s.contains("**"))
        .or_else(|| t.strip_prefix("#### "))?;
    let inner = inner.trim().trim_end_matches(':').trim();
    (!inner.is_empty()).then_some(inner)
}

/// `**Titel.** Satz.` → (Titel., Satz.); sonst nur Text
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

/// Versionen nach `installed` bis einschließlich `target`, mit Inhalt, neueste zuerst, jede einmal.
pub fn between(all: &[Notes], installed: Version, target: Version) -> Vec<&Notes> {
    let mut v: Vec<&Notes> = all
        .iter()
        .filter(|n| n.version.is_some_and(|v| v > installed && v <= target) && !n.is_empty())
        .collect();
    v.sort_by(|a, b| b.version.cmp(&a.version));
    v.dedup_by(|a, b| a.version == b.version);
    v
}

/// Was nach dem Start einmal als „Was ist neu“ angeboten wird. `last`: zuletzt gestartete Version
/// (`None` = noch nie gemerkt), `known_user`: keine frische Installation.
pub fn news_after_update(all: &[Notes], last: Option<Version>, current: Version, known_user: bool) -> Vec<&Notes> {
    if !known_user {
        return Vec::new();
    }
    match last {
        Some(l) => between(all, l, current),
        // Update von einer Version, die sich das noch nicht merkte: nur die aktuelle zeigen
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
            assert!(!n.en.is_empty() && !n.de.is_empty(), "{name}: en und de");
            assert_eq!(n.en.len(), n.de.len(), "{name}: gleiche Gruppen in beiden Sprachen");
            if n.version.is_some() {
                assert!(n.date.len() == 10, "{name}: Datum");
                for g in n.en.iter().chain(&n.de) {
                    assert!(!g.title.is_empty(), "{name}: Gruppe ohne Titel");
                    assert!(!g.entries.is_empty(), "{name}: leere Gruppe {}", g.title);
                    for e in &g.entries {
                        assert!(!e.text.is_empty() || !e.title.is_empty(), "{name}: leerer Eintrag");
                    }
                }
            }
        }
        // Versionen absteigend, keine doppelt
        let vs: Vec<Version> = all.iter().filter_map(|n| n.version).collect();
        assert!(vs.windows(2).all(|w| w[0] > w[1]), "{vs:?}");
        let n = all.iter().find(|n| n.version == Some(Version(0, 11, 1))).unwrap();
        assert_eq!(n.de[0].title, "Neu");
        assert_eq!(n.de[0].entries[0].title, "Pakete ausblenden.");
        assert!(n.de[0].entries[0].text.starts_with("Unter Einstellungen"));
        assert_eq!(n.en[1].title, "Improved");
        assert_eq!(n.en[1].entries[0].title, "", "Eintrag ohne fetten Titel");
    }

    #[test]
    fn missing_language_falls_back() {
        let n = &parse("## 1.0.0\n### English\n**New**\n- **A.** b\n")[0];
        assert_eq!(n.groups(true)[0].entries[0].title, "A.");
        let n = &parse("## 1.0.0\n### Deutsch\n**Neu**\n- x\n### English\n**New**\n")[0];
        assert_eq!(n.groups(false)[0].title, "Neu", "leere Sprache zählt als fehlend");
        // ohne Sprachüberschrift: Englisch
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
        assert!(parse("## 2026 Pläne\n- x").is_empty(), "Jahreszahl ist keine Version");
        let n = &parse("## [1.2.3] - 2026-02-03\n### English\n#### Fixed:\n- **Kein Ende\n  zweite Zeile\n")[0];
        assert_eq!(n.en[0].title, "Fixed");
        assert_eq!(n.en[0].entries[0], Entry { title: String::new(), text: "**Kein Ende zweite Zeile".into() });
        // Release-Notiz mit Kopf, eingeklappten älteren Versionen und GitHub-Anhang
        let body = "## 1.1.0 – 2026-03-01\r\n### English\r\n**New**\r\n- **N.** x\r\n\r\n<details><summary>Earlier</summary>\r\n\r\n## 1.0.1\r\n**Fixed**\r\n- y\r\n</details>\r\n\r\n**Full Changelog**: https://github.com/x/compare/v1.0.0...v1.1.0\r\n";
        let all = parse(body);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].en.len(), 1, "„Full Changelog“ ist keine Gruppe");
        assert_eq!(all[1].en[0].entries.len(), 1);
    }

    #[test]
    fn versions_between_installed_and_new() {
        let all = parse(CHANGELOG);
        let v = |s: &str| Version::parse(s).unwrap();
        let got: Vec<Version> = between(&all, v("0.9.1"), v("0.11.1")).iter().filter_map(|n| n.version).collect();
        assert_eq!(got, [v("0.11.1"), v("0.11.0"), v("0.10.0")]);
        assert!(between(&all, v("0.11.1"), v("0.11.1")).is_empty());
        assert!(between(&all, v("0.11.1"), v("0.10.0")).is_empty(), "Rückschritt");
        let all = parse("## 1.0.0\n- a\n## 1.0.0\n- b\n## 0.9.0\n## 0.8.0\n- c\n");
        let got = between(&all, v("0.1"), v("1.0"));
        assert_eq!(got.len(), 2, "doppelte Version einmal, leere 0.9.0 nicht");
        assert_eq!(got[0].en[0].entries[0].text, "a");
    }

    #[test]
    fn news_once_after_update() {
        let all = parse(CHANGELOG);
        let v = |s: &str| Version::parse(s).unwrap();
        let cur = v("0.11.1");
        assert!(news_after_update(&all, None, cur, false).is_empty(), "frische Installation");
        assert!(news_after_update(&all, Some(cur), cur, true).is_empty(), "gleiche Version");
        assert!(news_after_update(&all, Some(v("0.12.0")), cur, true).is_empty(), "ältere Version gestartet");
        let got: Vec<_> = news_after_update(&all, Some(v("0.10.0")), cur, true).iter().filter_map(|n| n.version).collect();
        assert_eq!(got, [v("0.11.1"), v("0.11.0")], "höhere Version: alles seit der letzten");
        let got: Vec<_> = news_after_update(&all, None, cur, true).iter().filter_map(|n| n.version).collect();
        assert_eq!(got, [cur], "Update von einer Version ohne Merker");
        assert!(news_after_update(&all, Some(v("0.11.0")), v("9.0.0"), true).len() >= 1);
        assert!(news_after_update(&all, None, v("9.0.0"), true).is_empty(), "keine Notiz für diese Version");
    }

    /// Das Skript aus dem Release-Workflow schneidet genau den Abschnitt der Version aus, und die
    /// App liest das Ergebnis wieder. Läuft nur, wo `sh` und `awk` da sind (CI, Git Bash).
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
            eprintln!("kein sh – übersprungen");
            return;
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8(out.stdout).unwrap();
        let notes = parse(&text);
        let vs: Vec<_> = notes.iter().filter_map(|n| n.version).collect();
        assert_eq!(vs, [Version(0, 11, 0), Version(0, 10, 0), Version(0, 9, 1), Version(0, 9, 0)]);
        let all = parse(CHANGELOG);
        assert_eq!(notes[0], *all.iter().find(|n| n.version == Some(vs[0])).unwrap(), "unverändert übernommen");
        assert!(text.contains("<details>") && !text.contains("Unreleased"));
        // Vorabversion nimmt den Abschnitt der Basisversion; fehlender Abschnitt bricht ab
        assert!(run("0.11.0-rc.1").unwrap().status.success());
        let out = run("9.9.9").unwrap();
        assert!(!out.status.success() && out.stdout.is_empty());
    }
}
