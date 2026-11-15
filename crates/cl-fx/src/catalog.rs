//! Ein Katalog für alle Inhalte: eingebaute Effekte, Filter, Übergänge, Sounds und Sticker
//! (Paket „Grundausstattung“, `cutline.standard`) und Inhalte aus Paketen (`asset-pack`).
//!
//! Eingebaute Einträge stehen weiter im Code (IDs unverändert), Pakete werden mit
//! [`load_packs`] angemeldet. Vorschau, Export und die Tabs lesen von hier.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cl_core::{ClipContent, Project};

/// Paket-ID der eingebauten Inhalte.
pub const STANDARD: &str = "cutline.standard";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Effect,
    Filter,
    Transition,
    Sound,
    Sticker,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Effect, Kind::Filter, Kind::Transition, Kind::Sound, Kind::Sticker];
}

/// Was ein Eintrag mitbringt.
#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    /// Im Code definiert ([`crate::EFFECTS`], [`crate::sfx::SFX`] …)
    Builtin,
    /// FFmpeg-Kette mit `{intensity}` (Effekt/Filter aus Paketen)
    Ffmpeg(String),
    /// Datei im Paketordner (Sound/Sticker)
    File(PathBuf),
    /// Übergang als WGSL-Shader (cl-gpu) mit eingebautem `xfade`-Rückfall und den
    /// Standardwerten der Parameter (`params.p0` …)
    Shader {
        file: PathBuf,
        fallback: String,
        params: Vec<f32>,
    },
}

/// Wie ein Übergang gerendert wird.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionPlan {
    /// Eingebauter `xfade`-Name (immer gültig: Rückfall oder `fade`)
    pub xfade: &'static str,
    /// Shader-Datei und Parameter, wenn das Paket eingeschaltet ist
    pub shader: Option<(PathBuf, Vec<f32>)>,
}

/// Übergang `id` auflösen: eingebaut → `xfade`; Paket-Übergang → Shader samt Rückfall;
/// Paket aus → nur der Rückfall; unbekannt (Paket fehlt) → Überblenden.
pub fn transition_plan_in(packs: &[Pack], id: &str) -> TransitionPlan {
    let builtin = |x: &str| crate::TRANSITIONS.iter().find(|t| t.0 == x).map(|t| t.0);
    if let Some(x) = builtin(id) {
        return TransitionPlan { xfade: x, shader: None };
    }
    let found = packs
        .iter()
        .flat_map(|p| p.entries.iter().map(move |e| (p.enabled, e)))
        .find(|(_, e)| e.kind == Kind::Transition && e.id == id);
    match found {
        Some((on, Entry { payload: Payload::Shader { file, fallback, params }, .. })) => TransitionPlan {
            xfade: builtin(fallback).unwrap_or("fade"),
            shader: on.then(|| (file.clone(), params.clone())),
        },
        _ => TransitionPlan { xfade: "fade", shader: None },
    }
}

pub fn transition_plan(id: &str) -> TransitionPlan {
    read(|p| transition_plan_in(p, id))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Eingebaut: wie bisher (`blur`, `heart` …); Paket: `<paket-id>/<asset-id>`
    pub id: String,
    pub kind: Kind,
    /// Name in der aktuellen Sprache (eingebaut: deutsch, die Oberfläche übersetzt)
    pub name: String,
    pub categories: Vec<String>,
    pub pack: String,
    pub license: String,
    pub author: String,
    pub source: Option<String>,
    /// Schlagworte (Paket-Manifest `tags`): die Suche in den Tabs findet den Eintrag auch darüber
    pub tags: Vec<String>,
    pub payload: Payload,
}

