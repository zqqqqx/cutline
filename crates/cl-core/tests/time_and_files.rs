//! Time arithmetic, keyframes/curves at the edges, speed there and back, and loading
//! broken, old and foreign project files.

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
    // bug: addition overflowed with huge values (release: negative, debug: crash)
    assert_eq!(Time(i64::MAX) + Time(1), Time(i64::MAX));
    assert_eq!(Time(i64::MIN) - Time(1), Time(i64::MIN));
    assert_eq!(Time(5) - Time(7), Time(-2));
    assert_eq!(format!("{}", Time::from_secs_f64(1.5)), "1.500s");
    // 100 hours stay exact to the microsecond
    let big = Time(360_000 * 1_000_000 + 1);
    assert_eq!(Time::from_secs_f64(big.as_secs_f64()).0 - big.0, 0);
}

#[test]
fn ntsc_rates_timecode_and_snapping() {
    for (s_, num, den) in [("23.976", 24000, 1001), ("29.97", 30000, 1001), ("59.94", 60000, 1001), ("23.98", 24000, 1001), ("47.952", 48000, 1001)] {
        assert_eq!(Rational::parse(s_), Some(Rational::new(num, den)), "{s_}");
    }
    // bug: 12.5 fps became 12513/1001 (= 12.5005)
    assert_eq!(Rational::parse("12.5"), Some(Rational::new(12500, 1000)));
    assert_eq!(Rational::parse(" 25 "), Some(Rational::new(25, 1)));
    assert_eq!(Rational::parse("24000/1001"), Some(Rational::new(24000, 1001)));
    // bug: 0, negative, NaN and infinity were accepted as frame rate
    for bad in ["0", "-30", "NaN", "inf", "1e300", "0/1", "30/0", "", "abc", "30/", "/1", "-1/2", "0.0000001"] {
        assert_eq!(Rational::parse(bad), None, "{bad}");
    }
    for fps in [Rational::new(24000, 1001), Rational::new(30000, 1001), Rational::new(60000, 1001), Rational::new(25, 1)] {
        let f = fps.as_f64();
        let mut last = String::new();
        for i in 0..2000 {
            let t = Time::from_secs_f64(i as f64 * 0.0371);
            let snapped = t.snap_to_frame(fps);
            assert_eq!(snapped.snap_to_frame(fps), snapped, "snapping is idempotent");
            assert!((snapped.as_secs_f64() - t.as_secs_f64()).abs() <= 0.5 / f + 1e-6);
            let tc = t.timecode(fps);
            let frames: u32 = tc[9..].parse().unwrap();
            assert!(frames < f.ceil() as u32, "{tc} at {f}");
            assert!(tc >= last, "timecode runs backwards: {last} → {tc}");
            last = tc;
        }
    }
    assert_eq!(Time::from_secs_f64(-5.0).timecode(Rational::new(30, 1)), "00:00:00:00");
    assert!(Time(i64::MAX).timecode(Rational::new(30, 1)).len() > 11);
    // bug: frame rate 0 (broken file) pulled every time to 0
    assert_eq!(s(3.3).snap_to_frame(Rational::new(0, 1)), s(3.3));
    assert_eq!(Rational::new(30, 0).as_f64(), 30.0);
}

#[test]
fn keyframes_at_the_edges() {
    let mut k = Keyframes::default();
    assert!(k.is_empty() && k.times().is_empty());
    assert!(!k.remove(Prop::X, Time::ZERO));
    k.set(Prop::X, s(1.0), 5.0);
    // a single keyframe applies everywhere
    for t in [Time(i64::MIN), Time::ZERO, s(1.0), Time(i64::MAX)] {
        assert_eq!(k.eval(Prop::X, t), Some(5.0));
    }
    assert_eq!(k.eval(Prop::Y, s(1.0)), None);
    // replaced within 1 ms, otherwise new
    k.set(Prop::X, s(1.0) + Time(999), 6.0);
    assert_eq!(k.get(Prop::X).len(), 1);
    k.set(Prop::X, s(1.0) + Time(1000), 7.0);
    assert_eq!(k.get(Prop::X).len(), 2);
    // two keyframes 1 µs apart: no division by 0
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
                // monotonic over the whole length
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
    // beats: only in the used range, converted to timeline time
    let mut c = media_clip(&m, 2.0, 4.0);
    c.source_in = s(1.0);
    c.speed = 2.0;
    c.beats = vec![s(0.5), s(1.0), s(5.0), s(9.0), s(9.1)];
    assert_eq!(c.beat_times(), vec![s(2.0), s(4.0), s(6.0)]);
    c.speed = 0.0; // broken value: no division by 0
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
    // truncated at many places
    for cut in (1..json.len() - 1).step_by(41).filter(|i| json.is_char_boundary(*i)) {
        assert!(Project::from_json(&json[..cut]).is_err(), "truncated at {cut}");
    }
    // wrong types and impossible numbers
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
    // number larger than i64
    let w = json.replacen(&format!("\"schema_version\": {}", cl_core::PROJECT_SCHEMA_VERSION), "\"schema_version\": 99999999999999999999999", 1);
    assert_ne!(w, json);
    assert!(Project::from_json(&w).is_err());
}

