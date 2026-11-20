//! One catalog for all content: built-in effects, filters, transitions, sounds and stickers
//! (package „Grundausstattung“, `cutline.standard`) and content from packages (`asset-pack`).
//!
//! Built-in entries stay in the code (IDs unchanged), packages are registered with
//! [`load_packs`]. Preview, export and the tabs read from here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cl_core::{ClipContent, Project};

/// Package ID of the built-in content.
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

/// What an entry brings along.
#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    /// Defined in code ([`crate::EFFECTS`], [`crate::sfx::SFX`] …)
    Builtin,
    /// FFmpeg chain with `{intensity}` (effect/filter from packages)
    Ffmpeg(String),
    /// File in the package folder (sound/sticker)
    File(PathBuf),
    /// Transition as a WGSL shader (cl-gpu) with a built-in `xfade` fallback and the
    /// default values of the parameters (`params.p0` …)
    Shader {
        file: PathBuf,
        fallback: String,
        params: Vec<f32>,
    },
}

/// How a transition is rendered.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionPlan {
    /// Built-in `xfade` name (always valid: fallback or `fade`)
    pub xfade: &'static str,
    /// Shader file and parameters if the package is enabled
    pub shader: Option<(PathBuf, Vec<f32>)>,
}

/// Resolve transition `id`: built-in → `xfade`; package transition → shader with fallback;
/// package off → only the fallback; unknown (package missing) → cross-fade.
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
    /// Built-in: as before (`blur`, `heart` …); package: `<package-id>/<asset-id>`
    pub id: String,
    pub kind: Kind,
    /// Name in the current language (built-in: German, the UI translates)
    pub name: String,
    pub categories: Vec<String>,
    pub pack: String,
    pub license: String,
    pub author: String,
    pub source: Option<String>,
    /// Tags (package manifest `tags`): the search in the tabs also finds the entry by them
    pub tags: Vec<String>,
    pub payload: Payload,
}

impl Entry {
    /// First category (folder in the tab)
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

/// Package with the number of entries per kind.
#[derive(Debug, Clone, PartialEq)]
pub struct PackSummary {
    pub info: PackInfo,
    pub enabled: bool,
    pub counts: BTreeMap<Kind, usize>,
}

static PACKS: std::sync::RwLock<Vec<Pack>> = std::sync::RwLock::new(Vec::new());

/// Reload package entries (replaces the previous ones; enabled or not).
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

/// Details of the base package.
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

/// Built-in ones first (current order), then enabled packages.
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

/// Entries for the tabs: like [`list_in`], without those of hidden packages (they stay enabled,
/// resolving by ID with [`get`] still works).
pub fn shown_in(packs: &[Pack], hidden: &[String], kind: Kind) -> Vec<Entry> {
    let mut v = list_in(packs, kind);
    v.retain(|e| !hidden.contains(&e.pack));
    v
}

/// Hidden package IDs (settings)
static HIDDEN: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

pub fn set_hidden(ids: Vec<String>) {
    if let Ok(mut g) = HIDDEN.write() {
        *g = ids;
    }
}

/// List for the tabs (without hidden packages).
pub fn list(kind: Kind) -> Vec<Entry> {
    let hidden = HIDDEN.read().map(|g| g.clone()).unwrap_or_default();
    read(|p| shown_in(p, &hidden, kind))
}

/// All entries of enabled packages, hidden ones too (resolving names).
pub fn list_all(kind: Kind) -> Vec<Entry> {
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

/// Name of a package entry, even with the package disabled (to name missing content readably).
pub fn name_any_in(packs: &[Pack], id: &str) -> Option<String> {
    packs.iter().flat_map(|p| p.entries.iter()).find(|e| e.id == id).map(|e| e.name.clone())
}

pub fn name_any(id: &str) -> Option<String> {
    read(|p| name_any_in(p, id))
}

/// FFmpeg chain of a package effect/filter with the strength filled in.
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

/// Base package first, then all loaded packages (disabled ones too).
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

/// Credits as Markdown: per package name, license, author, source; differing details
/// of individual items grouped. `tr` translates the fixed texts.
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
        // (license, author, source) → names, in order of first appearance
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

/// Effect/filter/transition IDs of clips the catalog does not know: (id, clip name).
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
        // current order
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
        assert!(get_in(&on, Kind::Filter, "t.pack/glow").is_none(), "kind counts");
        let off = [pack(false)];
        assert_eq!(list_in(&off, Kind::Filter).len(), crate::FILTERS.len());
        assert!(get_in(&off, Kind::Effect, "t.pack/glow").is_none());
    }

    #[test]
    fn hidden_packs_leave_tabs_but_still_resolve() {
        let on = [pack(true)];
        let hide = vec!["t.pack".to_string()];
        assert_eq!(shown_in(&on, &hide, Kind::Filter).len(), crate::FILTERS.len());
        assert_eq!(shown_in(&on, &[], Kind::Filter).len(), crate::FILTERS.len() + 2);
        assert!(get_in(&on, Kind::Effect, "t.pack/glow").is_some());
        assert_eq!(transition_plan_in(&[shader_pack(true)], "t.pack/swirl").xfade, "radial");
        let mut p = Project::new("x");
        let c = Clip::new(ClipContent::Effect(FxRef::new("t.pack/glow")), Time::ZERO, Time::from_secs_f64(1.0));
        let mut t = Track::new(TrackKind::Effect);
        t.clips = vec![c];
        p.timeline.tracks.push(t);
        assert!(missing_in(&on, &p).is_empty(), "hidden is not missing");
        // base package can be hidden: only package entries remain
        let std = vec![STANDARD.to_string()];
        let l = shown_in(&on, &std, Kind::Filter);
        assert_eq!(l.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["t.pack/warm", "t.pack/hot"]);
        assert!(shown_in(&[], &std, Kind::Sound).is_empty());
        assert!(get_in(&[], Kind::Sound, crate::sfx::SFX[0].id).is_some());
    }

    #[test]
    fn credits_group_deviating_assets() {
        let c = credits(&[pack(true)], &|s| s.to_string());
        assert!(c.contains("## Grundausstattung (cutline.standard)"), "{c}");
        assert!(c.contains("## Test (t.pack)\n\n- Lizenz: CC0-1.0\n- Autor: A\n"), "{c}");
        assert!(c.contains("- warm, hot: Lizenz CC-BY-4.0 · Autor A\n"), "{c}");
        assert!(!c.contains("glow"), "details as the package: not listed separately");
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
        // name even with the package disabled; only entirely missing packages stay at the id
        assert_eq!(name_any_in(&[pack(false)], "t.pack/warm").as_deref(), Some("warm"));
        assert_eq!(name_any_in(&[pack(false)], "weg/filter"), None);
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
        // package off: fallback without shader; package missing: cross-fade
        let off = [shader_pack(false)];
        assert_eq!(transition_plan_in(&off, "t.pack/swirl"), TransitionPlan { xfade: "radial", shader: None });
        assert_eq!(transition_plan_in(&[], "t.pack/swirl").xfade, "fade");
        assert_eq!(transition_plan_in(&[], "wipeleft"), TransitionPlan { xfade: "wipeleft", shader: None });
        // injected text never becomes a filter name
        assert_eq!(transition_plan_in(&[], "fade:x;movie").xfade, "fade");
        // unknown fallback in the package → cross-fade
        let mut bad = shader_pack(true);
        if let Payload::Shader { fallback, .. } = &mut bad.entries[0].payload {
            *fallback = "gibtsnicht".into();
        }
        assert_eq!(transition_plan_in(&[bad], "t.pack/swirl").xfade, "fade");
        // missing transitions are reported
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