impl Entry {
    /// Erste Kategorie (Ordner im Tab)
    pub fn category(&self) -> &str {
        self.categories.first().map(String::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub license: String,
    pub author: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub info: PackInfo,
    pub enabled: bool,
    pub entries: Vec<Entry>,
}

/// Paket mit Anzahl der Einträge je Art.
#[derive(Debug, Clone, PartialEq)]
pub struct PackSummary {
    pub info: PackInfo,
    pub enabled: bool,
    pub counts: BTreeMap<Kind, usize>,
}

static PACKS: std::sync::RwLock<Vec<Pack>> = std::sync::RwLock::new(Vec::new());

/// Paketeinträge neu laden (ersetzt die bisherigen; eingeschaltet oder nicht).
pub fn load_packs(packs: Vec<Pack>) {
    if let Ok(mut g) = PACKS.write() {
        *g = packs;
    }
}

fn read<T>(f: impl FnOnce(&[Pack]) -> T) -> T {
    match PACKS.read() {
        Ok(g) => f(&g),
        Err(_) => f(&[]),
    }
}

/// Angaben zur Grundausstattung.
pub fn standard_info() -> PackInfo {
    PackInfo {
        id: STANDARD.into(),
        name: "Grundausstattung".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: "Eingebaute Effekte, Filter, Übergänge, Sounds und Sticker".into(),
        license: "MPL-2.0".into(),
        author: "Cutline-Projekt".into(),
        source: Some("https://github.com/zqqqqx/cutline".into()),
    }
}

fn builtin(kind: Kind) -> Vec<Entry> {
    let info = standard_info();
    let e = |id: &str, name: &str, cat: &str| Entry {
        id: id.into(),
        kind,
        name: name.into(),
        categories: vec![cat.into()],
        pack: STANDARD.into(),
        license: info.license.clone(),
        author: info.author.clone(),
        source: info.source.clone(),
        tags: Vec::new(),
        payload: Payload::Builtin,
    };
    match kind {
        Kind::Effect => crate::EFFECTS.iter().map(|d| e(d.id, d.name, d.category)).collect(),
        Kind::Filter => crate::FILTERS.iter().map(|d| e(d.id, d.name, d.category)).collect(),
        Kind::Transition => crate::TRANSITIONS.iter().map(|t| e(t.0, t.1, t.2)).collect(),
        Kind::Sound => crate::sfx::SFX.iter().map(|s| e(s.id, s.name, s.category)).collect(),
        Kind::Sticker => crate::stickers::STICKERS
            .iter()
            .map(|s| e(s.id, s.name, s.category))
            .collect(),
    }
}

/// Eingebaute zuerst (heutige Reihenfolge), dann eingeschaltete Pakete.
pub fn list_in(packs: &[Pack], kind: Kind) -> Vec<Entry> {
    let mut v = builtin(kind);
    v.extend(
        packs
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| p.entries.iter().filter(|e| e.kind == kind).cloned()),
    );
    v
}

pub fn list(kind: Kind) -> Vec<Entry> {
    read(|p| list_in(p, kind))
}

pub fn get_in(packs: &[Pack], kind: Kind, id: &str) -> Option<Entry> {
    let known = match kind {
        Kind::Effect => crate::effect(id).is_some(),
        Kind::Filter => crate::filter(id).is_some(),
        Kind::Transition => crate::TRANSITIONS.iter().any(|t| t.0 == id),
        Kind::Sound => crate::sfx::sfx(id).is_some(),
        Kind::Sticker => crate::stickers::sticker(id).is_some(),
    };
    if known {
        return builtin(kind).into_iter().find(|e| e.id == id);
    }
    packs
        .iter()
        .filter(|p| p.enabled)
        .flat_map(|p| p.entries.iter())
        .find(|e| e.kind == kind && e.id == id)
        .cloned()
}

pub fn get(kind: Kind, id: &str) -> Option<Entry> {
    read(|p| get_in(p, kind, id))
}

/// FFmpeg-Kette eines Paket-Effekts/-Filters mit eingesetzter Stärke.
pub(crate) fn pack_chain(kind: Kind, id: &str, intensity: f32) -> Option<String> {
    read(|packs| {
        packs
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| p.entries.iter())
            .find(|e| e.kind == kind && e.id == id)
            .and_then(|e| match &e.payload {
                Payload::Ffmpeg(c) => Some(c.replace("{intensity}", &format!("{intensity:.3}"))),
                _ => None,
            })
    })
}

fn counts(entries: &[Entry]) -> BTreeMap<Kind, usize> {
    let mut m = BTreeMap::new();
    for e in entries {
        *m.entry(e.kind).or_insert(0) += 1;
    }
    m
}

/// Grundausstattung zuerst, dann alle geladenen Pakete (auch ausgeschaltete).
pub fn packs_in(packs: &[Pack]) -> Vec<PackSummary> {
    let std_entries: Vec<Entry> = Kind::ALL.iter().flat_map(|k| builtin(*k)).collect();
    let mut v = vec![PackSummary {
        info: standard_info(),
        enabled: true,
        counts: counts(&std_entries),
    }];
    v.extend(packs.iter().map(|p| PackSummary {
        info: p.info.clone(),
        enabled: p.enabled,
        counts: counts(&p.entries),
    }));
    v
}

pub fn packs() -> Vec<PackSummary> {
    read(packs_in)
}

