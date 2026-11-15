//! Zeitrechnung, Keyframes/Kurven an den Rändern, Tempo hin und zurück und das Laden
//! kaputter, alter und fremder Projektdateien.

mod common;
use cl_core::*;
use common::*;

#[test]
fn time_rounding_saturation_and_conversions() {
    assert_eq!(Time::from_secs_f64(0.000_000_4), Time(0));
    assert_eq!(Time::from_secs_f64(0.000_000_5), Time(1));
    assert_eq!(Time::from_secs_f64(-0.000_000_5), Time(-1));
    assert_eq!(Time::from_secs_f64(f64::NAN), Time(0));
    assert_eq!(Time::from_secs_f64(1e300), Time(i64::MAX));
    assert_eq!(Time::from_secs_f64(-1e300), Time(i64::MIN));
    assert_eq!(Time::from_millis(1500).as_millis(), 1500);
    assert_eq!(Time(-1999).as_millis(), -1);
    // Fehler: Addition lief bei riesigen Werten über (Release: negativ, Debug: Absturz)
    assert_eq!(Time(i64::MAX) + Time(1), Time(i64::MAX));
    assert_eq!(Time(i64::MIN) - Time(1), Time(i64::MIN));
    assert_eq!(Time(5) - Time(7), Time(-2));
    assert_eq!(format!("{}", Time::from_secs_f64(1.5)), "1.500s");
    // 100 Stunden bleiben auf die Mikrosekunde genau
    let big = Time(360_000 * 1_000_000 + 1);
    assert_eq!(Time::from_secs_f64(big.as_secs_f64()).0 - big.0, 0);
}

#[test]
fn ntsc_rates_timecode_and_snapping() {
    for (s_, num, den) in [("23.976", 24000, 1001), ("29.97", 30000, 1001), ("59.94", 60000, 1001), ("23.98", 24000, 1001), ("47.952", 48000, 1001)] {
        assert_eq!(Rational::parse(s_), Some(Rational::new(num, den)), "{s_}");
    }
    // Fehler: 12,5 fps wurde zu 12513/1001 (= 12,5005)
    assert_eq!(Rational::parse("12.5"), Some(Rational::new(12500, 1000)));
    assert_eq!(Rational::parse(" 25 "), Some(Rational::new(25, 1)));
    assert_eq!(Rational::parse("24000/1001"), Some(Rational::new(24000, 1001)));
    // Fehler: 0, negative, NaN und Unendlich wurden als Bildrate angenommen
    for bad in ["0", "-30", "NaN", "inf", "1e300", "0/1", "30/0", "", "abc", "30/", "/1", "-1/2", "0.0000001"] {
        assert_eq!(Rational::parse(bad), None, "{bad}");
    }
    for fps in [Rational::new(24000, 1001), Rational::new(30000, 1001), Rational::new(60000, 1001), Rational::new(25, 1)] {
        let f = fps.as_f64();
        let mut last = String::new();
        for i in 0..2000 {
            let t = Time::from_secs_f64(i as f64 * 0.0371);
            let snapped = t.snap_to_frame(fps);
            assert_eq!(snapped.snap_to_frame(fps), snapped, "Einrasten ist idempotent");
            assert!((snapped.as_secs_f64() - t.as_secs_f64()).abs() <= 0.5 / f + 1e-6);
            let tc = t.timecode(fps);
            let frames: u32 = tc[9..].parse().unwrap();
            assert!(frames < f.ceil() as u32, "{tc} bei {f}");
            assert!(tc >= last, "Zeitcode läuft rückwärts: {last} → {tc}");
            last = tc;
        }
    }
    assert_eq!(Time::from_secs_f64(-5.0).timecode(Rational::new(30, 1)), "00:00:00:00");
    assert_eq!(Time(i64::MAX).timecode(Rational::new(30, 1)).len() > 11, true);
    // Fehler: Bildrate 0 (kaputte Datei) zog jede Zeit auf 0
    assert_eq!(s(3.3).snap_to_frame(Rational::new(0, 1)), s(3.3));
    assert_eq!(Rational::new(30, 0).as_f64(), 30.0);
}

