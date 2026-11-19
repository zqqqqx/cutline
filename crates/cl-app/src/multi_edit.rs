//! Mehrere Clips gleichzeitig bearbeiten: Änderungen am angezeigten Clip werden Feld für Feld
//! auf die übrigen ausgewählten Clips übertragen (nur die geänderten Felder, nur wo es sie gibt).

use cl_core::*;
use serde_json::Value;

/// Felder, die je Clip eigen bleiben (Lage, Länge, Quelle, Zugehörigkeit).
const OWN: &[&str] = &[
    "id",
    "start",
    "duration",
    "source_in",
    "group",
    "label",
    "transition_out",
    "keyframes",
];

/// Geänderte Blätter von `a` nach `b` (Objekte werden durchlaufen, Listen und Werte als Ganzes).
fn diff(a: &Value, b: &Value, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, Value)>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, vb) in y {
                path.push(k.clone());
                match x.get(k) {
                    Some(va) => diff(va, vb, path, out),
                    None => out.push((path.clone(), vb.clone())),
                }
                path.pop();
            }
        }
        _ if a != b => out.push((path.clone(), b.clone())),
        _ => {}
    }
}

/// Wert an `path` setzen, wenn der Elternknoten existiert (legt nichts Neues an).
fn set_path(v: &mut Value, path: &[String], val: Value) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut cur = v;
    for k in parents {
        match cur.get_mut(k.as_str()) {
            Some(n) => cur = n,
            None => return false,
        }
    }
    match cur.as_object_mut() {
        Some(o) => {
            o.insert(last.clone(), val);
            true
        }
        None => false,
    }
}

/// Clip als JSON, auch mit leerem Zuschnitt (fehlt sonst in der Datei und ließe sich weder
/// vergleichen noch übertragen).
fn to_value(c: &Clip) -> serde_json::Result<Value> {
    let mut v = serde_json::to_value(c)?;
    if let Some(o) = v.as_object_mut() {
        o.entry("crop").or_insert(serde_json::to_value(Crop::default())?);
    }
    Ok(v)
}

/// Ersetzungen für die übrigen Clips in `selection`, die `orig → edited` am Haupt-Clip entsprechen.
pub fn propagate(p: &Project, orig: &Clip, edited: &Clip, selection: &[ClipId]) -> Vec<EditOp> {
    let others: Vec<&Clip> = selection
        .iter()
        .filter(|id| **id != orig.id)
        .filter_map(|id| p.find_clip(*id).map(|(_, c)| c))
        .collect();
    if others.is_empty() {
        return Vec::new();
    }
    let (Ok(a), Ok(b)) = (to_value(orig), to_value(edited)) else {
        return Vec::new();
    };
    let mut changes = Vec::new();
    diff(&a, &b, &mut Vec::new(), &mut changes);
    // Quelle eines Medienclips nie übertragen
    changes.retain(|(path, _)| {
        !path.first().is_some_and(|k| OWN.contains(&k.as_str()))
            && !(path.first().map(String::as_str) == Some("content")
                && path.get(1).map(String::as_str) == Some("Media"))
    });
    if changes.is_empty() {
        return Vec::new();
    }
    let mut ops = Vec::new();
    for o in others {
        let Ok(mut v) = to_value(o) else {
            continue;
        };
        let mut any = false;
        for (path, val) in &changes {
            any |= set_path(&mut v, path, val.clone());
        }
        if !any {
            continue;
        }
        if let Ok(n) = serde_json::from_value::<Clip>(v) {
            if n != *o {
                ops.push(EditOp::ReplaceClip { clip: n });
            }
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_and_effects_go_to_all_selected_clips_but_timing_stays() {
        let mut p = Project::new("t");
        let a = Clip::new(
            ClipContent::Text(TextStyle {
                text: "a".into(),
                ..Default::default()
            }),
            Time::ZERO,
            Time::from_secs_f64(2.0),
        );
        let b = Clip::new(
            ClipContent::Text(TextStyle {
                text: "b".into(),
                ..Default::default()
            }),
            Time::from_secs_f64(5.0),
            Time::from_secs_f64(3.0),
        );
        let mut t = Track::new(TrackKind::Text);
        t.clips = vec![a.clone(), b.clone()];
        p.timeline.tracks.push(t);
        let mut e = a.clone();
        e.transform.scale = 2.0;
        e.fx.effects.push(FxRef {
            id: "glow".into(),
            intensity: 1.0,
            params: Default::default(),
        });
        e.duration = Time::from_secs_f64(9.0);
        let ops = propagate(&p, &a, &e, &[a.id, b.id]);
        assert_eq!(ops.len(), 1);
        let EditOp::ReplaceClip { clip } = &ops[0] else {
            panic!()
        };
        assert_eq!(clip.id, b.id);
        assert_eq!(clip.transform.scale, 2.0);
        assert_eq!(clip.fx.effects.len(), 1);
        assert_eq!(clip.duration, b.duration, "Länge bleibt");
        assert_eq!(clip.start, b.start, "Lage bleibt");
        let ClipContent::Text(s) = &clip.content else {
            panic!()
        };
        assert_eq!(s.text, "b", "Text bleibt, wenn er nicht geändert wurde");
    }

    /// Zuschnitt fehlt im JSON, wenn er leer ist: Setzen, Ändern und Zurücksetzen gehen trotzdem
    /// auf die anderen ausgewählten Clips über.
    #[test]
    fn crop_goes_to_all_selected_clips() {
        let mut p = Project::new("t");
        let mk = |s: f64| Clip::new(ClipContent::Text(TextStyle::default()), Time::from_secs_f64(s), Time::from_secs_f64(1.0));
        let (mut a, mut b) = (mk(0.0), mk(2.0));
        a.crop.left = 0.2;
        let mut t = Track::new(TrackKind::Text);
        t.clips = vec![a.clone(), b.clone()];
        p.timeline.tracks.push(t);
        let bid = b.id;
        let other = |orig: &Clip, e: &Clip, p: &Project| match propagate(p, orig, e, &[orig.id, bid]).as_slice() {
            [EditOp::ReplaceClip { clip }] => clip.crop,
            o => panic!("{o:?}"),
        };
        // Ändern: b hat noch keinen Zuschnitt
        let mut e = a.clone();
        e.crop.top = 0.3;
        assert_eq!(other(&a, &e, &p), Crop { top: 0.3, ..Default::default() });
        // Zurücksetzen (Feld fällt aus dem JSON)
        b.crop = Crop { left: 0.5, ..Default::default() };
        p.timeline.tracks[0].clips[1] = b.clone();
        let mut e = a.clone();
        e.crop = Crop::default();
        assert_eq!(other(&a, &e, &p), Crop::default());
    }
}
