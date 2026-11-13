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
    let (Ok(a), Ok(b)) = (serde_json::to_value(orig), serde_json::to_value(edited)) else {
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
        let Ok(mut v) = serde_json::to_value(o) else {
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
}