#[test]
fn keyframes_at_the_edges() {
    let mut k = Keyframes::default();
    assert!(k.is_empty() && k.times().is_empty());
    assert!(!k.remove(Prop::X, Time::ZERO));
    k.set(Prop::X, s(1.0), 5.0);
    // ein einziger Keyframe gilt überall
    for t in [Time(i64::MIN), Time::ZERO, s(1.0), Time(i64::MAX)] {
        assert_eq!(k.eval(Prop::X, t), Some(5.0));
    }
    assert_eq!(k.eval(Prop::Y, s(1.0)), None);
    // innerhalb 1 ms ersetzt, sonst neu
    k.set(Prop::X, s(1.0) + Time(999), 6.0);
    assert_eq!(k.get(Prop::X).len(), 1);
    k.set(Prop::X, s(1.0) + Time(1000), 7.0);
    assert_eq!(k.get(Prop::X).len(), 2);
    // zwei Keyframes 1 µs auseinander: kein Teilen durch 0
    let mut k = Keyframes::default();
    k.0.insert(Prop::Scale, vec![
        Keyframe { t: s(1.0), v: 1.0, ease: Ease::Linear },
        Keyframe { t: s(1.0), v: 3.0, ease: Ease::Linear },
        Keyframe { t: s(2.0), v: 2.0, ease: Ease::Hold },
        Keyframe { t: s(3.0), v: 4.0, ease: Ease::Bezier(-5.0, 2.0, 9.0, -1.0) },
    ]);
    for i in -10..50 {
        let v = k.eval(Prop::Scale, s(i as f64 * 0.1)).unwrap();
        assert!(v.is_finite());
    }
    assert_eq!(k.eval(Prop::Scale, s(2.5)), Some(2.0), "Halten");
    assert_eq!(k.eval(Prop::Scale, s(9.0)), Some(4.0));
    for e in [Ease::Linear, Ease::EaseIn, Ease::EaseOut, Ease::EaseInOut, Ease::Bezier(0.3, -2.0, 0.7, 3.0)] {
        assert!(e.apply(0.0).abs() < 1e-3 && (e.apply(1.0) - 1.0).abs() < 1e-3, "{e:?}");
        assert_eq!(e.apply(-7.0), e.apply(0.0));
        assert_eq!(e.apply(7.0), e.apply(1.0));
    }
    k.shift(s(10.0));
    assert_eq!(k.get(Prop::Scale)[0].t, s(-9.0));
    assert_eq!(k.times().len(), 3, "gleiche Zeiten zusammengefasst");
}

#[test]
fn tempo_maps_source_and_timeline_both_ways() {
    let m = media(MediaKind::Video, 100.0);
    for speed in [0.1f32, 0.37, 1.0, 2.5, 100.0] {
        for reverse in [false, true] {
            for curve in [None, Some("montage"), Some("bullet"), Some("unbekannt")] {
                let mut c = media_clip(&m, 3.0, 4.0);
                c.source_in = s(10.0);
                c.speed = speed;
                c.fx.reverse = reverse;
                c.fx.speed_curve = curve.map(String::from);
                let span = c.source_span();
                let (a, b) = (c.source_time(c.start), c.source_time(c.end()));
                let near = |x: Time, y: Time| assert!((x.0 - y.0).abs() <= 2, "{speed} {reverse} {curve:?}: {x:?} vs {y:?}");
                if reverse {
                    near(a, c.source_in + span);
                    near(b, c.source_in);
                } else {
                    near(a, c.source_in);
                    near(b, c.source_in + span);
                }
                // monoton über die ganze Länge
                let mut last = a;
                for i in 1..=200 {
                    let t = c.start + Time(c.duration.0 * i / 200);
                    let st = c.source_time(t);
                    assert!(if reverse { st <= last } else { st >= last }, "{speed} {reverse} {curve:?}");
                    last = st;
                }
            }
        }
    }
    // Beats: nur im genutzten Bereich, in Zeitleistenzeit umgerechnet
    let mut c = media_clip(&m, 2.0, 4.0);
    c.source_in = s(1.0);
    c.speed = 2.0;
    c.beats = vec![s(0.5), s(1.0), s(5.0), s(9.0), s(9.1)];
    assert_eq!(c.beat_times(), vec![s(2.0), s(4.0), s(6.0)]);
    c.speed = 0.0; // kaputter Wert: kein Teilen durch 0
    assert!(c.beat_times().iter().all(|t| t.0 >= 0));
}

fn full_project_json() -> String {
    let r = rich();
    let mut d = r.doc;
    let mut k = Keyframes::default();
    k.set(Prop::Opacity, Time::ZERO, 0.0);
    apply(&mut d, EditOp::SetKeyframes { clip: r.v1, keyframes: k });
    let track = d.project().find_clip(r.a1).unwrap().0.id;
    apply(&mut d, EditOp::RenameTrack { track, name: Some("Ton".into()) });
    d.project().to_json()
}

