//! Translations with Project Fluent.
//!
//! - Real plural and grammar rules instead of string tables
//! - A glossary test prevents known mistranslations
//! - Pseudo-localization (`qps`) makes texts ~40 % longer so truncated
//!   labels show up during development already

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

pub use fluent_bundle::FluentArgs as Args;

mod ui;
pub use ui::{entries as ui_entries, key_english, legacy_english, set_ui_lang, tr, tr_in, ui_lang};

const DE: &str = include_str!("../../../translations/de_DE/app.ftl");
const EN: &str = include_str!("../../../translations/en_US/app.ftl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
    /// Pseudo-localization for length and encoding tests.
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
    /// From the system language, e.g. "de-DE" → De.
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
        FluentResource::try_new(src.to_string()).expect("the FTL file is valid (checked by a test)");
    let mut b = FluentBundle::new_concurrent(vec![id]);
    b.set_use_isolating(false);
    b.add_resource(res).expect("no duplicate keys");
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

    /// Translates `key`. Missing keys show up visibly as `⟦key⟧`.
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

/// All keys of an FTL source (for completeness tests).
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

// `I18n::new` also sets the process-wide UI language; no test in this crate reads it
// (`ui::translate` ignores it and `tr` is not tested here), so the tests need no reset.
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
            "missing in en"
        );
        assert_eq!(
            en.difference(&de).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "missing in de"
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
        assert_eq!(de.t_num("files-importing", "count", 1), "Eine Datei wird importiert");
        assert_eq!(en.t_num("files-importing", "count", 3), "Importing 3 files");
        assert_eq!(en.t("does-not-exist"), "⟦does-not-exist⟧");
        // tool messages: singular without „1 …n“
        assert_eq!(de.t_num("sync-done", "count", 1), "Eine Aufnahme synchronisiert");
        assert_eq!(de.t_num("sync-done", "count", 3), "3 Aufnahmen synchronisiert");
        assert_eq!(en.t_num("recap-done", "count", 1), "Recap created from one marker");
        let db = [("count", FluentValue::from(1)), ("db", FluentValue::from("12"))];
        assert_eq!(de.t_args("music-ducked", &db), "Musik an einer Stelle abgesenkt (−12 dB)");
    }

    /// Known mistranslations must never appear here.
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
                assert!(!line.contains(f), "glossary violation „{f}“ in: {line}");
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

    #[test]
    fn locales_plural_edges_and_keys() {
        for (s, l) in [("de-DE", Lang::De), ("DE_at", Lang::De), ("de", Lang::De), ("", Lang::En), ("en-US", Lang::En), ("fr", Lang::En), ("d", Lang::En)] {
            assert_eq!(Lang::from_locale(s), l, "{s}");
        }
        let de = I18n::new(Lang::De);
        for n in [0, -1, 2, i64::MAX, i64::MIN] {
            let s = de.t_num("fonts-count", "count", n);
            assert!(!s.contains('⟦') && !s.is_empty(), "{n}: {s}");
        }
        // wrong argument name: no crash, the text stays readable
        assert!(!de.t_num("fonts-count", "falsch", 3).is_empty());
        assert!(!de.t_str("font-imported", "family", "🎬 {x} „").is_empty());
        assert_eq!(de.t(""), "⟦⟧");
        assert_eq!(keys("a = b\n  fortsetzung = x\n# c = d\nb=c\nd = \n = x"), vec!["a", "d"]);
        let mut i = I18n::new(Lang::En);
        i.set_lang(Lang::De);
        assert_eq!(i.lang(), Lang::De);
        assert_eq!(pseudo(""), "[]");
        assert!(pseudo("äbc").starts_with("[äbc"));
    }
}
