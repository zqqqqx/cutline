//! Karaoke subtitles: which word is current, and what split, trim, speed and text edits
//! do to the word times.

use cl_core::*;

fn s(x: f64) -> Time {
    Time::from_secs_f64(x)
}

fn ms(v: &[Time]) -> Vec<i64> {
    v.iter().map(|t| t.as_millis()).collect()
}

fn karaoke(text: &str, words: &[f64]) -> TextStyle {
    TextStyle {
        text: text.into(),
        karaoke: Some(Karaoke { color: [255, 221, 0, 255], fill: false, words: words.iter().map(|w| s(*w)).collect() }),
        ..Default::default()
    }
}

#[test]
fn word_at_time_with_stored_times_and_at_the_edges() {
    let st = [s(0.5), s(1.0), s(2.0)];
    assert_eq!(karaoke_word(&st, s(0.0)), None, "before the first word");
    assert_eq!(karaoke_word(&st, s(0.5)), Some(0));
    assert_eq!(karaoke_word(&st, s(0.99)), Some(0));
    assert_eq!(karaoke_word(&st, s(1.5)), Some(1), "speech pause: the last word stays");
    assert_eq!(karaoke_word(&st, s(2.0)), Some(2));
    assert_eq!(karaoke_word(&st, s(99.0)), Some(2), "after the last word");
    assert_eq!(karaoke_word(&[], s(1.0)), None);
    let k = karaoke("one two three", &[0.5, 1.0, 2.0]);
    assert_eq!(k.karaoke_lit(s(3.0), s(1.2)), Some(1..2));
    let mut fill = k.clone();
    fill.karaoke.as_mut().unwrap().fill = true;
    assert_eq!(fill.karaoke_lit(s(3.0), s(1.2)), Some(0..2), "fill: spoken words stay");
    assert_eq!(TextStyle::default().karaoke_lit(s(3.0), s(1.2)), None);
}

#[test]
fn even_distribution_by_characters() {
    // without times (by hand, SRT): weight = characters + 1
    let k = Karaoke { color: [0; 4], fill: false, words: vec![] };
    assert_eq!(ms(&k.starts("ab abcd a", s(1.2))), [0, 360, 960]);
    assert_eq!(k.starts("", s(1.0)), Vec::<Time>::new());
    // different word count than stored (text changed): even
    let k2 = Karaoke { words: vec![s(0.1), s(0.2)], ..k.clone() };
    assert_eq!(ms(&k2.starts("ab abcd a", s(1.2))), [0, 360, 960]);
    // same word count (word corrected): times stay
    assert_eq!(ms(&k2.starts("Hallo Welt", s(1.2))), [100, 200]);
    assert_eq!(word_ranges(" Hallo  schöne\nWelt "), [1..6, 8..15, 16..20]);
}

fn doc_with(st: TextStyle, start: f64, dur: f64) -> (Document, ClipId) {
    let mut d = Document::new(Project::new("k"));
    let OpOutput::Clip(id) = d
        .apply(EditOp::AddClip { track: None, clip: Clip::new(ClipContent::Text(st), s(start), s(dur)) }, OpSource::Ui)
        .unwrap()
    else {
        panic!()
    };
    (d, id)
}

fn words(d: &Document, id: ClipId) -> Vec<i64> {
    match &d.project().find_clip(id).unwrap().1.content {
        ClipContent::Text(TextStyle { karaoke: Some(k), .. }) => ms(&k.words),
        _ => panic!(),
    }
}

#[test]
fn split_keeps_times_with_the_right_halves() {
    let (mut d, id) = doc_with(karaoke("a b c d", &[0.0, 1.0, 2.0, 3.0]), 10.0, 4.0);
    let OpOutput::Clip(r) = d.apply(EditOp::SplitClip { clip: id, at: s(12.5) }, OpSource::Ui).unwrap() else { panic!() };
    assert_eq!(words(&d, id), [0, 1000, 2000, 3000]);
    assert_eq!(words(&d, r), [-2500, -1500, -500, 500]);
    // the highlight continues across the cut: on the right „c“ at the start
    let rc = d.project().find_clip(r).unwrap().1.clone();
    let ClipContent::Text(st) = &rc.content else { panic!() };
    assert_eq!(st.karaoke_lit(rc.duration, Time::ZERO), Some(2..3));
    // evenly spread words are pinned with the whole length
    let (mut d, id) = doc_with(karaoke("a b c d", &[]), 0.0, 4.0);
    let OpOutput::Clip(r) = d.apply(EditOp::SplitClip { clip: id, at: s(1.0) }, OpSource::Ui).unwrap() else { panic!() };
    assert_eq!(words(&d, id), [0, 1000, 2000, 3000]);
    assert_eq!(words(&d, r), [-1000, 0, 1000, 2000]);
}

#[test]
fn trim_and_tempo_move_the_times() {
    let (mut d, id) = doc_with(karaoke("a b c", &[0.5, 1.0, 2.0]), 0.0, 3.0);
    d.apply(EditOp::TrimClip { clip: id, edge: Edge::Start, to: s(0.8) }, OpSource::Ui).unwrap();
    assert_eq!(words(&d, id), [-300, 200, 1200], "words stay in place on the timeline");
    d.apply(EditOp::TrimClip { clip: id, edge: Edge::End, to: s(2.5) }, OpSource::Ui).unwrap();
    assert_eq!(words(&d, id), [-300, 200, 1200]);
    d.apply(EditOp::SetSpeed { clip: id, speed: 2.0 }, OpSource::Ui).unwrap();
    assert_eq!(words(&d, id), [-150, 100, 600]);
    // moving changes nothing (relative to the clip start)
    d.apply(EditOp::MoveClip { clip: id, track: None, start: s(5.0) }, OpSource::Ui).unwrap();
    assert_eq!(words(&d, id), [-150, 100, 600]);
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(words(&d, id), [-300, 200, 1200]);
}

#[test]
fn files_without_karaoke_stay_unchanged() {
    let (d, _) = doc_with(TextStyle { text: "Hello".into(), ..Default::default() }, 0.0, 1.0);
    let json = d.project().to_json();
    assert!(!json.contains("karaoke"), "no new field without karaoke");
    let p = Project::from_json(&json).unwrap();
    assert_eq!(p.to_json(), json);
    // karaoke without times and without fill: color only
    let (d, _) = doc_with(karaoke("a", &[]), 0.0, 1.0);
    let json = d.project().to_json();
    assert!(json.contains("karaoke") && !json.contains("words") && !json.contains("fill"), "{json}");
    assert_eq!(Project::from_json(&json).unwrap(), *d.project());
}