#[test]
fn broken_project_files_give_errors_not_panics() {
    let json = full_project_json();
    for bad in ["", " ", "null", "[]", "{}", "\"Projekt\"", "{\"schema_version\":1}", "\u{feff}{}", "{\"name\":"] {
        assert!(Project::from_json(bad).is_err(), "{bad:?}");
    }
    // abgeschnitten an vielen Stellen
    for cut in (1..json.len() - 1).step_by(41).filter(|i| json.is_char_boundary(*i)) {
        assert!(Project::from_json(&json[..cut]).is_err(), "abgeschnitten bei {cut}");
    }
    // falsche Typen und unmögliche Zahlen
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let swaps: [(&str, serde_json::Value); 7] = [
        ("/timeline/tracks/0/clips/0/start", "zehn".into()),
        ("/timeline/tracks/0/clips/0/duration", serde_json::json!(1e30)),
        ("/timeline/tracks/0/clips/0/start", serde_json::json!(1.5)),
        ("/timeline/tracks/0/clips/0/opacity", serde_json::Value::Null),
        ("/settings/fps/num", serde_json::json!(-30)),
        ("/timeline/tracks/0/kind", "Hologramm".into()),
        ("/timeline/tracks/0/id", "keine-uuid".into()),
    ];
    for (ptr, val) in swaps {
        let mut w = v.clone();
        *w.pointer_mut(ptr).unwrap_or_else(|| panic!("{ptr}")) = val;
        assert!(Project::from_json(&w.to_string()).is_err(), "{ptr}");
    }
    // Zahl größer als i64
    let w = json.replacen("\"schema_version\": 1", "\"schema_version\": 99999999999999999999999", 1);
    assert!(Project::from_json(&w).is_err());
}

#[test]
fn unknown_fields_and_old_files_still_load() {
    let json = full_project_json();
    let p = Project::from_json(&json).unwrap();
    // unbekannte Felder (neuere Version) überall
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    fn add_unknown(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                for x in m.values_mut() {
                    add_unknown(x);
                }
                m.insert("zukunft_feld_🎬".into(), serde_json::json!({"x": [1, 2]}));
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(add_unknown),
            _ => {}
        }
    }
    add_unknown(&mut v);
    // Ausnahmen: Keyframe-Listen sind Maps mit festen Schlüsseln (Prop), Adjust.hsl ist ein Array
    let mut v2: serde_json::Value = serde_json::from_str(&json).unwrap();
    v2["zukunft"] = 1.into();
    v2["settings"]["zukunft"] = 1.into();
    v2["timeline"]["tracks"][0]["zukunft"] = 1.into();
    v2["timeline"]["tracks"][0]["clips"][0]["zukunft"] = 1.into();
    assert_eq!(Project::from_json(&v2.to_string()).unwrap(), p);
    let _ = Project::from_json(&v.to_string()); // darf scheitern, aber nicht abstürzen
    // Alte Dateien: alle Felder mit Standardwert fehlen
    let mut old: serde_json::Value = serde_json::from_str(&json).unwrap();
    for k in ["ripple_all_tracks", "platform", "channels"] {
        old["settings"].as_object_mut().unwrap().remove(k);
    }
    old["timeline"].as_object_mut().unwrap().remove("markers");
    for t in old["timeline"]["tracks"].as_array_mut().unwrap() {
        t.as_object_mut().unwrap().remove("name");
        for c in t["clips"].as_array_mut().unwrap() {
            let c = c.as_object_mut().unwrap();
            for k in ["speed", "disabled", "fx", "keyframes", "anim", "transition_out", "fade_in", "fade_out", "audio", "beats", "label", "group"] {
                c.remove(k);
            }
            if let Some(st) = c.get_mut("content").and_then(|x| x.get_mut("Text")).and_then(|x| x.as_object_mut()) {
                for k in ["outline", "background", "shadow", "line_spacing", "align"] {
                    st.remove(k);
                }
            }
        }
    }
    let p_old = Project::from_json(&old.to_string()).unwrap();
    assert!(p_old.settings.ripple_all_tracks && p_old.timeline.markers.is_empty());
    let c = &p_old.timeline.tracks[0].clips[0];
    assert_eq!((c.speed, c.disabled, c.fade_in), (1.0, false, Time::ZERO));
    let text = p_old.timeline.tracks.iter().flat_map(|t| &t.clips).find_map(|c| match &c.content {
        ClipContent::Text(st) => Some(st.clone()),
        _ => None,
    });
    assert!(text.unwrap().shadow, "alter Text bekommt Schatten-Standard");
    // geladene alte Datei lässt sich weiter bearbeiten
    let mut d = Document::new(p_old);
    let id = d.project().timeline.tracks[0].clips[0].id;
    apply(&mut d, EditOp::SplitClip { clip: id, at: Time(1) });
    check(d.project(), "alte Datei");
}