#[test]
fn unknown_fields_and_old_files_still_load() {
    let json = full_project_json();
    let p = Project::from_json(&json).unwrap();
    // unknown fields (newer version) everywhere
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
    // exceptions: keyframe lists are maps with fixed keys (Prop), Adjust.hsl is an array
    let mut v2: serde_json::Value = serde_json::from_str(&json).unwrap();
    v2["zukunft"] = 1.into();
    v2["settings"]["zukunft"] = 1.into();
    v2["timeline"]["tracks"][0]["zukunft"] = 1.into();
    v2["timeline"]["tracks"][0]["clips"][0]["zukunft"] = 1.into();
    assert_eq!(Project::from_json(&v2.to_string()).unwrap(), p);
    // a file from a newer version is refused instead of losing its new fields on the next save
    v2["schema_version"] = (cl_core::PROJECT_SCHEMA_VERSION + 1).into();
    let e = Project::from_json(&v2.to_string()).unwrap_err();
    assert_eq!(e.to_string(), cl_core::NEWER_PROJECT);
    v2["timeline"] = "neu".into();
    assert_eq!(Project::from_json(&v2.to_string()).unwrap_err().to_string(), cl_core::NEWER_PROJECT);
    // unknown keys everywhere, also next to enum tags (clip content `{"Media": …}`): today refused
    // with a position error; if it ever loads, nothing of the project may be lost
    match Project::from_json(&v.to_string()) {
        Ok(q) => assert_eq!(q, p),
        Err(e) => assert!(e.line() > 0 && e.column() > 0, "{e}"),
    }
    // old files: all fields with default value are missing
    let mut old: serde_json::Value = serde_json::from_str(&json).unwrap();
    for k in ["ripple_all_tracks", "platform", "channels"] {
        old["settings"].as_object_mut().unwrap().remove(k);
    }
    old["timeline"].as_object_mut().unwrap().remove("markers");
    let media = old["media"].as_array_mut().unwrap();
    assert!(!media.is_empty());
    for m in media {
        m.as_object_mut().unwrap().remove("audio_streams");
    }
    for t in old["timeline"]["tracks"].as_array_mut().unwrap() {
        t.as_object_mut().unwrap().remove("name");
        for c in t["clips"].as_array_mut().unwrap() {
            let c = c.as_object_mut().unwrap();
            for k in ["speed", "disabled", "fx", "keyframes", "anim", "transition_out", "fade_in", "fade_out", "audio", "audio_stream", "beats", "label", "group"] {
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
    // a loaded old file can still be edited
    let mut d = Document::new(p_old);
    let id = d.project().timeline.tracks[0].clips[0].id;
    apply(&mut d, EditOp::SplitClip { clip: id, at: Time(1) });
    check(d.project(), "old file");
}

/// Adjustable shadow (format 4): a file without `shadow_style` loads with the fixed default look
/// and is saved without the field; a set shadow survives saving; offsets are clamped when drawn.
#[test]
fn text_shadow_style_round_trips_and_old_files_keep_the_default() {
    let cur = format!("\"schema_version\": {PROJECT_SCHEMA_VERSION}");
    let text = |st: TextStyle| {
        let mut p = Project::new("S");
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(ClipContent::Text(st), Time::ZERO, Time::from_secs_f64(1.0)));
        p.timeline.tracks.push(t);
        p
    };
    let style = |p: &Project| match &p.timeline.tracks[0].clips[0].content {
        ClipContent::Text(st) => st.clone(),
        _ => unreachable!(),
    };
    // old file (version 3, no field): default look, nothing new written
    let json = text(TextStyle::default()).to_json();
    assert!(!json.contains("shadow_style"), "{json}");
    assert!(json.contains(&cur), "{json}");
    let old = json.replace(&cur, "\"schema_version\": 3");
    let p = Project::from_json(&old).unwrap();
    assert_eq!(style(&p).shadow_style, None);
    assert_eq!(p.to_json(), json);
    // set shadow: survives saving and loading
    let sh = Shadow { color: [255, 0, 0, 200], dx: 12.0, dy: -8.0, blur: 0.0 };
    let p = text(TextStyle { shadow_style: Some(sh), ..Default::default() });
    let json = p.to_json();
    assert!(json.contains("shadow_style"), "{json}");
    assert_eq!(style(&Project::from_json(&json).unwrap()).shadow_style, Some(sh));
    // the default is the look before the shadow was adjustable; offsets whole and within ±100 px
    assert_eq!(Shadow::default(), Shadow { color: [0, 0, 0, 153], dx: 3.0, dy: 3.0, blur: 0.0 });
    assert_eq!(Shadow::default().offset(), (3, 3));
    let off = |dx: f32, dy: f32| Shadow { dx, dy, ..Default::default() }.offset();
    assert_eq!(off(2.5, -2.5), (3, -3));
    assert_eq!(off(1e6, -1e6), (100, -100));
    assert_eq!(off(f32::NAN, f32::INFINITY), (0, 100));
    // inspector: 135° = light top left, shadow down right; distance and angle there and back
    let (d, a) = Shadow::default().polar();
    assert!((d - 18f32.sqrt()).abs() < 1e-4 && (a - 135.0).abs() < 1e-3, "{d} {a}");
    for (dist, deg, want) in [(10.0, 0.0, (-10, 0)), (10.0, 90.0, (0, 10)), (10.0, 270.0, (0, -10)), (20.0, 315.0, (-14, -14))] {
        let mut s = Shadow::default();
        s.set_polar(dist, deg);
        assert_eq!(s.offset(), want, "{deg}°");
        let (d, a) = s.polar();
        assert!((d - dist).abs() < 1e-3 && (a - deg).abs() < 1e-3, "{deg}°: {d} {a}");
    }
}

/// Bug: a URL as media path went to FFmpeg (network) and did not count as missing on relink.
#[test]
fn only_local_files_count_as_media() {
    let mut m: MediaRef = serde_json::from_value(serde_json::json!({
        "id": MediaId::new(), "path": "http://example.invalid/a.mp4", "kind": "Video", "duration": 1000000,
        "width": 2, "height": 2, "fps": null, "has_audio": false, "video_codec": null, "audio_codec": null
    })).unwrap();
    assert!(!m.is_local());
    m.path = std::env::current_exe().unwrap();
    assert!(m.is_local());
}

/// Bug: SetSettings accepted width 0 or frame rate 0 (apply_ops, project file) – the export failed.
#[test]
fn invalid_settings_are_rejected_and_repaired_on_load() {
    let mut d = Document::new(Project::new("T"));
    let mut s = d.project().settings.clone();
    s.width = 0;
    assert!(d.apply(EditOp::SetSettings { settings: s.clone() }, OpSource::Ui).is_err());
    s.width = 1280;
    s.fps = Rational::new(0, 1);
    assert!(d.apply(EditOp::SetSettings { settings: s.clone() }, OpSource::Ui).is_err());
    let mut p = Project::new("T");
    p.settings.width = 0;
    p.settings.fps = Rational::new(0, 1);
    let back = Project::from_json(&p.to_json()).unwrap();
    assert!(back.settings.valid());
    assert_eq!(back.settings.height, 1080, "valid values stay");
}

/// A clip with a duration of 0 or less (hand edit, foreign tool) is dropped on load and broken
/// start times are clamped; editing the rest must not panic (fades clamp to half the length).
#[test]
fn clips_with_impossible_times_are_dropped_on_load() {
    let mut v: serde_json::Value = serde_json::from_str(&full_project_json()).unwrap();
    let clips = v["timeline"]["tracks"][0]["clips"].as_array_mut().unwrap();
    let n = clips.len();
    assert!(n >= 2);
    clips[0]["duration"] = (-5_000_000).into();
    clips[1]["start"] = (-7).into();
    clips[1]["source_in"] = (-3).into();
    let p = Project::from_json(&v.to_string()).unwrap();
    let t = &p.timeline.tracks[0];
    assert_eq!(t.clips.len(), n - 1);
    assert!(t.clips.iter().all(|c| c.duration > Time::ZERO && c.start >= Time::ZERO && c.source_in >= Time::ZERO));
    let mut d = Document::new(p);
    for c in d.project().timeline.tracks[0].clips.clone() {
        apply(&mut d, EditOp::SetFades { clip: c.id, fade_in: Time::from_secs_f64(1.0), fade_out: Time::from_secs_f64(1.0) });
    }
}

/// Soft shadow (format 5): a hard shadow is written exactly as 0.17.0 wrote it (no `blur`), a
/// file without `blur` loads hard, a soft one survives saving; the softness is clamped.
#[test]
fn soft_shadow_round_trips_and_hard_shadows_stay_as_before() {
    assert!(PROJECT_SCHEMA_VERSION >= 5);
    // as 0.17.0 wrote a set shadow
    let v4 = r#"{"color":[255,0,0,200],"dx":12.0,"dy":-8.0}"#;
    let hard: Shadow = serde_json::from_str(v4).unwrap();
    assert_eq!(hard.blur, 0.0);
    assert_eq!(serde_json::to_string(&hard).unwrap(), v4);
    let soft = Shadow { blur: 6.0, ..hard };
    let json = serde_json::to_string(&soft).unwrap();
    assert!(json.contains("\"blur\":6.0"), "{json}");
    assert_eq!(serde_json::from_str::<Shadow>(&json).unwrap(), soft);
    // sigma = softness / 2, softness within 0–40
    let sigma = |blur: f32| Shadow { blur, ..hard }.sigma();
    assert_eq!((sigma(6.0), sigma(0.0), sigma(-3.0), sigma(1e9), sigma(f32::NAN)), (3.0, 0.0, 0.0, 20.0, 0.0));
}

/// Mask range (format 6): a clip without one is written as before (no `cutout_span`), a set
/// range survives saving, and `keep_bound` keeps it with the mask.
#[test]
fn mask_range_round_trips_and_old_clips_stay_as_before() {
    assert_eq!(PROJECT_SCHEMA_VERSION, 6);
    let plain = ClipFx { cutout: Some("m.mkv".into()), ..Default::default() };
    let json = serde_json::to_string(&plain).unwrap();
    assert!(!json.contains("cutout_span"), "{json}");
    let span = MatteSpan { start: Time::from_secs_f64(1.0), end: Time::from_secs_f64(2.5) };
    let set = ClipFx { cutout_span: Some(span), ..plain.clone() };
    let json = serde_json::to_string(&set).unwrap();
    assert_eq!(serde_json::from_str::<ClipFx>(&json).unwrap(), set);
    let mut look = ClipFx::default();
    look.keep_bound(&set);
    assert_eq!((look.cutout, look.cutout_span), (set.cutout, Some(span)));
}

/// Object marks (format 6) live on the clip: absent from the file when unused, a set one survives
/// saving with its frame and background points; background points alone count as nothing marked.
#[test]
fn object_marks_round_trip_on_the_clip() {
    let mut c = Clip::new(ClipContent::Text(Default::default()), Time::ZERO, Time::from_secs_f64(1.0));
    assert!(!serde_json::to_string(&c).unwrap().contains("roto"));
    let key = RotoKey {
        t: Time::from_secs_f64(0.5),
        points: vec![RotoPoint { x: 0.25, y: 0.5, remove: false }, RotoPoint { x: 0.9, y: 0.1, remove: true }],
        rect: Some([0.1, 0.2, 0.6, 0.8]),
    };
    c.roto = Some(Roto { keys: vec![key.clone()] });
    let json = serde_json::to_string(&c).unwrap();
    assert_eq!(serde_json::from_str::<Clip>(&json).unwrap().roto, c.roto);
    // the marks do not touch the effects (their fingerprint keys the preview)
    assert_eq!(c.fx, ClipFx::default());
    // an object point written without `remove`
    assert!(json.contains(r#"{"x":0.25,"y":0.5}"#), "{json}");
}