/// Danksagung als Markdown: je Paket Name, Lizenz, Autor, Quelle; abweichende Angaben
/// einzelner Inhalte gruppiert. `tr` übersetzt die festen Texte.
pub fn credits(packs: &[Pack], tr: &dyn Fn(&str) -> String) -> String {
    let std = Pack {
        info: standard_info(),
        enabled: true,
        entries: Vec::new(),
    };
    let mut out = format!("# {}\n", tr("Quellen und Lizenzen"));
    for p in std::iter::once(&std).chain(packs) {
        let i = &p.info;
        out += &format!("\n## {} ({})\n\n", i.name, i.id);
        out += &format!("- {}: {}\n", tr("Lizenz"), i.license);
        if !i.author.is_empty() {
            out += &format!("- {}: {}\n", tr("Autor"), i.author);
        }
        if let Some(s) = &i.source {
            out += &format!("- {}: {s}\n", tr("Quelle"));
        }
        // (Lizenz, Autor, Quelle) → Namen, in Reihenfolge des ersten Auftretens
        let mut groups: Vec<((&str, &str, Option<&str>), Vec<&str>)> = Vec::new();
        for e in &p.entries {
            let k = (e.license.as_str(), e.author.as_str(), e.source.as_deref());
            if k == (i.license.as_str(), i.author.as_str(), i.source.as_deref()) {
                continue;
            }
            match groups.iter_mut().find(|g| g.0 == k) {
                Some(g) => g.1.push(&e.name),
                None => groups.push((k, vec![&e.name])),
            }
        }
        if !groups.is_empty() {
            out += &format!("\n{}:\n\n", tr("Abweichende Angaben"));
        }
        for ((lic, author, src), names) in groups {
            let mut parts = vec![format!("{} {lic}", tr("Lizenz"))];
            if !author.is_empty() {
                parts.push(format!("{} {author}", tr("Autor")));
            }
            if let Some(s) = src {
                parts.push(format!("{} {s}", tr("Quelle")));
            }
            out += &format!("- {}: {}\n", names.join(", "), parts.join(" · "));
        }
    }
    out
}

/// Effekt-/Filter-/Übergangs-IDs der Clips, die der Katalog nicht kennt: (id, Clipname).
pub fn missing_in(packs: &[Pack], p: &Project) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for c in p.timeline.tracks.iter().flat_map(|t| t.clips.iter()) {
        let mut refs: Vec<(Kind, &str)> = Vec::new();
        match &c.content {
            ClipContent::Effect(r) => refs.push((Kind::Effect, &r.id)),
            ClipContent::Filter(r) => refs.push((Kind::Filter, &r.id)),
            _ => {}
        }
        refs.extend(c.fx.filter.iter().map(|f| (Kind::Filter, f.id.as_str())));
        refs.extend(c.fx.effects.iter().map(|e| (Kind::Effect, e.id.as_str())));
        refs.extend(c.transition_out.iter().map(|t| (Kind::Transition, t.id.as_str())));
        for (k, id) in refs {
            if get_in(packs, k, id).is_none() {
                out.push((id.to_string(), c.label(p)));
            }
        }
    }
    out
}

