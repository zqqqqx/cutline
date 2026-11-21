//! Measurement tests (by hand, without a window): UI frame times when importing a large
//! CapCut draft, when opening a project with 500 media / 3000 clips and afterward, while
//! proxies, thumbnails and waveforms are being created.
//!
//! `cargo test --release -p cl-app -- --ignored --nocapture --test-threads 1 load_perf`
//!
//! Test material: `F:/cutline-testmedien/import` (or `CUTLINE_IMPORT_MEDIA`), created with
//! `gen.sh` and `mkdraft.py` from that folder; if it is missing, the tests skip themselves.

use super::*;

fn material() -> Option<PathBuf> {
    let d = std::env::var_os("CUTLINE_IMPORT_MEDIA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("F:/cutline-testmedien/import"));
    d.join("grosser_entwurf/draft_content.json").is_file().then_some(d)
}

#[derive(Default)]
struct Stats {
    frames: u32,
    sum: Duration,
    max: Duration,
    max_what: &'static str,
    over16: u32,
    over100: u32,
    by_section: HashMap<&'static str, Duration>,
}

impl Stats {
    fn add(&mut self, (d, what, secs): (Duration, &'static str, Vec<(&'static str, Duration)>)) {
        let mut known = Duration::ZERO;
        for (w, t) in secs {
            *self.by_section.entry(w).or_default() += t;
            known += t;
        }
        *self.by_section.entry("übrige Oberfläche").or_default() += d.saturating_sub(known);
        self.frames += 1;
        self.sum += d;
        if d > self.max {
            self.max = d;
            self.max_what = what;
        }
        self.over16 += (d > Duration::from_millis(16)) as u32;
        self.over100 += (d > Duration::from_millis(100)) as u32;
    }
    fn print(&self, what: &str) {
        println!(
            "{what}: {} Bilder, Ø {:.2} ms, max {:.1} ms ({}), >16 ms: {}, >100 ms: {}",
            self.frames,
            self.sum.as_secs_f64() * 1000.0 / self.frames.max(1) as f64,
            self.max.as_secs_f64() * 1000.0,
            self.max_what,
            self.over16,
            self.over100
        );
        let mut v: Vec<_> = self.by_section.iter().collect();
        v.sort_by_key(|(_, d)| std::cmp::Reverse(**d));
        let n = self.frames.max(1) as f64;
        let parts: Vec<String> = v.iter().take(6).map(|(w, d)| format!("{w} {:.2}", d.as_secs_f64() * 1000.0 / n)).collect();
        println!("    Ø je Bild in ms: {}", parts.join(", "));
    }
}

struct Headless {
    // holds the data folder and its lock for the whole measurement
    app: crate::saving::tests::TestApp,
    ctx: egui::Context,
    frame: eframe::Frame,
}

fn headless() -> Headless {
    std::env::set_var("CUTLINE_PERF", "1");
    assert!(perf(), "Mess-Tests einzeln laufen lassen (CUTLINE_PERF wird einmal gelesen)");
    let ctx = egui::Context::default();
    let app = crate::saving::tests::test_app_with(ctx.clone());
    Headless { app, ctx, frame: eframe::Frame::_new_kittest() }
}

impl Headless {
    /// One UI frame; returns: duration and the most expensive section.
    fn frame(&mut self) -> (Duration, &'static str, Vec<(&'static str, Duration)>) {
        // Keep the frame rate limiter out of the measurement
        std::thread::sleep(Duration::from_millis(17));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1600.0, 900.0))),
            ..Default::default()
        };
        let (app, frame) = (&mut *self.app, &mut self.frame);
        let _ = self.ctx.run(input, |ctx| eframe::App::update(app, ctx, frame));
        let (d, secs) = last_frame();
        (d, slowest_section(d, &secs).0, secs)
    }

    fn frames_for(&mut self, secs: f64, what: &str) {
        let mut st = Stats::default();
        let t = Instant::now();
        while t.elapsed().as_secs_f64() < secs {
            st.add(self.frame());
        }
        st.print(what);
    }
}

