//! Übersetzungen mit Project Fluent.
//!
//! - Echte Plural- und Grammatikregeln statt String-Tabellen
//! - Glossar-Test verhindert bekannte Fehlübersetzungen
//! - Pseudo-Lokalisierung (`qps`) macht Texte ~40 % länger, damit abgeschnittene
//!   Labels schon in der Entwicklung auffallen

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

pub use fluent_bundle::FluentArgs as Args;

mod ui;
pub use ui::{entries as ui_entries, set_ui_lang, tr, ui_lang};

const DE: &str = include_str!("../locales/de.ftl");
const EN: &str = include_str!("../locales/en.ftl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
    /// Pseudo-Lokalisierung für Längen- und Kodierungstests.
    Pseudo,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::De, Lang::En, Lang::Pseudo];
    pub fn code(self) -> &'static str {
        match self {
            Lang::De => "de",
            Lang::En => "en",
            Lang::Pseudo => "qps",
        }
    }
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::De => "Deutsch",
            Lang::En => "English",
            Lang::Pseudo => "Pseudo (Test)",
        }
    }
    /// Aus Systemsprache, z. B. "de-DE" → De.
    pub fn from_locale(s: &str) -> Lang {
        if s.to_ascii_lowercase().starts_with("de") {
            Lang::De
        } else {
            Lang::En
        }
    }
}

pub struct I18n {
    lang: Lang,
    bundle: FluentBundle<FluentResource>,
}

fn bundle_for(lang: Lang) -> FluentBundle<FluentResource> {
    let (src, id): (&str, LanguageIdentifier) = match lang {
        Lang::De => (DE, "de".parse().unwrap()),
        Lang::En | Lang::Pseudo => (EN, "en".parse().unwrap()),
    };
    let res =
        FluentResource::try_new(src.to_string()).expect("FTL-Datei ist gültig (per Test geprüft)");
    let mut b = FluentBundle::new_concurrent(vec![id]);
    b.set_use_isolating(false);
    b.add_resource(res).expect("keine doppelten Schlüssel");
    b
}

fn pseudo(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| match c {
            'a' => 'á',
            'e' => 'ë',
            'i' => 'ï',
            'o' => 'ô',
            'u' => 'ü',
            'A' => 'Å',
            'E' => 'É',
            'O' => 'Ö',
            'U' => 'Û',
            c => c,
        })
        .collect();
    let pad = (s.chars().count() * 2).div_ceil(5); // +40 %
    format!("[{mapped}{}]", "~".repeat(pad))
}

impl I18n {
    pub fn new(lang: Lang) -> Self {
        set_ui_lang(lang);
        Self {
            lang,
            bundle: bundle_for(lang),
        }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn set_lang(&mut self, lang: Lang) {
        if lang != self.lang {
            *self = Self::new(lang);
        }
    }

    /// Übersetzt `key`. Fehlende Schlüssel erscheinen sichtbar als `⟦key⟧`.
    pub fn t(&self, key: &str) -> String {
        self.format(key, None)
    }

    pub fn t_args(&self, key: &str, args: &[(&str, FluentValue<'_>)]) -> String {
        let mut a = FluentArgs::new();
        for (k, v) in args {
            a.set(*k, v.clone());
        }
        self.format(key, Some(&a))
    }

    pub fn t_num(&self, key: &str, name: &str, n: i64) -> String {
        self.t_args(key, &[(name, FluentValue::from(n))])
    }

    pub fn t_str(&self, key: &str, name: &str, v: &str) -> String {
        self.t_args(key, &[(name, FluentValue::from(v))])
    }

    fn format(&self, key: &str, args: Option<&FluentArgs>) -> String {
        let Some(pattern) = self.bundle.get_message(key).and_then(|m| m.value()) else {
            return format!("⟦{key}⟧");
        };
        let mut errs = Vec::new();
        let s = self
            .bundle
            .format_pattern(pattern, args, &mut errs)
            .into_owned();
        if self.lang == Lang::Pseudo {
            pseudo(&s)
        } else {
            s
        }
    }
}

/// Alle Schlüssel einer FTL-Quelle (für Vollständigkeits-Tests).
pub fn keys(src: &str) -> Vec<String> {
    src.lines()
        .filter(|l| !l.starts_with(' ') && !l.starts_with('#'))
        .filter_map(|l| l.split_once(" =").map(|(k, _)| k.trim().to_string()))
        .filter(|k| !k.is_empty())
        .collect()
}

pub fn source(lang: Lang) -> &'static str {
    match lang {
        Lang::De => DE,
        Lang::En | Lang::Pseudo => EN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn all_locales_parse_and_have_same_keys() {
        for src in [DE, EN] {
            FluentResource::try_new(src.to_string()).expect("FTL fehlerfrei");
        }
        let de: BTreeSet<_> = keys(DE).into_iter().collect();
        let en: BTreeSet<_> = keys(EN).into_iter().collect();
        assert_eq!(
            de.difference(&en).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "fehlt in en"
        );
        assert_eq!(
            en.difference(&de).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "fehlt in de"
        );
    }

    #[test]
    fn plurals_and_args() {
        let de = I18n::new(Lang::De);
        assert_eq!(
            de.t_num("fonts-count", "count", 1),
            "Eine importierte Schrift"
        );
        assert_eq!(
            de.t_num("fonts-count", "count", 3),
            "3 importierte Schriften"
        );
        assert_eq!(
            de.t_str("font-imported", "family", "Impact"),
            "Schrift „Impact“ importiert"
        );
        let en = I18n::new(Lang::En);
        assert_eq!(en.t_num("media-imported", "count", 2), "Imported 2 items");
        assert_eq!(en.t("does-not-exist"), "⟦does-not-exist⟧");
    }

    /// Bekannte Fehlübersetzungen dürfen bei uns nie auftauchen.
    #[test]
    fn german_glossary() {
        let forbidden = [
            "Rahmen",
            "Kostenlos",
            "Mittelgrau",
            "Abkürzung",
            "Personen suchen",
            "Geschw.",
        ];
        for line in DE.lines().filter(|l| !l.starts_with('#')) {
            for f in forbidden {
                assert!(!line.contains(f), "Glossar-Verstoß „{f}“ in: {line}");
            }
        }
    }

    #[test]
    fn pseudo_is_longer() {
        let p = I18n::new(Lang::Pseudo);
        let s = p.t("import-media");
        assert!(
            s.starts_with('[')
                && s.chars().count() as f32 >= "Import media …".chars().count() as f32 * 1.4
        );
    }
}