pub fn missing(p: &Project) -> Vec<(String, String)> {
    read(|packs| missing_in(packs, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::*;

    fn pack(enabled: bool) -> Pack {
        let info = PackInfo {
            id: "t.pack".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            license: "CC0-1.0".into(),
            author: "A".into(),
            source: None,
        };
        let e = |id: &str, kind, license: &str| Entry {
            id: format!("t.pack/{id}"),
            kind,
            name: id.into(),
            categories: vec!["Licht".into()],
            pack: "t.pack".into(),
            license: license.into(),
            author: "A".into(),
            source: None,
            tags: Vec::new(),
            payload: Payload::Ffmpeg("eq=brightness=0.1*{intensity}".into()),
        };
        Pack {
            info,
            enabled,
            entries: vec![
                e("glow", Kind::Effect, "CC0-1.0"),
                e("warm", Kind::Filter, "CC-BY-4.0"),
                e("hot", Kind::Filter, "CC-BY-4.0"),
            ],
        }
    }

    #[test]
    fn builtin_ids_unique_and_complete() {
        for k in Kind::ALL {
            let l = list_in(&[], k);
            let mut ids = std::collections::HashSet::new();
            assert!(l.iter().all(|e| ids.insert(e.id.clone())), "{k:?} doppelt");
            assert!(l.iter().all(|e| e.pack == STANDARD && e.payload == Payload::Builtin));
        }
        assert_eq!(list_in(&[], Kind::Effect).len(), crate::EFFECTS.len());
        assert_eq!(list_in(&[], Kind::Filter).len(), crate::FILTERS.len());
        assert_eq!(list_in(&[], Kind::Transition).len(), crate::TRANSITIONS.len());
        assert_eq!(list_in(&[], Kind::Sound).len(), crate::sfx::SFX.len());
        assert_eq!(list_in(&[], Kind::Sticker).len(), crate::stickers::STICKERS.len());
        // heutige Reihenfolge
        assert_eq!(list_in(&[], Kind::Effect)[0].id, crate::EFFECTS[0].id);
        let s = packs_in(&[pack(false)]);
        assert_eq!(s[0].info.id, STANDARD);
        assert_eq!(s[1].counts[&Kind::Filter], 2);
    }

    #[test]
    fn packs_follow_builtin_and_switch_off() {
        let on = [pack(true)];
        let l = list_in(&on, Kind::Filter);
        assert_eq!(l.len(), crate::FILTERS.len() + 2);
        assert_eq!(l[crate::FILTERS.len()].id, "t.pack/warm");
        assert!(get_in(&on, Kind::Effect, "t.pack/glow").is_some());
        assert!(get_in(&on, Kind::Filter, "t.pack/glow").is_none(), "Art zählt");
        let off = [pack(false)];
        assert_eq!(list_in(&off, Kind::Filter).len(), crate::FILTERS.len());
        assert!(get_in(&off, Kind::Effect, "t.pack/glow").is_none());
    }

    #[test]
    fn credits_group_deviating_assets() {
        let c = credits(&[pack(true)], &|s| s.to_string());
        assert!(c.contains("## Grundausstattung (cutline.standard)"), "{c}");
        assert!(c.contains("## Test (t.pack)\n\n- Lizenz: CC0-1.0\n- Autor: A\n"), "{c}");
        assert!(c.contains("- warm, hot: Lizenz CC-BY-4.0 · Autor A\n"), "{c}");
        assert!(!c.contains("glow"), "Angaben wie das Paket: nicht einzeln");
    }

    #[test]
    fn missing_lists_unknown_effects_and_filters() {
        let mut p = Project::new("x");
        let mut c = Clip::new(ClipContent::Effect(FxRef::new("t.pack/glow")), Time::ZERO, Time::from_secs_f64(1.0));
        c.fx.filter = Some(FxRef::new("noir"));
        let mut d = Clip::new(ClipContent::Filter(FxRef::new("weg/filter")), Time::ZERO, Time::from_secs_f64(1.0));
        d.fx.effects.push(FxRef::new("blur"));
        let mut t = Track::new(TrackKind::Effect);
        t.clips = vec![c, d];
        p.timeline.tracks.push(t);
        let m = missing_in(&[pack(false)], &p);
        assert_eq!(
            m,
            vec![
                ("t.pack/glow".to_string(), "t.pack/glow".to_string()),
                ("weg/filter".to_string(), "weg/filter".to_string())
            ]
        );
        assert_eq!(missing_in(&[pack(true)], &p).len(), 1);
    }

    fn shader_pack(enabled: bool) -> Pack {
        let mut p = pack(enabled);
        p.entries = vec![Entry {
            id: "t.pack/swirl".into(),
            kind: Kind::Transition,
            name: "Wirbel".into(),
            categories: vec!["Effekt".into()],
            pack: "t.pack".into(),
            license: "MIT".into(),
            author: "A".into(),
            source: None,
            tags: Vec::new(),
            payload: Payload::Shader {
                file: PathBuf::from("swirl.wgsl"),
                fallback: "radial".into(),
                params: vec![1.0],
            },
        }];
        p
    }

    #[test]
    fn shader_transitions_resolve_with_fallback() {
        let on = [shader_pack(true)];
        let plan = transition_plan_in(&on, "t.pack/swirl");
        assert_eq!(plan.xfade, "radial");
        assert_eq!(plan.shader, Some((PathBuf::from("swirl.wgsl"), vec![1.0])));
        assert_eq!(list_in(&on, Kind::Transition).len(), crate::TRANSITIONS.len() + 1);
        // Paket aus: Rückfall ohne Shader; Paket fehlt: Überblenden
        let off = [shader_pack(false)];
        assert_eq!(transition_plan_in(&off, "t.pack/swirl"), TransitionPlan { xfade: "radial", shader: None });
        assert_eq!(transition_plan_in(&[], "t.pack/swirl").xfade, "fade");
        assert_eq!(transition_plan_in(&[], "wipeleft"), TransitionPlan { xfade: "wipeleft", shader: None });
        // eingeschleuster Text wird nie zum Filternamen
        assert_eq!(transition_plan_in(&[], "fade:x;movie").xfade, "fade");
        // unbekannter Rückfall im Paket → Überblenden
        let mut bad = shader_pack(true);
        if let Payload::Shader { fallback, .. } = &mut bad.entries[0].payload {
            *fallback = "gibtsnicht".into();
        }
        assert_eq!(transition_plan_in(&[bad], "t.pack/swirl").xfade, "fade");
        // fehlende Übergänge werden gemeldet
        let mut p = Project::new("x");
        let mut c = Clip::new(ClipContent::Effect(FxRef::new("blur")), Time::ZERO, Time::from_secs_f64(1.0));
        c.transition_out = Some(TransitionRef { id: "t.pack/swirl".into(), duration: Time::from_secs_f64(0.5) });
        let mut t = Track::new(TrackKind::Effect);
        t.clips = vec![c];
        p.timeline.tracks.push(t);
        assert_eq!(missing_in(&on, &p).len(), 0);
        assert_eq!(missing_in(&off, &p).len(), 1);
    }
}