/// Measurement (see the module doc for material and command): `… -- --ignored --nocapture load_perf_import_big_draft`
#[test]
#[ignore]
fn load_perf_import_big_draft() {
    let Some(dir) = material() else { return };
    let mut h = headless();
    h.frame();
    let t = Instant::now();
    h.app.import.open = true;
    h.app.start_import(dir.join("grosser_entwurf"), &h.ctx.clone());
    let mut st = Stats::default();
    while h.app.import_busy() {
        st.add(h.frame());
        assert!(t.elapsed() < Duration::from_secs(600), "Import hängt");
    }
    println!("Import fertig: {:.1} s", t.elapsed().as_secs_f64());
    // Show the summary, then „Projekt öffnen“
    for _ in 0..5 {
        st.add(h.frame());
    }
    h.app.open_imported(0);
    while h.app.screen != Screen::Editor {
        st.add(h.frame());
    }
    println!("Import bis Editor: {:.1} s", t.elapsed().as_secs_f64());
    st.print("während Import");
    println!(
        "Projekt: {} Medien, {} Clips",
        h.app.project.media.len(),
        h.app.project.timeline.tracks.iter().map(|t| t.clips.len()).sum::<usize>()
    );
    h.frames_for(20.0, "20 s nach Import (Proxys/Vorschaubilder laufen)");
}

/// Project with 500 media and 3000 clips from the test material (checked in parallel).
fn big_project(dir: &Path) -> Project {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("mp4" | "png" | "wav")))
        .collect();
    files.sort();
    files.truncate(500);
    let media: Vec<MediaRef> = std::thread::scope(|s| {
        let hs: Vec<_> = files
            .chunks(files.len().div_ceil(8).max(1))
            .map(|c| s.spawn(move || c.iter().filter_map(|p| cl_media::probe(p).ok()).collect::<Vec<_>>()))
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let mut p = Project::new("Groß");
    for k in 0..10 {
        let kind = if k < 7 { TrackKind::Video } else { TrackKind::Audio };
        let mut t = cl_core::Track::new(kind);
        t.main = k == 0;
        let pool: Vec<&MediaRef> = media
            .iter()
            .filter(|m| (kind == TrackKind::Audio) == (m.kind == MediaKind::Audio))
            .collect();
        for i in 0..300 {
            let m = pool[(i * 7 + k * 13) % pool.len()];
            let dur = Time::from_secs_f64(1.0);
            t.clips.push(cl_core::Clip::new(
                ClipContent::Media { media: m.id, kind: m.kind },
                Time::from_secs_f64(i as f64),
                dur,
            ));
        }
        p.timeline.tracks.push(t);
    }
    p.media = media;
    p
}

/// Measurement (see the module doc for material and command): `… -- --ignored --nocapture load_perf_open_big_project`
#[test]
#[ignore]
fn load_perf_open_big_project() {
    // own project (copy, the original stays untouched): CUTLINE_OPEN_FILE=<.cutline>
    let own = std::env::var_os("CUTLINE_OPEN_FILE").filter(|s| !s.is_empty()).map(PathBuf::from);
    let Some(dir) = material().or_else(|| own.clone()) else { return };
    let mut h = headless();
    h.frame();
    let file = h.app.dir().join("gross.cutline");
    let p = match &own {
        Some(f) => {
            std::fs::copy(f, &file).unwrap();
            Project::from_json(&std::fs::read_to_string(&file).unwrap()).unwrap()
        }
        None => {
            let p = big_project(&dir);
            std::fs::write(&file, p.to_json()).unwrap();
            p
        }
    };
    println!(
        "Projekt: {} Medien, {} Clips, {} KB",
        p.media.len(),
        p.timeline.tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
        std::fs::metadata(&file).unwrap().len() / 1024
    );
    let t = Instant::now();
    h.app.open_project(&file);
    println!("open_project (UI-Thread): {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let mut st = Stats::default();
    while h.app.screen != Screen::Editor {
        st.add(h.frame());
        assert!(t.elapsed() < Duration::from_secs(60), "Öffnen hängt");
    }
    println!("bis Editor: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    st.print("bis geöffnet");
    let t = Instant::now();
    h.frame();
    println!("erstes Bild im Editor: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let secs = std::env::var("CUTLINE_PERF_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(20.0);
    h.frames_for(secs, "nach dem Öffnen (Proxys/Vorschaubilder laufen)");
    // Saving a large project
    let mut s = h.app.project.settings.clone();
    s.main_track_magnet = !s.main_track_magnet;
    h.app.apply(EditOp::SetSettings { settings: s });
    h.frames_for(4.0, "Autosave");
}
