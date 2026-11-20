//! Player: setzt alle sichtbaren Ebenen am Playhead zusammen (Video/Bild/Sticker mit
//! Effektkette als Textur je Clip, Text mit Kontur/Fläche/Schatten), wertet Keyframes
//! und Animationen aus und blendet Übergänge über. Bearbeitung direkt im Bild über
//! `player_edit`.

use std::hash::{Hash, Hasher};

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText};

use crate::app::{App, DragAsset};
use crate::player_edit::Region;
use crate::playcore::{Mode, Want};
use crate::preview::{to_color_image, Done, Job, OpenJob};
use crate::theme::{self, icon};

/// Messpunkt (`CUTLINE_PERF`): Zeit der Player-Fläche je UI-Bild (Summe und Höchstwert, µs)
pub static PERF_PLAYER_SUM_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PERF_PLAYER_MAX_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Ebene des Player-Kerns zu einem Clip (bzw. einer abgeleiteten Fassung)
fn lid(id: ClipId) -> crate::playcore::Lid {
    id.0.as_u128()
}

/// Eine sichtbare Ebene zum Zeitpunkt t.
#[derive(Clone)]
pub(crate) struct Layer {
    clip: Clip,
    /// Zeit innerhalb des Clips (kann bei Übergängen außerhalb liegen)
    t_rel: Time,
    /// Zusätzliche Deckkraft (Übergänge)
    alpha: f32,
    /// Übergang vom vorigen Clip: (dessen ID, Fortschritt 0…1)
    from: Option<(ClipId, f32)>,
}

/// Vorschau gemischter Übergänge: letzte Bilder der beteiligten Clips (CPU), das gemischte Bild
/// und welche Übergänge fürs Abspielen zu langsam waren.
#[derive(Default)]
pub struct ShaderPreview {
    cpu: std::collections::HashMap<ClipId, (u64, std::sync::Arc<cl_media::RgbaFrame>)>,
    /// mischt im Hintergrund (das Mischen kostet auf der CPU 10–150 ms je Bild)
    mixer: Option<Mixer>,
    /// beauftragte Mischung (höchstens eine) und die fertige, noch nicht übernommene
    pending: Option<u64>,
    ready: Option<MixDone>,
    /// außer den Clips mit Übergang: diese Bilder (Fassungen mit Effekt-Elementen) auch behalten
    wanted: Vec<ClipId>,
    tex: Option<egui::TextureHandle>,
    /// gemischte Fassungen mit den Effekt-Elementen darüber: (Textur, Maske, Stärke)
    fx: Vec<(egui::TextureHandle, Option<Mask>, f32)>,
    /// zuletzt gemischt: alle Eingaben (Bilder, Lage, Fläche, Fortschritt)
    last: Option<u64>,
    /// gerade gezeigt statt A und B: (A, B)
    current: Option<(ClipId, ClipId)>,
    /// beim Abspielen zu langsam: diese Übergänge bis zum Anhalten überblenden
    slow: Vec<(ClipId, ClipId)>,
    /// Anzahl zu langsamer Bilder im Übergang `slow_pair`
    slow_hits: u8,
    slow_pair: Option<(ClipId, ClipId)>,
    /// Übergang, zu dem `tex` gehört (wird gehalten, solange ein Bild fehlt)
    shown: Option<(ClipId, ClipId)>,
    /// Wiedergabe lief im letzten Bild (Start erkennen: Shader vorbereiten)
    was_playing: bool,
    /// Shader schon einmal (beim ersten Bild nach dem Start) vorbereitet
    warmed: bool,
}

/// Was die Vorschau in einem Übergang zeigt, den sie mischt.
#[derive(Debug, PartialEq)]
enum Step {
    /// alle Bilder da: mischen
    Mix,
    /// ein Bild fehlt noch: letztes Mischbild dieses Übergangs stehen lassen
    Hold,
    /// überblenden (noch kein Mischbild oder beim Abspielen zu langsam)
    Skip,
}

impl ShaderPreview {
    fn step(&self, pair: (ClipId, ClipId), have_frames: bool) -> Step {
        if self.slow.contains(&pair) {
            Step::Skip
        } else if have_frames {
            Step::Mix
        } else if self.shown == Some(pair) && self.tex.is_some() {
            Step::Hold
        } else {
            Step::Skip
        }
    }

    /// Dauer eines Mischbilds beim Abspielen: zwei Bilder länger als ein Bild-Intervall → dieser
    /// Übergang wird bis zum Anhalten überblendet (kein Hin und Her).
    fn note_time(&mut self, pair: (ClipId, ClipId), secs: f64, interval: f64) {
        if self.slow_pair != Some(pair) {
            (self.slow_pair, self.slow_hits) = (Some(pair), 0);
        }
        if secs > interval {
            self.slow_hits += 1;
            if self.slow_hits >= 2 && !self.slow.contains(&pair) {
                self.slow.push(pair);
            }
        }
    }
}

/// Mischauftrag an den Hintergrund-Thread: Bildpaare (Clips, dann Fassungen mit Effekt-Elementen)
/// mit Lage, Fläche und Fortschritt.
struct MixJob {
    id: u64,
    pair: (ClipId, ClipId),
    plan: cl_fx::catalog::TransitionPlan,
    frames: Vec<(std::sync::Arc<cl_media::RgbaFrame>, std::sync::Arc<cl_media::RgbaFrame>)>,
    /// je Bildpaar ab dem zweiten: Maske und Stärke des Effekt-Elements
    sets: Vec<(Option<Mask>, f32)>,
    ev: (Evaluated, Evaluated),
    size: (u32, u32),
    proj: (f32, f32),
    p: f32,
}

/// Fertige Mischung: Mischbild (unmaskierte Elemente eingerechnet), dann je maskiertem Element
/// dessen gemischte Fassung. Leer, wenn das Mischen scheiterte.
struct MixDone {
    id: u64,
    pair: (ClipId, ClipId),
    images: Vec<egui::ColorImage>,
    masked: Vec<(Option<Mask>, f32)>,
    secs: f64,
}

fn run_mix(j: MixJob) -> MixDone {
    let t0 = std::time::Instant::now();
    let (cw, ch) = j.size;
    let mixed: Option<Vec<Vec<u8>>> = j
        .frames
        .iter()
        .map(|(fa, fb)| mix_layers(&j.plan, (fa, &j.ev.0), (fb, &j.ev.1), (cw, ch), j.proj, j.p))
        .collect();
    let (mut images, mut masked) = (Vec::new(), Vec::new());
    if let Some(mixed) = mixed {
        // unmaskierte Elemente ins Mischbild einrechnen, maskierte zeichnet `draw_layers` in der Form
        let mut it = mixed.into_iter();
        let mut base = it.next().unwrap_or_default();
        let mut rest = Vec::new();
        for (img, x) in it.zip(&j.sets) {
            if x.0.is_none() {
                fold_fx(&mut base, &img, x.1);
            } else {
                rest.push(img);
                masked.push(*x);
            }
        }
        images = std::iter::once(base)
            .chain(rest)
            .map(|img| egui::ColorImage::from_rgba_unmultiplied([cw as usize, ch as usize], &img))
            .collect();
    }
    MixDone { id: j.id, pair: j.pair, images, masked, secs: t0.elapsed().as_secs_f64() }
}

/// Hintergrund-Thread für Übergangs-Mischungen (das Ergebnis erscheint ein Bild später).
struct Mixer {
    tx: std::sync::mpsc::Sender<MixJob>,
    rx: std::sync::mpsc::Receiver<MixDone>,
}

impl Mixer {
    fn spawn(ctx: egui::Context) -> Self {
        let (tx, jobs) = std::sync::mpsc::channel::<MixJob>();
        let (done, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("cl-mix".into()).spawn(move || {
            while let Ok(j) = jobs.recv() {
                if done.send(run_mix(j)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Self { tx, rx }
    }
}

/// Längste Seite der Mischfläche beim Abspielen: so bleibt ein Mischbild auch auf der CPU deutlich
/// unter einem Bild-Intervall, auch mit einem Effekt-Element darüber (Messung: Test `mix_time`).
const MIX_PLAY: f32 = 960.0;

/// Mischfläche im Seitenverhältnis des Projekts `pw`×`ph`, so groß wie die Anzeige `px` (höchstens
/// 1920, beim Abspielen höchstens [`MIX_PLAY`] an der langen Seite).
fn mix_size(pw: f32, ph: f32, px: egui::Vec2, playing: bool) -> (u32, u32) {
    let max = if playing { MIX_PLAY } else { 1920.0 };
    let k = (px.x / pw).min(px.y / ph).min(max / pw.max(ph));
    (((pw * k).round() as u32).max(16), ((ph * k).round() as u32).max(16))
}

/// Shader-Übergänge des Projekts im Hintergrund vorbereiten (GPU öffnen, Pipelines übersetzen),
/// damit das erste Mischbild beim Abspielen nicht hängt.
fn warm_shaders(project: &Project) {
    let mut files: Vec<std::path::PathBuf> = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter_map(|c| cl_fx::catalog::transition_plan(&c.transition_out.as_ref()?.id).shader.map(|s| s.0))
        .collect();
    files.sort();
    files.dedup();
    if files.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let Some(g) = cl_gpu::gpu() else { return };
        for f in files {
            if let Ok(src) = std::fs::read_to_string(&f) {
                let _ = g.prepare(&src);
            }
        }
    });
}

/// Unmaskierte Fassung `fx` eines Effekt-/Filter-Elements mit Stärke `k` in das Mischbild `base`
/// einrechnen: wie der Export, der das Element auf den fertigen Übergang anwendet (darübergelegt
/// schienen halbdurchsichtige Stellen des Übergangs, z. B. Spiegelungen, ungefiltert durch).
fn fold_fx(base: &mut [u8], fx: &[u8], k: f32) {
    for (b, f) in base.chunks_exact_mut(4).zip(fx.chunks_exact(4)) {
        for c in 0..4 {
            b[c] = (b[c] as f32 + (f[c] as f32 - b[c] as f32) * k).round() as u8;
        }
    }
}

/// Fassung `fx` (Effekt-/Filter-Element) über dem Bild `base` an den Ecken `corners` zeichnen,
/// mit Maske nur in der Form (umgekehrt: überall außer der Form), nur im sichtbaren Teil `shown`
/// (Zuschnitt; sonst gleich `corners`).
#[allow(clippy::too_many_arguments)]
fn draw_fx(content: &egui::Painter, base: egui::TextureId, fx: egui::TextureId, corners: [egui::Pos2; 4], shown: [egui::Pos2; 4], mask: &Option<Mask>, alpha: f32, canvas: Rect) {
    let Some(mk) = mask else {
        crate::fx_mask::draw_textured(content, fx, corners, &shown, alpha);
        return;
    };
    let (polys, inverted) = crate::fx_mask::polygons(mk, canvas);
    let polys = polys.iter().map(|p| crate::fx_mask::clip_convex(p, &shown));
    if inverted {
        // alles außer der Form: Effekt ganz, Original in der Form darüber
        crate::fx_mask::draw_textured(content, fx, corners, &shown, alpha);
        for poly in polys {
            crate::fx_mask::draw_textured(content, base, corners, &poly, alpha);
        }
    } else {
        for poly in polys {
            crate::fx_mask::draw_textured(content, fx, corners, &poly, alpha);
        }
    }
}

/// Fortschritt eines Übergangs der Länge `d` um den Schnitt `cut` (wie im Export:
/// Fenster `cut − d/2 … cut + d/2`), `None` außerhalb.
pub(crate) fn transition_progress(ts: f64, cut: f64, d: f64) -> Option<f32> {
    if d <= 0.0 || ts < cut - d / 2.0 || ts >= cut + d / 2.0 {
        return None;
    }
    Some(((ts - (cut - d / 2.0)) / d).clamp(0.0, 1.0) as f32)
}

/// Fertige Ebene für die Übergangs-Vorschau: das Clipbild (Effekte, Filter und Maske schon
/// eingerechnet) mit Lage, Größe, Drehung und Deckkraft aus `ev` auf eine leere Fläche
/// `cw`×`ch` (Projekt `pw`×`ph`) gesetzt – wie im Export, der Übergänge auch zwischen fertigen
/// Vollbild-Ebenen mischt. Gleiche Geometrie wie `draw_layers`.
// shortcut: nächstes Pixel statt Filterung; reicht, weil das Clipbild schon in Anzeigegröße kommt
pub(crate) fn place_on_canvas(src: &cl_media::RgbaFrame, ev: &Evaluated, cw: u32, ch: u32, pw: f32, ph: f32) -> Vec<u8> {
    let mut out = vec![0u8; (cw * ch * 4) as usize];
    let (sw, sh) = (src.width as f32, src.height as f32);
    let fit = (cw as f32 / sw).min(ch as f32 / sh) * ev.scale;
    if src.width == 0 || src.height == 0 || fit <= 0.0 || src.data.len() < (src.width * src.height * 4) as usize {
        return out;
    }
    let (kx, ky) = (cw as f32 / pw, ch as f32 / ph);
    let (cx, cy) = (cw as f32 / 2.0 + ev.x * kx, ch as f32 / 2.0 + ev.y * ky);
    let (s, c) = (-ev.rotation_deg.to_radians()).sin_cos();
    let alpha = ev.opacity.clamp(0.0, 1.0);
    // Zuschnitt: nur dieser Teil des Clipbilds, an seiner Stelle
    let k = ev.crop;
    let (u0, u1, v0, v1) = (k.left * sw, (1.0 - k.right) * sw, k.top * sh, (1.0 - k.bottom) * sh);
    // Zeilen auf mehrere Kerne verteilt: beim Abspielen zählt jede Millisekunde
    cl_fx::xfade::rows(&mut out, cw as i32, &|y, line| {
        let (dx, dy) = (0.5 - cx, y as f32 + 0.5 - cy);
        let (mut u, mut v) = ((dx * c - dy * s) / fit + sw / 2.0, (dx * s + dy * c) / fit + sh / 2.0);
        for o in line.chunks_exact_mut(4) {
            if u >= u0 && v >= v0 && u < u1 && v < v1 {
                let i = ((v as u32 * src.width + u as u32) * 4) as usize;
                // wie das Overlay auf die durchsichtige Fläche im Export: Farbe außerhalb der Maske
                // (und bei Deckkraft < 1) geht in Schwarz über, statt beim Mischen durchzuscheinen
                let a = (src.data[i + 3] as f32 * alpha).round() as u32;
                for k in 0..3 {
                    o[k] = ((src.data[i + k] as u32 * a + 127) / 255) as u8;
                }
                o[3] = a as u8;
            }
            u += c / fit;
            v += s / fit;
        }
    });
    out
}

/// Mischt die Vorschau den Übergang selbst? Sonst überblendet sie (`fade` ist genau das; andere
/// `xfade`-Sorten ohne Nachbildung in `cl_fx::xfade` auch).
pub(crate) fn mixes(plan: &cl_fx::catalog::TransitionPlan) -> bool {
    plan.shader.is_some() || (plan.xfade != "fade" && cl_fx::xfade::supported(plan.xfade))
}

/// Übergang wie im Export zwischen fertigen Ebenen mischen: Clipbilder `fa`/`fb` (Effekte und
/// Maske eingerechnet) mit Lage `ea`/`eb` auf eine Fläche `cw`×`ch` (Projekt `pw`×`ph`) setzen,
/// dann Shader (cl-gpu) bzw. `cl_fx::xfade` beim Fortschritt `p`. RGBA, Alpha bleibt Alpha.
/// Ohne GPU (oder bei einem Shader-Fehler) wie der Export mit dem `xfade`-Rückfall des Pakets.
pub(crate) fn mix_layers(
    plan: &cl_fx::catalog::TransitionPlan,
    (fa, ea): (&cl_media::RgbaFrame, &Evaluated),
    (fb, eb): (&cl_media::RgbaFrame, &Evaluated),
    (cw, ch): (u32, u32),
    (pw, ph): (f32, f32),
    p: f32,
) -> Option<Vec<u8>> {
    let a = place_on_canvas(fa, ea, cw, ch, pw, ph);
    let b = place_on_canvas(fb, eb, cw, ch, pw, ph);
    let shader = plan.shader.as_ref().and_then(|(file, params)| {
        cl_gpu::render(
            &std::fs::read_to_string(file).ok()?,
            &crate::preview::shader_params(params),
            cl_gpu::Frame { rgba: &a, width: cw, height: ch },
            cl_gpu::Frame { rgba: &b, width: cw, height: ch },
            (cw, ch),
            p,
        )
        .ok()
    });
    shader.or_else(|| cl_fx::xfade::mix(plan.xfade, &a, &b, cw, ch, p))
}

/// Längen der Übergänge am Anfang und Ende von `c` (Clip `i` der Spur, ggf. in Bearbeitung
/// verändert): nur zu direkt anschließenden, aktiven Nachbarn, Länge wie im Export.
pub(crate) fn transition_spans(clips: &[Clip], i: usize, c: &Clip) -> (Option<f64>, Option<f64>) {
    let next = clips.get(i + 1).filter(|n| (n.start.0 - c.end().0).abs() < 2000 && !n.disabled);
    let tr_out = next.filter(|_| c.transition_out.is_some()).map(|n| cl_fx::transition_secs(c, n));
    let prev = i.checked_sub(1).and_then(|k| clips.get(k));
    let tr_in = prev
        .filter(|p| (c.start.0 - p.end().0).abs() < 2000 && !p.disabled && p.transition_out.is_some())
        .map(|p| cl_fx::transition_secs(p, c));
    (tr_in, tr_out)
}

/// Dekodiergröße eines Clips für die Vorschau (`canvas_px` = Anzeigefläche in Pixeln).
fn decode_size(c: &Clip, m: &MediaRef, canvas_px: egui::Vec2, div: f32) -> (u32, u32) {
    // Vergrößerte Clips (z. B. Querformat-Gameplay im Hochformat auf 200 %) brauchen
    // entsprechend mehr Pixel, sonst wird ein kleines Bild hochgezogen und unscharf.
    let zoom = c
        .keyframes
        .get(Prop::Scale)
        .iter()
        .map(|k| k.v)
        .fold(c.transform.scale, f32::max)
        .clamp(0.25, 6.0);
    let cap = |v: f32, src: u32| (v as u32).clamp(64, 3840).min(src.max(64));
    let max_w = cap(canvas_px.x / div * zoom, m.width);
    let max_h = cap(canvas_px.y / div * zoom, m.height);
    cl_media::fit_size(m.width, m.height, max_w, max_h)
}

/// Quellzeit des Einzelbilds eines Clips bei Clipzeit `t_rel`.
fn still_src(c: &Clip, m: &MediaRef, t_rel: Time, fps: Rational) -> Time {
    let ClipContent::Media { kind: MediaKind::Video, .. } = &c.content else {
        // Bewegtes Bild: dasselbe Einzelbild wie im Export (Schleife bzw. letztes Bild halten)
        // shortcut: beim Abspielen ein Einzelbild-Auftrag je Bild statt Strom, ausbauen falls es ruckelt
        return cl_media::animation(&m.path).map_or(Time::ZERO, |a| a.seek_time(t_rel));
    };
    let mut end = c.source_in + c.source_span();
    if m.duration > Time::ZERO {
        end = Time(end.0.min(m.duration.0));
    }
    still_seek(c.source_time(c.start + t_rel), c.source_in, end, m.fps.unwrap_or(fps))
}

/// Quellzeit für ein Einzelbild der Vorschau: aufs Bildraster gerundet, höchstens anderthalb
/// Bilder vor dem Ende von Clip bzw. Datei. FFmpeg liefert das erste Bild ab `-ss`; eine Zeit im
/// letzten Bild (oder aufs Ende gerundet) ergab gar keins – „Datei kann nicht gelesen werden: “.
fn still_seek(t: Time, source_in: Time, end: Time, fps: Rational) -> Time {
    let last = end - Time::from_secs_f64(1.5 / fps.as_f64().max(1.0));
    Time(t.snap_to_frame(fps).0.min(last.0).max(source_in.0))
}

/// Alle sichtbaren Ebenen von unten nach oben (inkl. Übergängen); `live` ersetzt den Clip
/// gleicher ID (laufende Bearbeitung).
fn layers_in(project: &Project, live: Option<&Clip>, t: Time) -> Vec<Layer> {
    let mut out = Vec::new();
    for tr in project
        .timeline
        .tracks
        .iter()
        .filter(|tr| !tr.hidden && tr.kind != TrackKind::Audio)
    {
        for (i, c0) in tr.clips.iter().enumerate() {
            // erst das Zeitfenster prüfen, dann klonen
            let c = live.filter(|l| l.id == c0.id).unwrap_or(c0);
            if c.disabled {
                continue;
            }
            let (tr_in, tr_out) = transition_spans(&tr.clips, i, &c);
            let prev = if i > 0 { tr.clips.get(i - 1) } else { None };
            let ts = t.as_secs_f64();
            let (s, e) = (c.start.as_secs_f64(), c.end().as_secs_f64());
            let lo = s - tr_in.map(|d| d / 2.0).unwrap_or(0.0);
            let hi = e + tr_out.map(|d| d / 2.0).unwrap_or(0.0);
            if ts < lo || ts >= hi {
                continue;
            }
            let mut alpha = 1.0;
            let mut from = None;
            if let Some(d) = tr_in {
                // Einblenden über dem Vorgänger
                let p = transition_progress(ts, s, d).unwrap_or(1.0);
                alpha = p;
                from = prev.map(|pc| (pc.id, p));
            }
            let t_rel = Time::from_secs_f64((ts - s).clamp(0.0, (e - s - 1e-3).max(0.0)));
            out.push(Layer {
                clip: c.clone(),
                t_rel,
                alpha,
                from,
            });
        }
    }
    out
}

/// Eine Ebene, für die die Vorschau ein Bild braucht: Clip (bzw. seine Fassung mit einem
/// Effekt-Element), Medium, Lage in der Zeit, ggf. Startstelle (vorab öffnen) und – bei Fassungen
/// mit Element – (Basis-Clip, Maske, Stärke).
pub(crate) struct Need {
    pub id: ClipId,
    pub clip: Clip,
    pub media: MediaRef,
    pub layer: Layer,
    pub prefetch: Option<Time>,
    pub overlay: Option<(ClipId, Option<Mask>, f32)>,
}

/// Was die Vorschau zu `now` braucht (gemeinsam für App und Mess-Test): alle sichtbaren Medien-
/// Ebenen, aktive bzw. gleich beginnende Effekt-Elemente als eigene Fassungen und beim Abspielen
/// Clips, die in höchstens 1,5 s beginnen (Ströme vorab öffnen, damit an Schnitten nichts hakt).
/// `adjust` ändert den Clip vor der Anfrage (Pipette: ohne Chroma-Key).
pub(crate) fn needs(project: &Project, layers: &[Layer], now: Time, playing: bool, adjust: impl Fn(&mut Clip)) -> Vec<Need> {
    const AHEAD: f64 = 1.5;
    let mut out = Vec::new();
    for l in layers {
        let mut c = l.clip.clone();
        adjust(&mut c);
        let ClipContent::Media { media, .. } = &c.content else {
            continue;
        };
        let Some(m) = project.media(*media) else {
            continue;
        };
        out.push(Need { id: c.id, clip: c.clone(), media: m.clone(), layer: l.clone(), prefetch: None, overlay: None });
        for e in fx_elements_over(project, c.id) {
            let active = e.contains(now);
            let soon = playing && e.start > now && (e.start - now).as_secs_f64() <= AHEAD && e.start < c.end();
            if !active && !soon {
                continue;
            }
            let Some((x, k)) = fx_version(&c, e, now) else { continue };
            let id = c.id.derived(e.id.0.as_u128());
            let (layer, prefetch, overlay) = if active {
                (l.clone(), None, Some((c.id, e.fx.mask, k)))
            } else {
                (Layer { clip: x.clone(), t_rel: e.start - c.start, alpha: l.alpha, from: None }, Some(e.start), None)
            };
            out.push(Need { id, clip: x, media: m.clone(), layer, prefetch, overlay });
        }
    }
    if playing {
        let upcoming = project
            .timeline
            .tracks
            .iter()
            .filter(|tr| !tr.hidden && matches!(tr.kind, TrackKind::Video | TrackKind::Sticker))
            .flat_map(|tr| tr.clips.iter())
            .filter(|c| !c.disabled && c.start > now && (c.start - now).as_secs_f64() <= AHEAD);
        for c in upcoming {
            let ClipContent::Media { media, .. } = &c.content else {
                continue;
            };
            let Some(m) = project.media(*media) else {
                continue;
            };
            let layer = Layer { clip: c.clone(), t_rel: Time::ZERO, alpha: 1.0, from: None };
            out.push(Need { id: c.id, clip: c.clone(), media: m.clone(), layer, prefetch: Some(c.start), overlay: None });
        }
    }
    out
}

/// Kann der Clip als Strom laufen? (Rückwärts und Tempo-Kurven: nur Einzelbilder)
pub(crate) fn streamable(c: &Clip) -> bool {
    matches!(&c.content, ClipContent::Media { kind: MediaKind::Video, .. }) && !c.fx.reverse && c.fx.speed_curve.is_none()
}

/// Quellbildrate für das Rückwärts-Fenster des Decoders (`preview::Job::Frame::window`): nur beim
/// Abspielen vorwärts eines rückwärts laufenden Videos aus dem Original ohne Effektgraph (der
/// Graph hängt von der Clipzeit ab), höchstens 1080p. Stand, Pause, Schrittweise und Scrubben:
/// `None` wie bisher. Über 1080p decodiert FFmpeg kaum schneller als Echtzeit (gemessen 4K:
/// ~35 Bilder/s Wandzeit): das Fenster käme nie vor die Uhr und wäre langsamer als Einzelbilder.
pub(crate) fn reverse_window(c: &Clip, m: &MediaRef, original: bool, no_graph: bool, playing_forward: bool, fps: Rational) -> Option<Rational> {
    let video = matches!(&c.content, ClipContent::Media { kind: MediaKind::Video, .. });
    let hd = m.width.max(m.height) <= 1920 && m.width.min(m.height) <= 1080;
    (video && hd && c.fx.reverse && original && no_graph && playing_forward).then(|| m.fps.unwrap_or(fps))
}

/// Fingerabdruck der Effekte eines Clips (Teil der Bild- und Stromschlüssel)
pub(crate) fn fx_hash_of(c: &Clip) -> u64 {
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&c.fx).unwrap_or_default().hash(&mut hs);
    hs.finish()
}

/// Strom einer Ebene: Schlüssel und Decodiergröße
pub(crate) fn stream_spec(id: ClipId, c: &Clip, m: &MediaRef, canvas_px: egui::Vec2, div: f32, fps: Rational, fxh: u64) -> (u64, u32, u32) {
    let (w, h) = decode_size(c, m, canvas_px, div);
    (App::frame_key(id, c, Time::ZERO, w, h, Time::ZERO, false, fxh, fps), w, h)
}

/// Auftrag, den Strom ab der Zeitleistenstelle `o.at` zu öffnen (Quellzeit und Effekt-Graph dazu)
/// Quell-Fingerabdruck eines Stroms: Datei, Quellzeit je Zeitleistenstelle (bleibt beim Teilen
/// gleich), Tempo, Effekte, Größe, Bildrate – ohne Clip-ID (siehe `playcore::Want::alias`)
pub(crate) fn stream_alias(c: &Clip, m: &MediaRef, (w, h): (u32, u32), fps: Rational, fxh: u64) -> u64 {
    let offset = ((c.source_in.0 as f64 - c.start.0 as f64 * c.speed.max(0.01) as f64) / 1000.0).round() as i64;
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    (m.id, offset, c.speed.to_bits(), fxh, w, h, fps.num, fps.den).hash(&mut hs);
    hs.finish() | 1
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn open_job(o: crate::playcore::Open, c: &Clip, m: &MediaRef, path: std::path::PathBuf, (w, h): (u32, u32), fps: Rational, alias: u64) -> OpenJob {
    let lut = c.fx.lut.as_deref().filter(|p| std::path::Path::new(p).is_file()).map(cl_media::escape_path);
    let t_rel = Time((o.at - c.start).0.clamp(0, (c.duration.0 - 1000).max(0)));
    OpenJob {
        open: o,
        alias,
        source_at: c.source_time(c.start + t_rel),
        graph: cl_fx::preview_graph(c, w, h, t_rel.as_secs_f64(), Some(fps), lut),
        hw: crate::preview::stream_decode(m, &path),
        path,
        w,
        h,
        fps,
    }
}

/// Effekt-/Filter-Elemente auf Spuren über dem Clip `clip` (eingeschaltet, sichtbar). Ohne Klone:
/// die Aufrufer filtern nach Zeit (sonst kopierte jedes UI-Bild alle Elemente des Projekts).
fn fx_elements_over(project: &Project, clip: ClipId) -> Vec<&Clip> {
    let tracks = &project.timeline.tracks;
    let Some(idx) = tracks
        .iter()
        .position(|tr| tr.clips.iter().any(|c| c.id == clip))
    else {
        return Vec::new();
    };
    tracks[idx + 1..]
        .iter()
        .filter(|tr| !tr.hidden && matches!(tr.kind, TrackKind::Effect | TrackKind::Filter))
        .flat_map(|tr| tr.clips.iter())
        .filter(|c| !c.disabled)
        .collect()
}

/// Fassung von `c` mit dem Effekt-/Filter-Element `e` (zum Zeitpunkt `now`) und dessen Stärke.
/// Die Vorschau legt sie über das Clipbild (der Export wendet das Element auf alles darunter an).
fn fx_version(c: &Clip, e: &Clip, now: Time) -> Option<(Clip, f32)> {
    let (mut r, filter) = match &e.content {
        ClipContent::Effect(r) => (r.clone(), false),
        ClipContent::Filter(r) => (r.clone(), true),
        _ => return None,
    };
    let animated = !e.keyframes.get(Prop::Strength).is_empty();
    if animated {
        r.intensity = 1.0;
    }
    let mut x = c.clone();
    if filter {
        x.fx.filter = Some(r);
    } else if !x.fx.effects.iter().any(|f| f.id == r.id) {
        x.fx.effects.push(r);
    }
    Some((x, if animated { e.strength_at(now - e.start) } else { 1.0 }))
}

/// Übergänge, die die Vorschau zur Zeit `t` mischt: (Ebene A, Ebene B, Plan, Fortschritt), unten
/// zuerst. Alle anderen überblendet sie (B mit wachsender Deckkraft über A). Liegt ein Effekt-
/// oder Filter-Element darüber, wird auch Überblenden gemischt: der Export wendet das Element auf
/// den fertigen Übergang an, die Vorschau legt seine Fassung von A und B gemischt darüber.
fn mix_pairs(project: &Project, layers: &[Layer], t: Time) -> Vec<(usize, usize, cl_fx::catalog::TransitionPlan, f32)> {
    layers
        .iter()
        .enumerate()
        .filter_map(|(bi, b)| {
            let (a_id, p) = b.from?;
            let ai = layers.iter().position(|l| l.clip.id == a_id && l.alpha >= 0.999)?;
            let plan = cl_fx::catalog::transition_plan(&layers[ai].clip.transition_out.as_ref()?.id);
            let fx_over = || fx_elements_over(project, a_id).iter().any(|e| e.contains(t));
            (mixes(&plan) || fx_over()).then_some((ai, bi, plan, p))
        })
        .collect()
}

#[cfg(test)]
mod play_measure;

#[cfg(test)]
mod presence_tests {
    use super::*;

    /// Nachtest K7: 4K-Clip mit Tempo 2 über einem zweiten 4K-Clip blieb beim Abspielen ein
    /// Standbild, solange sein Proxy in Stücken gebaut wurde. Hier mit der echten App (Quellwahl,
    /// Starter, Kern) ohne Fenster: die obere Ebene muss fortlaufend Bilder holen.
    /// `CUTLINE_QA_TEMPO` (2, 0.5, rev, kurve, scale), `CUTLINE_QA_PARTS`: so viele Proxy-Stücke der
    /// oberen Quelle vorher bauen (der Rest fehlt, wie während des Baus).
    /// `cargo test --release -p cl-app --bin cutline tempo_in_app -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn tempo_in_app() {
        let (a, b) = (std::path::Path::new("F:/cutline-testmedien/4k60_h264_80mbit.mp4"), std::path::Path::new("F:/cutline-testmedien/4k60_hevc10_50mbit.mp4"));
        if !cl_media::ffmpeg_available() || !a.is_file() || !b.is_file() {
            return;
        }
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        std::env::set_var("CUTLINE_NO_UPDATE", "1");
        std::env::set_var("CUTLINE_NO_ONBOARDING", "1");
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        let mode = std::env::var("CUTLINE_QA_TEMPO").unwrap_or_else(|_| "2".into());
        let (ma, mb) = (cl_media::probe(a).unwrap(), cl_media::probe(b).unwrap());
        let mut p = Project::new("tempo");
        p.settings.fps = Rational::new(60, 1);
        let mut v1 = cl_core::Track::new(TrackKind::Video);
        v1.main = true;
        v1.clips.push(Clip::new(ClipContent::Media { media: ma.id, kind: MediaKind::Video }, Time::ZERO, ma.duration));
        let mut v2 = cl_core::Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: mb.id, kind: MediaKind::Video }, Time::ZERO, mb.duration);
        match mode.as_str() {
            "rev" => c.fx.reverse = true,
            "kurve" => c.fx.speed_curve = Some("montage".into()),
            "scale" => {
                for (i, v) in [0.5f32, 1.5, 0.5, 1.5].iter().enumerate() {
                    c.keyframes.set(Prop::Scale, Time::from_secs_f64(20.0 + i as f64 * 3.0), *v);
                }
            }
            t => {
                c.speed = t.parse().unwrap_or(2.0);
                c.duration = Time::from_secs_f64(mb.duration.as_secs_f64() / c.speed as f64);
            }
        }
        let top = c.id;
        v2.clips.push(c);
        p.timeline.tracks = vec![v1, v2];
        p.media = vec![ma, mb.clone()];
        app.project = p;
        // Proxy in Stücken (wie im laufenden Bau): `CUTLINE_QA_PARTS` Stücke à 10 s fertig
        if let Some(n) = std::env::var("CUTLINE_QA_PARTS").ok().and_then(|s| s.parse().ok()) {
            let want = (app.settings.proxy_h, app.project.settings.fps);
            let t = std::time::Instant::now();
            crate::proxy::test_build_parts(&mb, home.path().join("p").join("b-540.mp4"), want, n);
            eprintln!("{n} Stücke in {:.0} s", t.elapsed().as_secs_f64());
        }
        for at in [20.0, 33.0, 72.0] {
            app.playhead = Time::from_secs_f64(at);
            app.core.stop();
            app.core.play(app.playhead, app.revision, app.core_now());
            let t0 = std::time::Instant::now();
            // Strombilder der oberen Ebene (über Stromwechsel hinweg) und ab wann
            let (mut total, mut last_taken, mut first) = (0u64, 0u64, None);
            // Einzelbilder (Rückwärts, Tempo-Kurve): jedes neue Bild hat einen neuen Schlüssel
            let (mut stills, mut last_key) = (0u32, None);
            while t0.elapsed().as_secs_f64() < 8.0 {
                app.receive_decoded(&ctx);
                let t = crate::playcore::Tick { now: app.core_now(), playhead: app.playhead, revision: app.revision, audio: crate::playcore::AudioIn::None, end: app.project.duration(), fps: 60.0, loop_range: None, recording: false, max_quality_level: 3 };
                app.playhead = app.core.tick(&t);
                let _ = app.core.cmds();
                app.playing = app.core.playing();
                let layers = app.layers_at(app.playhead);
                app.update_layers(&ctx, &layers, egui::vec2(1280.0, 720.0));
                let k = app.frames.get(&top).map(|f| f.0);
                if k.is_some() && k != last_key {
                    stills += 1;
                    last_key = k;
                }
                let taken = app.core.taken(lid(top));
                total += if taken >= last_taken { taken - last_taken } else { taken };
                last_taken = taken;
                if first.is_none() && total > 0 {
                    first = Some((t0.elapsed().as_secs_f64(), total));
                }
                if std::env::var_os("CUTLINE_QA_DEBUG").is_some() {
                    let el = t0.elapsed().as_secs_f64();
                    if (el * 4.0).fract() < 0.03 {
                        eprintln!("{el:.2} s pos {:.2}: oben {} Bilder, Wahl {:?}, Ströme {:?} {}", app.playhead.as_secs_f64(), app.core.taken(lid(top)), app.proxy_picks.get(&lid(top)), app.core.streams().iter().map(|x| (x.0 & 0xffff, x.1 & 0xffff, x.2)).collect::<Vec<_>>(), app.core.sizes());
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let n = total;
            eprintln!("Tempo {mode} ab {at}: obere Ebene {n} Strombilder (erstes {first:?}), {stills} Einzelbilder, Ströme {:?}", app.core.streams());
            if streamable(app.project.find_clip(top).unwrap().1) {
                let (t1, n1) = first.expect("obere Ebene ohne Bild");
                assert!((n - n1) as f64 >= (8.0 - t1 - 1.0) * 20.0, "obere Ebene steht: {n1} → {n}");
            } else {
                assert!(stills >= 4, "obere Ebene steht: {stills} Einzelbilder in 8 s");
            }
        }
    }

    /// Befund aus dem Review: beim Abspielen prüfte jedes UI-Bild je Ebene die Datei
    /// (`is_local`, auf dem NAS blockierend). Jetzt entscheidet der Hintergrund-Zustand
    /// `presence` – hier „vorhanden" für eine Datei, die es nicht gibt: Strom und Einzelbild
    /// werden trotzdem angefordert, also fragt niemand das Dateisystem. Fehlt sie laut
    /// Hintergrund-Prüfung, wird nichts angefordert (keine offenen Aufträge).
    #[test]
    fn requests_follow_the_background_presence_check() {
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        std::env::set_var("CUTLINE_NO_UPDATE", "1");
        std::env::set_var("CUTLINE_NO_ONBOARDING", "1");
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        let path = home.path().join("gibt-es-nicht.mp4");
        let m = MediaRef {
            id: cl_core::MediaId::new(),
            path: path.clone(),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(10.0),
            width: 64,
            height: 36,
            fps: Some(Rational::new(30, 1)),
            has_audio: false,
            audio_streams: 0,
            video_codec: Some("h264".into()),
            audio_codec: None,
        };
        let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(10.0));
        let l = Layer { clip: c.clone(), t_rel: Time::ZERO, alpha: 1.0, from: None };
        let canvas = egui::vec2(64.0, 36.0);
        let ask = |app: &mut App| app.request_frame(&ctx, c.id, &c, &m, &l, canvas, None);
        // Stand: Einzelbild nur, wenn vorhanden
        app.presence.set_all([(path.clone(), false)].into(), 1);
        ask(&mut app);
        assert!(!app.core.has_requests(), "fehlende Datei: kein Auftrag");
        app.presence.set_all([(path.clone(), true)].into(), 1);
        ask(&mut app);
        assert!(app.core.has_requests(), "laut Prüfung vorhanden: angefordert");
        // Abspielen: Strom wird beauftragt (der Starter meldet sich, auch wenn das Öffnen scheitert)
        app.core.play(Time::ZERO, 0, 0.0);
        ask(&mut app);
        assert!(app.starter.rx.recv_timeout(std::time::Duration::from_secs(20)).is_ok(), "Strom beauftragt");
        app.core.invalidate();
        app.presence.set_all([(path, false)].into(), 1);
        ask(&mut app);
        assert!(app.starter.rx.recv_timeout(std::time::Duration::from_millis(500)).is_err(), "fehlt: kein Strom");
    }

    /// Strom aus Proxy-Stücken: der Auftrag zeigt auf die Stück-Liste, und die Zeit darin ist um
    /// den Anfang des ersten Stücks verschoben. Die Quellenwahl steckt im Stromschlüssel und im
    /// Quell-Fingerabdruck (ein Wechsel öffnet neu, ein Spender-Strom anderer Wahl wird nicht übernommen).
    #[test]
    fn stream_from_parts_opens_the_list_at_the_shifted_time() {
        let (mut app, home, _g) = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        let (starter, jobs) = crate::preview::Starter::capture();
        app.starter = starter;
        app.settings.use_proxies = true;
        let m = MediaRef {
            id: cl_core::MediaId::new(),
            path: home.path().join("q.mp4"),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(100.0),
            width: 3840,
            height: 2160,
            fps: Some(Rational::new(30, 1)),
            has_audio: false,
            audio_streams: 0,
            video_codec: Some("h264".into()),
            audio_codec: None,
        };
        app.project.media.push(m.clone());
        let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(100.0));
        let l = Layer { clip: c.clone(), t_rel: Time::from_secs_f64(12.0), alpha: 1.0, from: None };
        let want = (app.settings.proxy_h, app.project.settings.fps);
        let out = home.path().join("q-540.mp4");
        // Stücke 1–3 (10–40 s) fertig
        let ready: Vec<bool> = (0..10).map(|k| (1..4).contains(&k)).collect();
        crate::proxy::set_test_entry(&m.path, out.clone(), want, ready.clone(), false);
        app.playhead = Time::from_secs_f64(12.0);
        app.core.play(app.playhead, 0, 0.0);
        let ask = |app: &mut App| app.request_frame(&ctx, c.id, &c, &m, &l, egui::vec2(64.0, 36.0), None);
        ask(&mut app);
        let parts = jobs.try_recv().expect("Strom aus Stücken beauftragt");
        assert_eq!(parts.path, home.path().join("q-540.teile").join("0001-0004.ffconcat"));
        assert!((parts.source_at.as_secs_f64() - 2.0).abs() < 1e-3, "Quellzeit 12 s − Anfang Stück 1 (10 s): {:?}", parts.source_at);
        ask(&mut app);
        assert!(jobs.try_recv().is_err(), "gleiche Wahl: kein neuer Strom");
        // fertig zusammengefügt: andere Wahl → anderer Schlüssel, neuer Strom aus der ganzen Datei
        crate::proxy::set_test_entry(&m.path, out.clone(), want, ready, true);
        ask(&mut app);
        let full = jobs.try_recv().expect("Wechsel öffnet einen Ersatzstrom");
        // Datei aus dem Bestand, ohne auf der Platte nachzusehen (dort liegt sie im Test gar nicht)
        assert_eq!(full.path, out);
        assert!((full.source_at.as_secs_f64() - 12.0).abs() < 1e-3, "{:?}", full.source_at);
        assert_ne!(full.open.key, parts.open.key);
        assert_ne!(full.alias, parts.alias, "Quellenwahl im Fingerabdruck");
    }
}

#[cfg(test)]
mod shader_tests {
    use super::*;

    #[test]
    fn short_clip_between_two_long_transitions_matches_export() {
        // 0,2-s-Clip mit zwei 0,5-s-Übergängen: beide auf 0,2 s begrenzt, auch beim Nachbarn
        let tr = |s: f64, d: f64| {
            let mut c = Clip::new(ClipContent::Text(Default::default()), Time::from_secs_f64(s), Time::from_secs_f64(d));
            c.transition_out = Some(cl_core::TransitionRef { id: "fade".into(), duration: Time::from_secs_f64(0.5) });
            c
        };
        let clips = [tr(0.0, 2.0), tr(2.0, 0.2), tr(2.2, 2.0)];
        let span = |i: usize| transition_spans(&clips, i, &clips[i]);
        assert_eq!(span(0), (None, Some(0.2)));
        assert_eq!(span(1), (Some(0.2), Some(0.2)));
        assert_eq!(span(2), (Some(0.2), None));
        // halbe Übergänge ragen zusammen höchstens über die ganze Länge des kurzen Clips
        let (a, b) = span(1);
        assert!(a.unwrap() / 2.0 + b.unwrap() / 2.0 <= 0.2 + 1e-9);
    }

    /// Rückwärts-Fenster nur beim Abspielen vorwärts eines rückwärts laufenden Videos aus dem
    /// Original ohne Effektgraph; Pause/Schrittweise/Scrubben, Proxy und Effekte wie bisher.
    #[test]
    fn reverse_window_only_while_playing_a_plain_reversed_original() {
        let m = MediaRef {
            id: cl_core::MediaId::new(),
            path: "a.mp4".into(),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(10.0),
            width: 64,
            height: 36,
            fps: Some(Rational::new(30, 1)),
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        };
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(10.0));
        let p = Rational::new(60, 1);
        assert_eq!(reverse_window(&c, &m, true, true, true, p), None, "vorwärts laufender Clip");
        c.fx.reverse = true;
        assert_eq!(reverse_window(&c, &m, true, true, true, p), Some(Rational::new(30, 1)), "Quellrate");
        assert_eq!(reverse_window(&c, &m, true, true, false, p), None, "Stand, Pause, Scrubben");
        assert_eq!(reverse_window(&c, &m, false, true, true, p), None, "Proxy");
        assert_eq!(reverse_window(&c, &m, true, false, true, p), None, "Effektgraph");
        let m2 = MediaRef { fps: None, ..m.clone() };
        assert_eq!(reverse_window(&c, &m2, true, true, true, p), Some(p), "ohne Quellrate: Projektrate");
        let uhd = MediaRef { width: 3840, height: 2160, ..m.clone() };
        assert_eq!(reverse_window(&c, &uhd, true, true, true, p), None, "über 1080p: Einzelbilder");
        let img = Clip { content: ClipContent::Media { media: m.id, kind: MediaKind::Image }, ..c };
        assert_eq!(reverse_window(&img, &m, true, true, true, p), None, "Bild");
    }

    #[test]
    fn still_seek_never_lands_behind_the_last_frame() {
        let fps = Rational { num: 25, den: 1 };
        let end = Time::from_millis(6400);
        // vorher: 6,38 s (Ende − 20 ms) aufs Raster gerundet = 6,40 s → kein Bild
        let t = still_seek(Time::from_millis(6390), Time::ZERO, end, fps);
        assert!(t > Time::from_millis(6320) && t <= Time::from_millis(6360), "{t:?}");
        // mitten im Clip unverändert aufs Raster
        assert_eq!(still_seek(Time::from_millis(1013), Time::ZERO, end, fps), Time::from_millis(1000));
        // nie vor den Quell-Anfang
        assert_eq!(still_seek(Time::ZERO, Time::from_millis(6390), end, fps), Time::from_millis(6390));
    }

    #[test]
    fn progress_is_centered_on_the_cut() {
        assert_eq!(transition_progress(0.69, 1.0, 0.6), None);
        assert_eq!(transition_progress(0.7, 1.0, 0.6), Some(0.0));
        assert!((transition_progress(1.0, 1.0, 0.6).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(transition_progress(1.3, 1.0, 0.6), None);
        assert_eq!(transition_progress(1.0, 1.0, 0.0), None);
    }

    #[test]
    fn transition_layers_are_placed_like_on_screen() {
        let id = Evaluated { x: 0.0, y: 0.0, scale: 1.0, rotation_deg: 0.0, opacity: 1.0, volume_db: 0.0, crop: Crop::default() };
        let frame = |w: u32, h: u32, a: u8| cl_media::RgbaFrame { width: w, height: h, data: [255, 0, 0, a].repeat((w * h) as usize) };
        let alpha = |img: &[u8], cw: u32, x: u32, y: u32| img[((y * cw + x) * 4 + 3) as usize];
        // unverändert: füllt die Fläche
        let full = place_on_canvas(&frame(16, 9, 255), &id, 32, 18, 1920.0, 1080.0);
        assert!(full.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
        // Querformat im Hochformat-Projekt, auf Bildhöhe vergrößert (K2_short): füllt ebenfalls
        let cover = place_on_canvas(&frame(16, 9, 255), &Evaluated { scale: 3.1605, ..id }, 18, 32, 1080.0, 1920.0);
        assert!(cover.chunks_exact(4).all(|p| p[3] == 255));
        // ohne Vergrößerung: nur ein Streifen in der Mitte, oben und unten leer
        let strip = place_on_canvas(&frame(16, 9, 255), &id, 18, 32, 1080.0, 1920.0);
        assert_eq!((alpha(&strip, 18, 9, 16), alpha(&strip, 18, 9, 1)), (255, 0));
        // verkleinert und nach rechts verschoben: links leer, rechts Bild
        let moved = place_on_canvas(&frame(16, 9, 255), &Evaluated { scale: 0.5, x: 480.0, ..id }, 32, 18, 1920.0, 1080.0);
        assert_eq!((alpha(&moved, 32, 4, 9), alpha(&moved, 32, 24, 9)), (0, 255));
        // Deckkraft und Maske (Alpha im Clipbild) bleiben erhalten
        let faded = place_on_canvas(&frame(16, 9, 255), &Evaluated { opacity: 0.5, ..id }, 32, 18, 1920.0, 1080.0);
        assert_eq!(alpha(&faded, 32, 16, 9), 128);
        // außerhalb der Maske auch die Farbe schwarz (wie im Export): sonst schien beim Mischen
        // (Verpixeln in N_tr) das Bild ohne Maske durch
        let masked = place_on_canvas(&frame(16, 9, 0), &id, 32, 18, 1920.0, 1080.0);
        assert!(masked.iter().all(|&v| v == 0));
        let half = place_on_canvas(&frame(16, 9, 128), &id, 32, 18, 1920.0, 1080.0);
        assert_eq!(&half[..4], &[128, 0, 0, 128]);
        // 90° gedreht: das Querformat steht hochkant, links und rechts leer
        let turned = place_on_canvas(&frame(16, 9, 255), &Evaluated { rotation_deg: 90.0, ..id }, 32, 18, 1920.0, 1080.0);
        assert_eq!((alpha(&turned, 32, 16, 1), alpha(&turned, 32, 2, 9)), (255, 0));
    }
}

/// Vorschau-Standbild ohne Fenster: dieselben Schritte wie der Player im Stand (Ebenen,
/// Dekodiergröße, Einzelbild mit Effektkette, Effekt-Elemente, Mischen der Übergänge), nur
/// das Zusammensetzen der Ebenen auf der CPU statt in egui. Text bleibt weg.
#[cfg(test)]
pub(crate) mod still {
    use super::*;

    fn over(dst: &mut [u8], src: &[u8], k: f32) {
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let a = s[3] as f32 / 255.0 * k;
            for c in 0..3 {
                d[c] = (s[c] as f32 * a + d[c] as f32 * (1.0 - a)).round() as u8;
            }
        }
    }

    fn decode(project: &Project, c: &Clip, t_rel: Time, cw: u32, ch: u32) -> Option<cl_media::RgbaFrame> {
        let ClipContent::Media { media, .. } = &c.content else { return None };
        let m = project.media(*media)?;
        let (w, h) = decode_size(c, m, egui::vec2(cw as f32, ch as f32), 1.0);
        let lut = c.fx.lut.as_deref().filter(|p| std::path::Path::new(p).is_file()).map(cl_media::escape_path);
        let graph = cl_fx::preview_graph(c, w, h, t_rel.as_secs_f64(), None, lut);
        let src = still_src(c, m, t_rel, project.settings.fps);
        cl_media::decode_frame_fx(&m.path, src, w, h, graph.as_deref()).ok()
    }

    /// RGBA `cw`×`ch` des Projekts zur Zeit `t` auf Schwarz.
    pub(crate) fn render(project: &Project, t: Time, cw: u32, ch: u32) -> Vec<u8> {
        let (pw, ph) = (project.settings.width.max(2) as f32, project.settings.height.max(2) as f32);
        let layers = layers_in(project, None, t);
        let pairs = mix_pairs(project, &layers, t);
        let mut out = [0u8, 0, 0, 255].repeat((cw * ch) as usize);
        for (i, l) in layers.iter().enumerate() {
            if pairs.iter().any(|p| p.1 == i) || !matches!(l.clip.content, ClipContent::Media { .. }) {
                continue;
            }
            let pair = pairs.iter().find(|p| p.0 == i);
            // Clip selbst, dann die Fassungen mit den Effekt-Elementen darüber (mit ihrer Stärke)
            let mut versions = vec![(None, 1.0)];
            for e in fx_elements_over(project, l.clip.id).into_iter().filter(|e| e.contains(t)) {
                versions.push((Some(e.clone()), 0.0));
            }
            // Mischbild mit den unmaskierten Elementen eingerechnet (wie im Player); sonst darüber
            let mut base: Option<Vec<u8>> = None;
            for (e, _) in versions {
                let with = |c: &Clip| match &e {
                    Some(e) => fx_version(c, e, t),
                    None => Some((c.clone(), 1.0)),
                };
                let Some((ca, k)) = with(&l.clip) else { continue };
                let Some(fa) = decode(project, &ca, l.t_rel, cw, ch) else { continue };
                let ea = l.clip.eval_at(l.t_rel, pw, ph);
                let img = match pair {
                    Some(&(_, bi, ref plan, p)) => {
                        let b = &layers[bi];
                        let Some((cb, _)) = with(&b.clip) else { continue };
                        let Some(fb) = decode(project, &cb, b.t_rel, cw, ch) else { continue };
                        let eb = b.clip.eval_at(b.t_rel, pw, ph);
                        let Some(m) = mix_layers(plan, (&fa, &ea), (&fb, &eb), (cw, ch), (pw, ph), p) else { continue };
                        m
                    }
                    None => place_on_canvas(&fa, &Evaluated { opacity: ea.opacity * l.alpha, ..ea }, cw, ch, pw, ph),
                };
                match (&mut base, pair, e.as_ref().map(|e| e.fx.mask.is_none())) {
                    (None, Some(_), None) => base = Some(img),
                    (Some(b), Some(_), Some(true)) => fold_fx(b, &img, k),
                    // shortcut: maskierte Elemente ohne Maske darübergelegt (der Player zeichnet die Form)
                    _ => over(&mut out, &img, k),
                }
            }
            if let Some(b) = base {
                over(&mut out, &b, 1.0);
            }
        }
        out
    }

    /// Nachstellen von Hand: `CUTLINE_STILL="<projekt>;<sek>[,<sek>…];<ausgabeordner>"`.
    #[test]
    fn still_from_env() {
        let Ok(v) = std::env::var("CUTLINE_STILL") else { return };
        let parts: Vec<&str> = v.split(';').collect();
        let project = Project::from_json(&std::fs::read_to_string(parts[0]).unwrap()).unwrap();
        let reg = cl_plugin::Registry::scan([std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins"))]);
        cl_fx::catalog::load_packs(crate::contents::catalog_packs(&reg, "de"));
        let (cw, ch) = (640, 360);
        for t in parts[1].split(',') {
            let img = render(&project, Time::from_secs_f64(t.parse().unwrap()), cw, ch);
            let out = std::path::Path::new(parts[2]).join(format!("still_{t}.png"));
            image::save_buffer(&out, &img, cw, ch, image::ExtendedColorType::Rgba8).unwrap();
        }
    }
}

/// Übergänge der Vorschau gegen den Export (N_tr: Würfel, Verpixeln mit Maske, Kreis öffnen).
#[cfg(test)]
mod transition_tests {
    use super::*;
    use std::path::Path;

    fn load_shipped_packs() {
        let reg = cl_plugin::Registry::scan([Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins"))]);
        cl_fx::catalog::load_packs(crate::contents::catalog_packs(&reg, "de"));
    }

    fn media(dir: &Path, name: &str, src: &str) -> MediaRef {
        let path = dir.join(name);
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-threads", "2", "-f", "lavfi", "-i"])
            .arg(format!("{src}=size=320x180:rate=30:duration=2"))
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(st.success());
        cl_media::probe(&path).unwrap()
    }

    /// 320×180, 30 Bilder/s: A (0–1,5 s) → B (1,5–3 s), Übergang `tr` über 0,6 s (Fenster 1,2–1,8 s);
    /// `mask_b`: B mit Kreismaske; `filter`: Filter-Element über beiden.
    fn project(dir: &Path, tr: &str, mask_b: bool, filter: Option<&str>) -> Project {
        let a = media(dir, "a.mp4", "testsrc2");
        let b = media(dir, "b.mp4", "smptebars");
        let mut p = Project::new("Übergang");
        (p.settings.width, p.settings.height) = (320, 180);
        p.settings.fps = Rational { num: 30, den: 1 };
        let mut t = Track::new(TrackKind::Video);
        for (i, m) in [&a, &b].into_iter().enumerate() {
            let mut c = Clip::new(
                ClipContent::Media { media: m.id, kind: MediaKind::Video },
                Time::from_secs_f64(1.5 * i as f64),
                Time::from_secs_f64(1.5),
            );
            if i == 0 {
                c.transition_out = Some(TransitionRef { id: tr.into(), duration: Time::from_secs_f64(0.6) });
            } else if mask_b {
                c.fx.mask = Some(Mask::new(MaskShape::Circle));
            }
            t.clips.push(c);
        }
        p.media = vec![a, b];
        p.timeline.tracks.push(t);
        if let Some(f) = filter {
            let mut ft = Track::new(TrackKind::Filter);
            ft.clips.push(Clip::new(ClipContent::Filter(FxRef::new(f)), Time::from_secs_f64(1.0), Time::from_secs_f64(1.0)));
            p.timeline.tracks.push(ft);
        }
        p
    }

    /// Bild `n` des Exports (bildgenau)
    fn export_frame(p: &Project, dir: &Path, n: usize) -> Vec<u8> {
        let out = dir.join("out.mp4");
        let job = cl_export::build(p, &cl_export::ExportSettings::from_project(p), &|_| None, &out).unwrap();
        cl_export::run(&job, std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), |_| {}).unwrap();
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-threads", "2", "-i"])
            .arg(&out)
            .args(["-vf", &format!("select=eq(n\\,{n})"), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .output()
            .unwrap();
        assert_eq!(o.stdout.len(), 320 * 180 * 4);
        o.stdout
    }

    /// Mittlere Abweichung der Farben in 16×16-Blöcken (Kodierung und verschobene Pixelblöcke
    /// mitteln sich heraus, Form und Helligkeit nicht)
    fn block_diff(a: &[u8], b: &[u8]) -> f64 {
        let (w, h, s) = (320usize, 180usize, 16usize);
        let mean = |img: &[u8], bx: usize, by: usize, c: usize| {
            let mut sum = 0u32;
            for y in by * s..((by + 1) * s).min(h) {
                for x in bx * s..(bx + 1) * s {
                    sum += img[(y * w + x) * 4 + c] as u32;
                }
            }
            sum as f64 / (s * s.min(h - by * s)) as f64
        };
        let (mut d, mut n) = (0.0, 0.0);
        for by in 0..h.div_ceil(s) {
            for bx in 0..w / s {
                for c in 0..3 {
                    d += (mean(a, bx, by, c) - mean(b, bx, by, c)).abs();
                    n += 1.0;
                }
            }
        }
        d / n
    }

    fn still_vs_export(tr: &str, mask_b: bool, filter: Option<&str>, t: f64) -> f64 {
        let dir = tempfile::tempdir().unwrap();
        let p = project(dir.path(), tr, mask_b, filter);
        let ex = export_frame(&p, dir.path(), (t * 30.0).round() as usize);
        let pv = still::render(&p, Time::from_secs_f64(t), 320, 180);
        if let Ok(d) = std::env::var("CUTLINE_STILL_DEBUG") {
            let (d, tr) = (Path::new(&d), tr.replace('/', "_"));
            image::save_buffer(d.join(format!("t_{tr}_pv.png")), &pv, 320, 180, image::ExtendedColorType::Rgba8).unwrap();
            image::save_buffer(d.join(format!("t_{tr}_ex.png")), &ex, 320, 180, image::ExtendedColorType::Rgba8).unwrap();
        }
        let d = block_diff(&pv, &ex);
        eprintln!("{tr} bei {t} s: Abweichung Vorschau/Export {d:.2}");
        d
    }

    /// Verpixeln mit maskiertem Clip B (N_tr 32:13): außerhalb der Maske geht A wie im Export in
    /// Schwarz über, statt dass B ohne Maske durchscheint.
    #[test]
    fn masked_clip_pixelize_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = still_vs_export("pixelize", true, None, 1.6);
        assert!(d < 8.0, "Vorschau weicht vom Export ab: {d:.1}");
    }

    /// Zuschnitt (verschoben, verkleinert, gedreht) allein und in einem gemischten Übergang mit
    /// anderem Zuschnitt bei B: Vorschau wie Export.
    #[test]
    fn crop_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project(dir.path(), "circleopen", false, None);
        let clips = &mut p.timeline.tracks[0].clips;
        clips[0].crop = Crop { left: 0.3, right: 0.0, top: 0.2, bottom: 0.1 };
        clips[0].transform = Transform { x: 30.0, y: -10.0, scale: 0.7, rotation_deg: 20.0 };
        clips[1].crop = Crop { left: 0.0, right: 0.25, top: 0.0, bottom: 0.3 };
        for t in [0.5f64, 1.5, 2.5] {
            let ex = export_frame(&p, dir.path(), (t * 30.0).round() as usize);
            let pv = still::render(&p, Time::from_secs_f64(t), 320, 180);
            let d = block_diff(&pv, &ex);
            assert!(d < 8.0, "Zuschnitt bei {t} s: Vorschau weicht vom Export ab: {d:.1}");
            // ohne Zuschnitt in der Vorschau wäre es deutlich anders (prüft, dass der Test greift)
            let mut q = p.clone();
            q.timeline.tracks[0].clips.iter_mut().for_each(|c| c.crop = Crop::default());
            let d0 = block_diff(&still::render(&q, Time::from_secs_f64(t), 320, 180), &ex);
            assert!(d0 > 2.0 * d.max(2.0), "bei {t} s: ohne Zuschnitt {d0:.1}, mit {d:.1}");
        }
    }

    /// Splitscreen aus der Layout-Aktion (Zuschnitt + Transform zweier Spuren): Vorschau wie Export.
    #[test]
    fn splitscreen_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project(dir.path(), "fade", false, None);
        // B auf eine eigene Spur über A, beide ab 0
        let mut b = p.timeline.tracks[0].clips.pop().unwrap();
        p.timeline.tracks[0].clips[0].transition_out = None;
        b.start = Time::ZERO;
        let mut t = Track::new(TrackKind::Video);
        t.clips.push(b);
        p.timeline.tracks.push(t);
        let before = p.clone();
        let sizes: Vec<(f32, f32)> = p.media.iter().map(|m| (m.width as f32, m.height as f32)).collect();
        let geo = crate::arrange::arrange(320.0, 180.0, &sizes, crate::arrange::Arrange::Side);
        for (t, (tf, k)) in p.timeline.tracks.iter_mut().zip(geo) {
            (t.clips[0].transform, t.clips[0].crop) = (tf, k);
        }
        let ex = export_frame(&p, dir.path(), 15);
        let at = Time::from_secs_f64(0.5);
        let d = block_diff(&still::render(&p, at, 320, 180), &ex);
        assert!(d < 8.0, "Splitscreen: Vorschau weicht vom Export ab: {d:.1}");
        let d0 = block_diff(&still::render(&before, at, 320, 180), &ex);
        assert!(d0 > 2.0 * d.max(2.0), "ohne Layout {d0:.1}, mit {d:.1}");
    }

    /// Kreis öffnen unter einem Filter-Element (N_tr 26:50): gemischt wie der Export (Element auf
    /// dem fertigen Übergang), nicht übergeblendet.
    #[test]
    fn circleopen_under_filter_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = still_vs_export("circleopen", false, Some("noir"), 1.5);
        assert!(d < 8.0, "Vorschau weicht vom Export ab: {d:.1}");
    }

    /// Würfel (Paket-Shader) nach maskiertem Clip unter einem Filter-Element (N_tr 37:90).
    #[test]
    fn shader_transition_under_filter_matches_export() {
        if !cl_media::ffmpeg_available() || cl_gpu::gpu().is_none() {
            eprintln!("kein FFmpeg oder keine GPU, übersprungen");
            return;
        }
        load_shipped_packs();
        let d = still_vs_export("cutline.uebergaenge/wuerfel", false, Some("noir"), 1.5);
        assert!(d < 8.0, "Vorschau weicht vom Export ab: {d:.1}");
    }

    /// Entscheidung „mischen oder überblenden“: ein Effekt-/Filter-Element über dem Übergang ist
    /// kein Grund mehr zum Überblenden (vorher: Würfel und Kreis öffnen fehlten in N_tr).
    #[test]
    fn elements_above_do_not_force_a_crossfade() {
        let clip = |s: f64, tr: Option<&str>| {
            let mut c = Clip::new(
                ClipContent::Media { media: MediaId::new(), kind: MediaKind::Video },
                Time::from_secs_f64(s),
                Time::from_secs_f64(1.5),
            );
            c.transition_out = tr.map(|id| TransitionRef { id: id.into(), duration: Time::from_secs_f64(0.6) });
            c
        };
        let make = |tr: &str, filter: bool| {
            let mut p = Project::new("x");
            let mut t = Track::new(TrackKind::Video);
            t.clips = vec![clip(0.0, Some(tr)), clip(1.5, None)];
            p.timeline.tracks.push(t);
            if filter {
                let mut ft = Track::new(TrackKind::Filter);
                ft.clips.push(Clip::new(ClipContent::Filter(FxRef::new("noir")), Time::from_secs_f64(1.0), Time::from_secs_f64(1.0)));
                p.timeline.tracks.push(ft);
            }
            p
        };
        let at = Time::from_secs_f64(1.5);
        let pairs = |p: &Project| mix_pairs(p, &layers_in(p, None, at), at).into_iter().map(|x| (x.0, x.1, x.3)).collect::<Vec<_>>();
        for tr in ["circleopen", "pixelize", "fade"] {
            assert_eq!(pairs(&make(tr, true)), vec![(0, 1, 0.5)], "{tr} mit Filter darüber");
        }
        assert_eq!(pairs(&make("circleopen", false)), vec![(0, 1, 0.5)]);
        // reines Überblenden ohne Element: bleibt die (genaue) Überblendung der Anzeige
        assert!(pairs(&make("fade", false)).is_empty());
        // außerhalb des Fensters nichts
        let p = make("circleopen", true);
        assert!(mix_pairs(&p, &layers_in(&p, None, Time::from_secs_f64(1.1)), Time::from_secs_f64(1.1)).is_empty());
    }

    /// Fehlt beim Weiterschalten ein Bild, bleibt das letzte Mischbild desselben Übergangs stehen;
    /// zu langsam beim Abspielen gilt je Übergang bis zum Anhalten, ein schnelles Bild schaltet
    /// nicht zurück.
    #[test]
    fn missing_frames_hold_and_slow_sticks_per_transition() {
        let ctx = egui::Context::default();
        let (a, b, c) = (ClipId::new(), ClipId::new(), ClipId::new());
        let mut sp = ShaderPreview::default();
        assert_eq!(sp.step((a, b), false), Step::Skip);
        assert_eq!(sp.step((a, b), true), Step::Mix);
        sp.tex = Some(ctx.load_texture("t", egui::ColorImage::new([1, 1], Color32::BLACK), Default::default()));
        sp.shown = Some((a, b));
        assert_eq!(sp.step((a, b), false), Step::Hold);
        assert_eq!(sp.step((b, c), false), Step::Skip, "anderer Übergang: nicht das alte Bild");
        let iv = 1.0 / 30.0;
        // erstes langsames Bild (z. B. Pipeline) allein schaltet nicht um
        sp.note_time((a, b), 0.2, iv);
        assert_eq!(sp.step((a, b), true), Step::Mix);
        sp.note_time((a, b), 0.01, iv);
        sp.note_time((a, b), 0.2, iv);
        assert_eq!(sp.step((a, b), true), Step::Skip);
        sp.note_time((a, b), 0.01, iv);
        assert_eq!(sp.step((a, b), true), Step::Skip, "bleibt bis zum Anhalten");
        // der nächste Übergang beginnt neu
        sp.note_time((b, c), 0.2, iv);
        assert_eq!(sp.step((b, c), true), Step::Mix);
    }

    #[test]
    fn background_mix_matches_direct_mix_with_elements() {
        // gleiche Rechnung wie vorher im Oberflächen-Thread: Mischbild, unmaskiertes Element
        // eingerechnet, maskiertes als eigenes Bild
        let ev = Evaluated { x: 0.0, y: 0.0, scale: 1.0, rotation_deg: 0.0, opacity: 1.0, volume_db: 0.0, crop: Crop::default() };
        let f = |v: u8| std::sync::Arc::new(cl_media::RgbaFrame { width: 32, height: 18, data: [v, 255 - v, 90, 255].repeat(32 * 18) });
        let plan = cl_fx::catalog::transition_plan("circleopen");
        let mask = Mask::new(MaskShape::Circle);
        let job = MixJob {
            id: 7,
            pair: (ClipId::new(), ClipId::new()),
            plan: plan.clone(),
            frames: vec![(f(10), f(200)), (f(60), f(120)), (f(250), f(5))],
            sets: vec![(None, 0.5), (Some(mask), 0.8)],
            ev: (ev, ev),
            size: (32, 18),
            proj: (1920.0, 1080.0),
            p: 0.4,
        };
        let d = run_mix(job);
        assert_eq!((d.id, d.images.len(), d.masked.len()), (7, 2, 1));
        let mix = |a: u8, b: u8| mix_layers(&plan, (&f(a), &ev), (&f(b), &ev), (32, 18), (1920.0, 1080.0), 0.4).unwrap();
        let mut base = mix(10, 200);
        fold_fx(&mut base, &mix(60, 120), 0.5);
        assert_eq!(d.images[0], egui::ColorImage::from_rgba_unmultiplied([32, 18], &base));
        assert_eq!(d.images[1], egui::ColorImage::from_rgba_unmultiplied([32, 18], &mix(250, 5)));
        assert_eq!(d.masked[0], (Some(mask), 0.8));
    }

    #[test]
    fn mix_area_is_limited_while_playing() {
        let px = egui::vec2(2560.0, 1440.0);
        assert_eq!(mix_size(1920.0, 1080.0, px, false), (1920, 1080));
        assert_eq!(mix_size(1920.0, 1080.0, px, true), (960, 540));
        assert_eq!(mix_size(1080.0, 1920.0, egui::vec2(400.0, 700.0), true), (394, 700));
    }

    /// Zeit je Mischbild (1080p-Projekt): `cargo test --release -p cl-app mix_time -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mix_time() {
        load_shipped_packs();
        let ev = Evaluated { x: 0.0, y: 0.0, scale: 1.0, rotation_deg: 0.0, opacity: 1.0, volume_db: 0.0, crop: Crop::default() };
        for (cw, ch) in [(1920, 1080), (1600, 900), (1280, 720), (960, 540)] {
            let f = |v: u8| cl_media::RgbaFrame { width: cw, height: ch, data: [v, 255 - v, 90, 255].repeat((cw * ch) as usize) };
            let (fa, fb) = (f(30), f(200));
            let t0 = std::time::Instant::now();
            for _ in 0..5 {
                place_on_canvas(&fa, &ev, cw, ch, 1920.0, 1080.0);
            }
            eprintln!("{cw}x{ch} Ebene setzen: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3 / 5.0);
            for id in ["circleopen", "pixelize", "cutline.uebergaenge/wuerfel"] {
                let plan = cl_fx::catalog::transition_plan(id);
                let t0 = std::time::Instant::now();
                mix_layers(&plan, (&fa, &ev), (&fb, &ev), (cw, ch), (1920.0, 1080.0), 0.4);
                let first = t0.elapsed();
                let t0 = std::time::Instant::now();
                for i in 0..5 {
                    mix_layers(&plan, (&fa, &ev), (&fb, &ev), (cw, ch), (1920.0, 1080.0), 0.5 + i as f32 * 0.01);
                }
                eprintln!("{cw}×{ch} {id}: erstes {:.1} ms, danach {:.1} ms", first.as_secs_f64() * 1e3, t0.elapsed().as_secs_f64() * 1e3 / 5.0);
            }
        }
    }

    /// Lage des gelben Bereichs (Mitte, linker und rechter Rand) im Verhältnis zur Textbreite in
    /// der Vorschau: dieselbe Zeichnung wie im Player, ausgewertet an den Glyphen
    fn karaoke_preview(app: &App, st: &TextStyle, dur: Time, t: f64) -> (f32, f32, f32) {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 180.0)));
        app.draw_text(&painter, st, pos2(160.0, 90.0), 1.0, 0.0, 1.0, 320.0, st.karaoke_lit(dur, Time::from_secs_f64(t)));
        let out = ctx.end_pass();
        let hi = st.karaoke.as_ref().unwrap().color;
        let hi = Color32::from_rgba_unmultiplied(hi[0], hi[1], hi[2], hi[3]);
        let (mut all, mut lit) = ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
        for s in &out.shapes {
            let egui::Shape::Text(ts) = &s.shape else { continue };
            for g in ts.galley.rows.iter().flat_map(|r| r.glyphs.iter()).filter(|g| !g.chr.is_whitespace()) {
                let (x0, x1) = (ts.pos.x + g.pos.x, ts.pos.x + g.pos.x + g.advance_width);
                all = (all.0.min(x0), all.1.max(x1));
                if ts.galley.job.sections[g.section_index as usize].format.color == hi {
                    lit = (lit.0.min(x0), lit.1.max(x1));
                }
            }
        }
        let fr = |x: f32| (x - all.0) / (all.1 - all.0);
        (fr((lit.0 + lit.1) / 2.0), fr(lit.0), fr(lit.1))
    }

    /// Dasselbe im Exportbild: gelbe Spalten gegen alle Textspalten
    fn karaoke_export(img: &[u8]) -> (f32, f32, f32) {
        let (mut all, mut lit) = ((usize::MAX, 0), (usize::MAX, 0));
        for (i, c) in img.chunks_exact(4).enumerate() {
            let x = i % 320;
            let yellow = c[0] > 170 && c[1] > 140 && c[2] < 100;
            if yellow || c.iter().take(3).all(|&v| v > 170) {
                all = (all.0.min(x), all.1.max(x + 1));
            }
            if yellow {
                lit = (lit.0.min(x), lit.1.max(x + 1));
            }
        }
        assert!(lit.0 < lit.1, "kein gelbes Wort im Export");
        let fr = |x: usize| (x as f32 - all.0 as f32) / (all.1 - all.0) as f32;
        ((fr(lit.0) + fr(lit.1)) / 2.0, fr(lit.0), fr(lit.1))
    }

    /// Mitte jeder Zeile (y, Projektpixel) in der Vorschau: Zeilen der Galley des Haupttexts
    fn preview_lines(app: &App, st: &TextStyle) -> Vec<f32> {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 360.0)));
        app.draw_text(&painter, st, pos2(320.0, 180.0), 1.0, 0.0, 1.0, 640.0, None);
        let out = ctx.end_pass();
        let ts = out.shapes.iter().rev().find_map(|s| match &s.shape {
            egui::Shape::Text(ts) => Some(ts.clone()),
            _ => None,
        });
        let ts = ts.unwrap();
        ts.galley.rows.iter().filter(|r| !r.glyphs.is_empty()).map(|r| ts.pos.y + r.rect.center().y).collect()
    }

    /// Mitte jeder Zeile im Exportbild: Bänder heller Pixel (Großbuchstaben ohne Unterlängen,
    /// deshalb wird die Mitte der Versalhöhe gemessen, nicht die der Zeilenbox)
    fn export_lines(img: &[u8]) -> Vec<f32> {
        let ink: Vec<bool> = (0..360).map(|y| (0..640).any(|x| img[(y * 640 + x) * 4..][..3].iter().all(|&v| v > 160))).collect();
        let mut v = Vec::new();
        let mut start = None;
        for (y, &on) in ink.iter().chain([false].iter()).enumerate() {
            match (on, start) {
                (true, None) => start = Some(y),
                (false, Some(s)) => {
                    v.push((s + y) as f32 / 2.0);
                    start = None;
                }
                _ => {}
            }
        }
        v
    }

    /// Zeilenlage und -abstand: zwei-, dreizeilig und automatisch umbrochen. Die Vorschau setzt
    /// mit egui (eigene Schrift), der Export mit `drawtext`; verglichen werden Zeilenzahl, Abstand
    /// der Zeilen und die Mitte des Blocks.
    #[test]
    fn text_lines_match_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let (app, _home, _g) = crate::saving::tests::test_app();
        let dir = tempfile::tempdir().unwrap();
        for text in ["AAA BBB\nCCC DDD", "EINS\nZWEI\nDREI", "HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH"] {
            let st = TextStyle { text: text.into(), size: 44.0, shadow: false, ..Default::default() };
            let mut p = Project::new("Zeilen");
            (p.settings.width, p.settings.height) = (640, 360);
            p.settings.fps = Rational { num: 30, den: 1 };
            let mut t = Track::new(TrackKind::Text);
            t.clips.push(Clip::new(ClipContent::Text(st.clone()), Time::ZERO, Time::from_secs_f64(1.0)));
            p.timeline.tracks.push(t);
            let out = dir.path().join("z.mp4");
            let job = cl_export::build(&p, &cl_export::ExportSettings::from_project(&p), &|_| None, &out).unwrap();
            cl_export::run(&job, std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), |_| {}).unwrap();
            let o = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-i"])
                .arg(&out)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
                .output()
                .unwrap();
            let (pv, ex) = (preview_lines(&app, &st), export_lines(&o.stdout));
            let pitch = |v: &[f32]| (v[v.len() - 1] - v[0]) / (v.len() - 1) as f32;
            let mid = |v: &[f32]| (v[0] + v[v.len() - 1]) / 2.0;
            eprintln!("{text:?}: Vorschau {pv:.1?}, Export {ex:.1?}");
            assert_eq!(pv.len(), ex.len(), "{text:?}: Zeilenzahl");
            assert!(pv.len() >= 2, "{text:?}");
            let (dp, dm) = ((pitch(&pv) - pitch(&ex)).abs(), (mid(&pv) - mid(&ex)).abs());
            eprintln!("{text:?}: Abweichung Zeilenabstand {dp:.1} px, Blockmitte {dm:.1} px (Größe 44)");
            assert!(dp < 2.0 && dm < 5.0, "{text:?}: Abstand {dp:.1}, Mitte {dm:.1}");
        }
    }

    /// Karaoke an drei Zeitpunkten (und „Füllen“): in Vorschau und Export ist dasselbe Wort gelb.
    /// Die Vorschau nutzt die Schrift von egui, deshalb wird die Lage im Verhältnis zur Textbreite
    /// verglichen.
    #[test]
    fn karaoke_word_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let (app, _home, _g) = crate::saving::tests::test_app();
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new("Karaoke");
        (p.settings.width, p.settings.height) = (320, 180);
        p.settings.fps = Rational { num: 30, den: 1 };
        let words = [0.0, 1.0, 2.0].map(Time::from_secs_f64).to_vec();
        let mut st = TextStyle {
            text: "AAA BBB CCC".into(),
            size: 36.0,
            shadow: false,
            karaoke: Some(Karaoke { color: [255, 221, 0, 255], fill: false, words }),
            ..Default::default()
        };
        let dur = Time::from_secs_f64(3.0);
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(ClipContent::Text(st.clone()), Time::ZERO, dur));
        p.timeline.tracks.push(t);
        for (t, word) in [(0.5f64, 0usize), (1.5, 1), (2.5, 2)] {
            let pv = karaoke_preview(&app, &st, dur, t);
            let ex = karaoke_export(&export_frame(&p, dir.path(), (t * 30.0).round() as usize));
            eprintln!("Karaoke bei {t} s: Vorschau {pv:.2?}, Export {ex:.2?}");
            assert_eq!(((pv.0 * 3.0) as usize, (ex.0 * 3.0) as usize), (word, word), "bei {t} s");
            assert!((pv.0 - ex.0).abs() < 0.06, "bei {t} s: Vorschau {pv:?}, Export {ex:?}");
            // nur das eine Wort: nichts färbt in die Nachbarn
            assert!(ex.2 - ex.1 < 0.36 && pv.2 - pv.1 < 0.36, "bei {t} s: {pv:?} {ex:?}");
        }
        // Füllen: bei 1,5 s sind die ersten beiden Wörter gelb
        st.karaoke.as_mut().unwrap().fill = true;
        p.timeline.tracks[0].clips[0].content = ClipContent::Text(st.clone());
        let pv = karaoke_preview(&app, &st, dur, 1.5);
        let ex = karaoke_export(&export_frame(&p, dir.path(), 45));
        assert!(pv.1 < 0.05 && ex.1 < 0.05 && (0.55..0.72).contains(&pv.2) && (0.55..0.72).contains(&ex.2), "{pv:?} {ex:?}");
    }

    /// Feines Schachbrett (2 px) über 2 s, verlustfrei kodiert
    fn checker(dir: &Path) -> MediaRef {
        let path = dir.join("muster.mp4");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-threads", "2", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=30:d=2"])
            .args(["-vf", "format=gray,geq=lum='255*mod(floor(X/2)+floor(Y/2),2)'", "-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv444p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(st.success());
        cl_media::probe(&path).unwrap()
    }

    /// Ein Clip (0–2 s) mit dem Schachbrett; Zensur wandert von links (x 0,3) nach rechts (0,7)
    fn censor_project(dir: &Path, blur: bool) -> Project {
        let m = checker(dir);
        let mut p = Project::new("Zensur");
        (p.settings.width, p.settings.height) = (320, 180);
        p.settings.fps = Rational { num: 30, den: 1 };
        let mut t = Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(2.0));
        let key = |t: f64, cx: f32| CensorKey { t: Time::from_secs_f64(t), cx, cy: 0.5, w: 0.3, h: 0.5 };
        c.fx.censor = vec![Censor { blur, path: vec![key(0.0, 0.3), key(2.0, 0.7)], ..Default::default() }];
        t.clips.push(c);
        p.media = vec![m];
        p.timeline.tracks.push(t);
        p
    }

    /// Mittlerer Unterschied benachbarter Pixel (Rot) im Bereich – hoch beim feinen Muster
    fn detail(img: &[u8], x0: usize, x1: usize, y0: usize, y1: usize) -> f64 {
        let at = |x: usize, y: usize| img[(y * 320 + x) * 4] as f64;
        let (mut d, mut n) = (0.0, 0.0);
        for y in y0..y1 {
            for x in x0..x1 {
                d += (at(x, y) - at(x + 1, y)).abs() + (at(x, y) - at(x, y + 1)).abs();
                n += 1.0;
            }
        }
        d / n
    }

    /// Feinheit je 16×16-Block: mittlere Abweichung zweier Bilder
    fn detail_diff(a: &[u8], b: &[u8]) -> f64 {
        let (mut d, mut n) = (0.0, 0.0);
        for by in 0..11 {
            for bx in 0..19 {
                let r = (bx * 16, bx * 16 + 16, by * 16, by * 16 + 16);
                d += (detail(a, r.0, r.1, r.2, r.3) - detail(b, r.0, r.1, r.2, r.3)).abs();
                n += 1.0;
            }
        }
        d / n
    }

    /// Export: unter der Fläche ist das feine Muster weg (Verpixeln und Weichzeichnen, schwächste
    /// Stufe), daneben ist das Bild unverändert, und die Fläche liegt bei 0,2 s links, bei 1,8 s rechts.
    #[test]
    fn censor_destroys_the_pattern_only_inside_and_moves() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut plain = censor_project(dir.path(), false);
        plain.timeline.tracks[0].clips[0].fx.censor.clear();
        let orig = [export_frame(&plain, dir.path(), 6), export_frame(&plain, dir.path(), 54)];
        // Fläche bei Bild 6: x 61–157, bei Bild 54: x 163–259; y 45–135
        let (left, right) = ((66, 152), (168, 254));
        assert!(detail(&orig[0], left.0, left.1, 50, 130) > 60.0, "Muster kommt im Export an");
        for blur in [false, true] {
            let p = censor_project(dir.path(), blur);
            for (n, inside, outside, o) in [(6, left, right, &orig[0]), (54, right, left, &orig[1])] {
                let f = export_frame(&p, dir.path(), n);
                let (di, dout) = (detail(&f, inside.0, inside.1, 50, 130), detail(&f, outside.0, outside.1, 50, 130));
                eprintln!("blur={blur} Bild {n}: Feinheit innen {di:.1}, an der anderen Stelle {dout:.1}");
                assert!(di < 6.0, "blur={blur} Bild {n}: Muster unter der Fläche noch da ({di:.1})");
                assert!(dout > 60.0, "blur={blur} Bild {n}: Muster neben der Fläche fehlt ({dout:.1})");
                // weit genug weg von der Fläche: wie ohne Zensur
                let far = if n == 6 { 175..319 } else { 0..145 };
                let diff: f64 = (0..180).flat_map(|y| far.clone().map(move |x| (x, y))).map(|(x, y)| (f[(y * 320 + x) * 4] as f64 - o[(y * 320 + x) * 4] as f64).abs()).sum::<f64>()
                    / (180 * far.len()) as f64;
                assert!(diff < 3.0, "blur={blur} Bild {n}: außerhalb verändert ({diff:.1})");
            }
        }
    }

    /// Vorschau gegen Export an zwei Zeitpunkten (die Fläche wandert dazwischen).
    #[test]
    fn censor_preview_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = censor_project(dir.path(), false);
        let mut q = p.clone();
        q.timeline.tracks[0].clips[0].fx.censor.clear();
        for (t, n) in [(0.2, 6), (1.8, 54)] {
            let ex = export_frame(&p, dir.path(), n);
            let pv = still::render(&p, Time::from_secs_f64(t), 320, 180);
            let (d, c) = (detail_diff(&pv, &ex), block_diff(&pv, &ex));
            let d0 = detail_diff(&still::render(&q, Time::from_secs_f64(t), 320, 180), &ex);
            eprintln!("Zensur bei {t} s: Feinheit Vorschau/Export {d:.2} (ohne Zensur {d0:.2}), Farbe {c:.2}");
            assert!(d < 6.0 && c < 8.0, "bei {t} s: Vorschau weicht vom Export ab ({d:.1}, {c:.1})");
            assert!(d0 > 3.0 * d.max(2.0), "bei {t} s: Test greift nicht ({d0:.1} / {d:.1})");
        }
    }

    /// Mit Zuschnitt, Skalierung und Verschiebung des Clips liegt die Fläche weiter auf derselben
    /// Stelle des Quellbilds – in Vorschau und Export.
    #[test]
    fn censor_follows_crop_scale_and_position() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = censor_project(dir.path(), false);
        let c = &mut p.timeline.tracks[0].clips[0];
        c.crop = Crop { left: 0.05, right: 0.05, top: 0.1, bottom: 0.0 };
        c.transform = Transform { x: 40.0, y: 0.0, scale: 0.6, rotation_deg: 0.0 };
        // Bild 6: Fläche u 0,19–0,49, v 0,25–0,75 → x 160+40+(u−0,5)·192 = 144–201, y 90+(v−0,5)·108 = 63–117
        let ex = export_frame(&p, dir.path(), 6);
        let pv = still::render(&p, Time::from_secs_f64(0.2), 320, 180);
        for (what, img) in [("Export", &ex), ("Vorschau", &pv)] {
            let (di, dout) = (detail(img, 148, 197, 67, 113), detail(img, 212, 250, 67, 113));
            eprintln!("{what}: Feinheit auf der Fläche {di:.1}, daneben {dout:.1}");
            assert!(di < 6.0, "{what}: Fläche nicht auf dem Motiv ({di:.1})");
            assert!(dout > 15.0, "{what}: daneben zensiert ({dout:.1})");
        }
        // Feinheit daneben unterscheidet sich (Skalierung des Musters), die Farben nicht
        let d = block_diff(&pv, &ex);
        assert!(d < 8.0, "Vorschau weicht vom Export ab ({d:.1})");
    }
}

impl App {
    /// Fertige Decoder-Ergebnisse übernehmen.
    pub fn receive_decoded(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.decoder.rx.try_recv() {
            match d {
                Done::Frame { clip, key, frame } => {
                    if self.core.still_done(lid(clip), key) {
                        self.set_frame(ctx, clip, key, &frame);
                    }
                }
                Done::FrameFailed { clip, key, error } => {
                    log::warn!("Decoder: {error}");
                    self.core.still_failed(lid(clip), key);
                }
                Done::Thumb { media, frame } => {
                    let tex = ctx.load_texture(
                        format!("thumb-{media}"),
                        to_color_image(&frame),
                        egui::TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(media, tex);
                }
                Done::Wave { media, stream, peaks } => {
                    self.waves.insert((media, stream), peaks);
                }
                Done::SoundMeta { key, media, bars } => {
                    if self.sound_pending.as_deref() == Some(key.as_str()) {
                        self.sound_pending = None;
                        self.preview_media(key.clone(), media.clone());
                    }
                    crate::assets::put_capped(
                        &mut self.sound_meta,
                        &mut self.sound_meta_req,
                        key,
                        (media, bars),
                        crate::assets::SOUND_META_CAP,
                    );
                }
                Done::FxThumb { key, frame } => {
                    let tex = ctx.load_texture(
                        format!("fx-{key}"),
                        to_color_image(&frame),
                        egui::TextureOptions::LINEAR,
                    );
                    self.fx_thumbs.insert(key, tex);
                }
                Done::ProjectThumb { out } => self.home.thumb_ready(&out),
                Done::Skipped(key) => {
                    self.fx_thumbs_requested.remove(&key);
                }
                Done::Error(e) => log::warn!("Decoder: {e}"),
            }
        }
    }

    fn set_frame(&mut self, ctx: &egui::Context, clip: ClipId, key: u64, f: &cl_media::RgbaFrame) {
        if matches!(self.pick, Some(crate::pro::Pick::Chroma(c)) if c == clip) {
            self.frame_cpu = Some((clip, f.clone()));
        }
        if self.in_shader_transition(clip) || self.shader_preview.wanted.contains(&clip) {
            let cpu = &mut self.shader_preview.cpu;
            if cpu.len() >= 16 && !cpu.contains_key(&clip) {
                cpu.clear();
            }
            cpu.insert(clip, (key, std::sync::Arc::new(f.clone())));
        }
        let img = to_color_image(f);
        match self.frames.get_mut(&clip) {
            Some((k, t)) if t.size() == [f.width as usize, f.height as usize] => {
                t.set(img, egui::TextureOptions::LINEAR);
                *k = key;
            }
            _ => {
                let t =
                    ctx.load_texture(format!("frame-{clip}"), img, egui::TextureOptions::LINEAR);
                self.frames.insert(clip, (key, t));
            }
        }
    }

    /// Hat der Clip einen Shader- oder gemischten `xfade`-Übergang (nicht bloß Überblenden) am
    /// Anfang oder Ende? (Nur dann CPU-Bilder behalten.)
    fn in_shader_transition(&self, clip: ClipId) -> bool {
        let shader = |c: &Clip| {
            c.transition_out.as_ref().is_some_and(|t| {
                mixes(&cl_fx::catalog::transition_plan(&t.id))
            })
        };
        self.project.timeline.tracks.iter().any(|tr| {
            tr.clips.iter().enumerate().any(|(i, c)| {
                c.id == clip && (shader(c) || i.checked_sub(1).and_then(|p| tr.clips.get(p)).is_some_and(shader))
            })
        })
    }

    /// Steht der Playhead in einem Übergang, den die Vorschau mischt (`mix_pairs`), A und B wie
    /// der Export mischen (`mix_layers`), dazu je Effekt-Element darüber dessen Fassungen von A
    /// und B. Fehlt ein Bild, bleibt das letzte Mischbild dieses Übergangs stehen (das fehlende
    /// fordert beim Eintreffen ein Neuzeichnen an); gab es noch keins oder ist der Übergang beim
    /// Abspielen zu langsam: Überblendung.
    fn shader_mix(&mut self, ctx: &egui::Context, layers: &[Layer], canvas_px: egui::Vec2) {
        let sp = &mut self.shader_preview;
        sp.current = None;
        let mixer = sp.mixer.get_or_insert_with(|| Mixer::spawn(ctx.clone()));
        while let Ok(d) = mixer.rx.try_recv() {
            sp.pending = None;
            sp.ready = Some(d);
        }
        if !sp.warmed || (self.playing && !sp.was_playing) {
            warm_shaders(&self.project);
            sp.warmed = true;
        }
        sp.was_playing = self.playing;
        if !self.playing {
            (sp.slow, sp.slow_pair) = (Vec::new(), None);
        }
        let s = &self.project.settings;
        let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
        let t = self.playhead;
        let div = self.play_quality().0 as f32;
        let mut wanted = Vec::new();
        for (ai, bi, plan, p) in mix_pairs(&self.project, layers, t) {
            let (a, b) = (&layers[ai], &layers[bi]);
            let pair = (a.clip.id, b.clip.id);
            // Bildpaare: die Clips selbst, dann ihre Fassungen mit den Effekt-Elementen darüber
            let mut sets = vec![(pair.0, pair.1, None, 1.0)];
            for e in fx_elements_over(&self.project, pair.0).iter().filter(|e| e.contains(t)) {
                let Some((_, k)) = fx_version(&a.clip, e, t) else { continue };
                let d = e.id.0.as_u128();
                sets.push((pair.0.derived(d), pair.1.derived(d), e.fx.mask, k));
            }
            wanted.extend(sets.iter().flat_map(|x| [x.0, x.1]));
            let sp = &mut self.shader_preview;
            let frames: Option<Vec<_>> = sets.iter().map(|x| Some((sp.cpu.get(&x.0)?.clone(), sp.cpu.get(&x.1)?.clone()))).collect();
            match sp.step(pair, frames.is_some()) {
                Step::Skip => continue,
                Step::Hold => {
                    sp.current = Some(pair);
                    break;
                }
                Step::Mix => {}
            }
            let frames = frames.unwrap_or_default();
            let (ea, eb) = (a.clip.eval_at(a.t_rel, pw, ph), b.clip.eval_at(b.t_rel, pw, ph));
            let (cw, ch) = mix_size(pw, ph, canvas_px / div, self.playing);
            // andere Bilder, anderer Übergang, andere Lage oder Fläche: neu mischen
            let id = {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (plan.xfade, &plan.shader.as_ref().map(|s| &s.0), p.to_bits(), cw, ch).hash(&mut h);
                for e in [&ea, &eb] {
                    let k = e.crop;
                    [e.x, e.y, e.scale, e.rotation_deg, e.opacity, k.left, k.right, k.top, k.bottom].map(f32::to_bits).hash(&mut h);
                }
                for ((ka, _), (kb, _)) in &frames {
                    (ka, kb).hash(&mut h);
                }
                h.finish()
            };
            // fertige Mischung dieses Übergangs übernehmen
            if let Some(d) = sp.ready.take_if(|d| d.pair == pair) {
                let fps = self.project.settings.fps;
                let interval = fps.den.max(1) as f64 / fps.num.max(1) as f64;
                if self.core.mode() == Mode::Spielen {
                    sp.note_time(pair, d.secs, interval);
                }
                if crate::app::perf() {
                    crate::applog::perf_line!("perf: Übergang gemischt ({}×{}, {} Bild(er)) in {:.1} ms (im Hintergrund)", cw, ch, sets.len(), d.secs * 1e3);
                }
                sp.last = Some(d.id);
                if !d.images.is_empty() {
                    // vorhandene Texturen weiterverwenden: Mischbild, dann je Effekt-Element
                    let mut old = sp.tex.take().into_iter().chain(sp.fx.drain(..).map(|f| f.0));
                    let mut texs: Vec<egui::TextureHandle> = d
                        .images
                        .into_iter()
                        .enumerate()
                        .map(|(i, ci)| match old.next() {
                            Some(mut t) => {
                                t.set(ci, egui::TextureOptions::LINEAR);
                                t
                            }
                            None => ctx.load_texture(format!("shader-mix-{i}"), ci, egui::TextureOptions::LINEAR),
                        })
                        .collect();
                    drop(old);
                    let rest = texs.split_off(1);
                    sp.tex = texs.pop();
                    sp.fx = rest.into_iter().zip(d.masked).map(|(t, (m, k))| (t, m, k)).collect();
                    sp.shown = Some(pair);
                }
            }
            // andere Bilder, anderer Übergang, andere Lage oder Fläche: neu mischen (im Hintergrund)
            if sp.last != Some(id) && sp.pending.is_none() {
                let job = MixJob {
                    id,
                    pair,
                    plan: plan.clone(),
                    frames: frames.iter().map(|((_, a), (_, b))| (a.clone(), b.clone())).collect(),
                    sets: sets[1..].iter().map(|x| (x.2, x.3)).collect(),
                    ev: (ea, eb),
                    size: (cw, ch),
                    proj: (pw, ph),
                    p,
                };
                if let Some(m) = &sp.mixer {
                    if m.tx.send(job).is_ok() {
                        sp.pending = Some(id);
                    }
                }
            }
            // noch kein Mischbild dieses Übergangs: so lange überblenden
            if sp.shown != Some(pair) || sp.tex.is_none() {
                continue;
            }
            sp.current = Some(pair);
            break;
        }
        self.shader_preview.ready = None;
        self.shader_preview.wanted = wanted;
    }

    /// Signal „wartet auf Bild“ für die Warte-Linie (siehe [`crate::playcore::PlayCore::waiting`]).
    pub(crate) fn waiting_for_frame(&self) -> bool {
        self.core.waiting()
    }

    /// Vorschauqualität jetzt: (Teiler der Decodiergröße, halbe Bildrate). Beim Abspielen die
    /// Stufe der automatischen Qualität, im Stand die gewählte.
    pub(crate) fn play_quality(&self) -> (u32, bool) {
        let steps = self.quality_steps();
        if self.core.playing() {
            steps[self.core.quality.level.min(steps.len() - 1)]
        } else {
            steps[0]
        }
    }

    /// Fingerabdruck der Effekte eines Clips: je Bearbeitungsstand einmal berechnet (vorher
    /// serialisierte jedes UI-Bild die Effekte jeder Ebene zu JSON). `cacheable` = unverändert
    /// aus dem Projekt (nicht gerade gezogen, keine Pipette).
    fn fx_hash(&mut self, id: ClipId, c: &Clip, cacheable: bool) -> u64 {
        let calc = || fx_hash_of(c);
        if !cacheable {
            return calc();
        }
        if self.fx_hashes.0 != self.revision {
            self.fx_hashes = (self.revision, Default::default());
        }
        *self.fx_hashes.1.entry(id).or_insert_with(calc)
    }

    fn layers_at(&self, t: Time) -> Vec<Layer> {
        layers_in(&self.project, self.live.as_ref(), t)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame_key(
        id: ClipId,
        c: &Clip,
        src: Time,
        w: u32,
        h: u32,
        t_rel: Time,
        animated_fx: bool,
        fx: u64,
        fps: Rational,
    ) -> u64 {
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        (id, src, w, h, fps.num, fps.den, fx).hash(&mut hs);
        // Timing gehört zum Schlüssel: verschobene/getrimmte Clips brauchen einen neuen Strom
        (c.start, c.duration, c.source_in, c.speed.to_bits()).hash(&mut hs);
        if animated_fx {
            t_rel.hash(&mut hs);
        }
        hs.finish() | 1
    }

    /// Bilder für alle sichtbaren Medien-Ebenen anfordern bzw. aus Strömen holen.
    ///
    /// Flüssiges Abspielen: Der Strom eines Clips läuft ununterbrochen. Effekt-/Filter-Elemente
    /// darüber laufen als eigene Fassung des Clips (eigener Strom), die beim Zeichnen mit ihrer
    /// Stärke und ggf. nur innerhalb der Maske darübergelegt wird. Ströme für Clips und
    /// Elemente, die gleich beginnen, werden vorab geöffnet, damit an Schnitten nichts hakt.
    fn update_layers(&mut self, ctx: &egui::Context, layers: &[Layer], canvas_px: egui::Vec2) {
        // Beim Abspielen bleibt die Dekodiergröße fest (Umschalten auf den großen Player oder
        // Fenstergröße ändern startet die Ströme nicht neu); im Stand folgt sie der Ansicht.
        let canvas_px = if self.playing {
            *self.play_canvas_px.get_or_insert(canvas_px)
        } else {
            self.play_canvas_px = None;
            canvas_px
        };
        // im Hintergrund geöffnete Ströme anhängen (veraltete verwirft der Kern)
        let now = self.core_now();
        while let Ok((o, fps, alias, s)) = self.starter.rx.try_recv() {
            match s {
                Some(s) => {
                    self.core.attach_alias(o, fps, s, now, alias);
                }
                None => self.core.open_failed(o, now),
            }
        }
        // Pipette: Bild ohne Chroma-Key zeigen, damit die Farbe sichtbar bleibt
        let pick = match self.pick {
            Some(crate::pro::Pick::Chroma(id)) => Some(id),
            _ => None,
        };
        let needs = needs(&self.project, layers, self.playhead, self.playing, |c| {
            if pick == Some(c.id) {
                c.fx.chroma = None;
            }
        });
        let mut visible = Vec::with_capacity(needs.len());
        let mut overlays = Vec::new();
        for n in &needs {
            visible.push(n.id);
            if let Some((base, mk, k)) = n.overlay {
                overlays.push((base, n.id, mk, k));
            }
            self.request_frame(ctx, n.id, &n.clip, &n.media, &n.layer, canvas_px, n.prefetch);
        }
        self.fx_overlays = overlays;
        // Bilder von Clips, die nicht mehr unter dem Playhead liegen, verwirft `receive_decoded`
        // (beim Scrubben über viele Spuren kamen sonst Dutzende auf einmal); kehrt der Playhead
        // zurück, kommt das Bild erneut aus dem Zwischenspeicher des Decoders. Ebenso die
        // Texturen: sonst hielt ein großes Projekt nach dem Scrubben eine je Clip (in Vorschaugröße je 1–8 MB).
        self.core.visible(&visible.iter().map(|id| lid(*id)).collect::<Vec<_>>());
        self.frames.retain(|id, _| visible.contains(id));
    }

    /// Ein Bild für die Ebene anfordern (Strom beim Abspielen, sonst Einzelbild).
    #[allow(clippy::too_many_arguments)]
    fn request_frame(
        &mut self,
        ctx: &egui::Context,
        id: ClipId,
        c: &Clip,
        m: &MediaRef,
        l: &Layer,
        canvas_px: egui::Vec2,
        prefetch_at: Option<Time>,
    ) {
        let (div, half) = self.play_quality();
        let div = div as f32;
        let fps = self.project.settings.fps;
        // automatische Qualität, letzte Stufe: Vorschau mit halber Bildrate
        let stream_fps = if half { Rational::new(fps.num, fps.den * 2) } else { fps };
        if !matches!(&c.content, ClipContent::Media { .. }) {
            return;
        }
        let cacheable = self.live.as_ref().is_none_or(|lv| lv.id != l.clip.id)
            && !matches!(self.pick, Some(crate::pro::Pick::Chroma(p)) if p == l.clip.id);
        let fxh = self.fx_hash(id, c, cacheable);
        let now = self.core_now();
        // Auch während Übergängen: der vorab geöffnete Strom beginnt genau am Clipanfang
        if self.core.streaming() && streamable(c) {
            let (key, w, h) = stream_spec(id, c, m, canvas_px, div, stream_fps, fxh);
            // Quelle (Original/Proxy-Stücke) im Schlüssel: ein Wechsel öffnet einen Ersatzstrom
            let pick = self.stream_pick(lid(id), c, m, prefetch_at.unwrap_or(self.playhead));
            let key = key ^ pick.tag();
            // auch im Fingerabdruck: nach dem Teilen kein Spender-Strom über andere Stücke
            let alias = stream_alias(c, m, (w, h), stream_fps, fxh) ^ pick.tag();
            let want = Want {
                id: lid(id),
                key,
                start: prefetch_at.unwrap_or(self.playhead),
                end: c.end(),
                prefetch: prefetch_at.is_some(),
                alias,
            };
            // vorhanden laut Hintergrund-Prüfung (kein Dateizugriff je Ebene und UI-Bild: auf
            // einem schlafenden NAS blockierte der den UI-Thread)
            if self.presence.get(&m.path) {
                // Der Kern sagt, ob und ab wo: erster Strom, Ersatz nach einer Änderung (der alte
                // zeigt weiter, bis der neue liefert), Aufholen, hängender Strom
                if let Some(o) = self.core.want(&want, now) {
                    let (path, offset) = self.stream_source(m, h, pick);
                    let mut job = open_job(o, c, m, path, (w, h), stream_fps, alias);
                    job.source_at = Time((job.source_at - offset).0.max(0));
                    self.starter.send(job);
                }
            }
            if prefetch_at.is_some() {
                return;
            }
            let primary = id == l.clip.id;
            if let Some(f) = self.core.pull(lid(id), now, primary) {
                if primary && self.core.mode() == Mode::Spielen {
                    self.perf_frame_shown();
                }
                self.set_frame(ctx, id, key, &f);
            }
            return;
        }
        if prefetch_at.is_some() {
            return;
        }
        let (w, h) = decode_size(c, m, canvas_px, div);
        let src = still_src(c, m, l.t_rel, fps);
        let animated_fx = c.fx.effects.iter().any(|e| {
            matches!(
                e.id.as_str(),
                "shake"
                    | "zoom-pulse"
                    | "glitch"
                    | "rainbow"
                    | "strobe"
                    | "flash"
                    | "old-film"
                    | "warm-leak"
            )
        });
        let key = Self::frame_key(id, c, src, w, h, l.t_rel, animated_fx, fxh, fps);
        // fehlende Datei (Hintergrund-Prüfung): gar nicht erst anfordern, sonst bliebe der Auftrag offen
        if self.presence.get(&m.path) && self.core.still(lid(id), key, now) {
            // LUT-Datei erst beim Anfordern prüfen, nicht je UI-Bild
            let lut = c.fx.lut.as_deref().filter(|p| std::path::Path::new(p).is_file()).map(cl_media::escape_path);
            let graph = cl_fx::preview_graph(c, w, h, l.t_rel.as_secs_f64(), None, lut);
            let (path, at) = self.still_source(m, h, src);
            let window = reverse_window(c, m, path == m.path, graph.is_none(), self.core.playing() && self.core.rate() > 0.0, fps);
            self.decoder.send(Job::Frame {
                clip: id,
                key,
                path,
                at,
                w,
                h,
                graph,
                window,
            });
        }
    }

    pub fn player(&mut self, ui: &mut egui::Ui) {
        let t_player = std::time::Instant::now();
        let s = self.project.settings.clone();
        let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
        let controls_h = 34.0;
        let avail = ui.available_size() - vec2(0.0, controls_h);
        let fit_scale = ((avail.x - 16.0) / pw).min((avail.y - 16.0) / ph).max(0.01);
        let (area, area_resp) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());
        self.player_zoom_input(ui, &area_resp, area, fit_scale, pw, ph);
        let scale = fit_scale * self.player_zoom;
        let canvas_size = vec2(pw * scale, ph * scale);
        let canvas = Rect::from_center_size(area.center() + self.player_pan, canvas_size);
        let ppp = ui.ctx().pixels_per_point();
        let layers = self.layers_at(self.playhead);
        self.update_layers(ui.ctx(), &layers, canvas_size * ppp);
        self.shader_mix(ui.ctx(), &layers, canvas_size * ppp);

        let painter = ui.painter_at(area);
        painter.rect_filled(canvas, 0.0, Color32::BLACK);
        let content = painter.with_clip_rect(canvas.intersect(area));
        let regions = self.draw_layers(&content, canvas, scale, &layers);
        // Warten auf ein Bild: das letzte bleibt stehen, darunter eine dünne Lauflinie. Kommt nach
        // 8 s nichts (Datei nicht lesbar, entferntes Medium), endet die Linie.
        let wait_id = egui::Id::new("player-wait");
        let waiting = self.waiting_for_frame();
        let now = ui.input(|i| i.time);
        let since = waiting.then(|| ui.data(|d| d.get_temp::<f64>(wait_id)).unwrap_or(now));
        ui.data_mut(|d| match since {
            Some(s) => d.insert_temp(wait_id, s),
            None => d.remove::<f64>(wait_id),
        });
        if theme::loading(ui.ctx(), wait_id, since.is_some_and(|s| now - s < 8.0)) {
            let vis = canvas.intersect(area);
            crate::widgets::busy_line(&painter, Rect::from_min_max(pos2(vis.left(), vis.bottom() - 2.0), vis.max));
        }
        if crate::app::perf() {
            let us = t_player.elapsed().as_micros() as u64;
            PERF_PLAYER_SUM_US.fetch_add(us, std::sync::atomic::Ordering::Relaxed);
            PERF_PLAYER_MAX_US.fetch_max(us, std::sync::atomic::Ordering::Relaxed);
        }
        // Ablegen aus dem Asset-Bereich direkt im Player
        if let Some(payload) = area_resp.dnd_release_payload::<DragAsset>() {
            let on = area_resp
                .hover_pos()
                .and_then(|p| regions.iter().rev().find(|r| r.contains(p)).map(|r| r.clip));
            self.drop_asset(&payload, self.playhead, None, on);
        } else if area_resp.dnd_hover_payload::<DragAsset>().is_some() {
            painter.rect_stroke(
                canvas,
                0.0,
                egui::Stroke::new(2.0_f32, theme::ACCENT()),
                egui::StrokeKind::Inside,
            );
        }
        if area_resp.clicked() || area_resp.drag_started() {
            crate::app::release_text_focus(ui.ctx());
        }
        if !self.player_pick(ui, &area_resp, &regions) && !self.censor_gizmo(ui, &area_resp, &regions) {
            self.player_interact(ui, &area_resp, canvas, scale, &regions);
        }
        // Plattform-Bedienelemente (oben ~10 %, unten ~20 %, rechts ~12 %): nur Vorschau
        if self.safe_area && ph > pw {
            let (w, h) = (canvas.width(), canvas.height());
            let zones = [
                Rect::from_min_size(canvas.min, vec2(w, h * 0.10)),
                Rect::from_min_max(canvas.min + vec2(0.0, h * 0.80), canvas.max),
                Rect::from_min_max(canvas.min + vec2(w * 0.88, h * 0.10), canvas.max - vec2(0.0, h * 0.20)),
            ];
            let line = egui::Stroke::new(1.0_f32, Color32::from_white_alpha(70));
            for z in zones {
                content.rect_filled(z, 0.0, Color32::from_black_alpha(70));
                // Schraffur
                let hatch = content.with_clip_rect(z.intersect(canvas.intersect(area)));
                let mut x = z.left() - z.height();
                while x < z.right() {
                    hatch.line_segment([pos2(x, z.bottom()), pos2(x + z.height(), z.top())], egui::Stroke::new(1.0_f32, Color32::from_white_alpha(18)));
                    x += 12.0;
                }
                content.rect_stroke(z, 0.0, line, egui::StrokeKind::Inside);
            }
        }
        area_resp.context_menu(|ui| self.player_menu(ui));
        if self.voiceover.recorder.is_some() {
            painter.circle_filled(canvas.left_top() + vec2(14.0, 14.0), 5.0, theme::DANGER);
            painter.text(
                canvas.left_top() + vec2(24.0, 14.0),
                egui::Align2::LEFT_CENTER,
                crate::tr("REC"),
                egui::FontId::proportional(theme::FS_UI),
                Color32::WHITE,
            );
        }
        if self.project.duration() == Time::ZERO {
            painter.text(
                canvas.center() - vec2(0.0, 10.0),
                egui::Align2::CENTER_CENTER,
                icon::FILM_SLATE,
                egui::FontId::proportional(26.0), // Icon-Glyphe im leeren Zustand
                theme::TEXT_FAINT(),
            );
            painter.text(
                canvas.center() + vec2(0.0, 18.0),
                egui::Align2::CENTER_CENTER,
                crate::tr(self.t("drop-hint")),
                egui::FontId::proportional(theme::FS_TEXT),
                theme::TEXT_DIM(),
            );
        }
        self.player_controls(ui, controls_h);
    }

    /// Alle Ebenen in `canvas` zeichnen (Player und externes Vorschaufenster).
    fn draw_layers(
        &self,
        content: &egui::Painter,
        canvas: Rect,
        scale: f32,
        layers: &[Layer],
    ) -> Vec<Region> {
        let s = &self.project.settings;
        let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
        let mut regions: Vec<Region> = Vec::new();
        // Unterstes gezeigtes Bild: steht weiter, solange der nächste Clip noch kein Bild hat
        // (nur im selben Projekt: ein anderes zeigt nie das Bild des vorigen)
        let held_id = egui::Id::new("player-held-frame");
        let held_for = egui::Id::new(&self.project_path).value();
        let mut bottom = true;
        for l in layers {
            let c = &l.clip;
            let mut ev = c.eval_at(l.t_rel, pw, ph);
            if let Some((id, lt)) = self.gizmo.live {
                if id == c.id {
                    ev.x = lt.x;
                    ev.y = lt.y;
                    ev.scale = lt.scale;
                    ev.rotation_deg = lt.rotation_deg;
                }
            }
            if let Some((_, k)) = self.gizmo.live_crop.filter(|l| l.0 == c.id) {
                ev.crop = k;
            }
            let tr = Transform {
                x: ev.x,
                y: ev.y,
                scale: ev.scale,
                rotation_deg: ev.rotation_deg,
            };
            let alpha = (ev.opacity * l.alpha).clamp(0.0, 1.0);
            let center = canvas.center() + vec2(ev.x, ev.y) * scale;
            let angle = ev.rotation_deg.to_radians();
            match &c.content {
                ClipContent::Media { .. } => {
                    // Shader-Übergang: das gemischte Bild (fertige Ebenen auf ganzer Fläche) statt A,
                    // B entfällt
                    let mixed = match (self.shader_preview.current, &self.shader_preview.tex) {
                        (Some((_, b)), _) if b == c.id => continue,
                        (Some((a, _)), Some(t)) if a == c.id => Some(t),
                        _ => None,
                    };
                    if let Some(t) = mixed {
                        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                        content.image(t.id(), canvas, uv, Color32::WHITE);
                        // Effekt-Elemente darüber: ihre gemischte Fassung (wie der Export auf dem
                        // fertigen Übergang)
                        let corners = [canvas.left_top(), canvas.right_top(), canvas.right_bottom(), canvas.left_bottom()];
                        for (fx_tex, mk, k) in &self.shader_preview.fx {
                            draw_fx(content, t.id(), fx_tex.id(), corners, corners, mk, *k, canvas);
                        }
                    }
                    let Some(tex) = self.frames.get(&c.id).map(|f| &f.1) else {
                        if std::mem::take(&mut bottom) && mixed.is_none() {
                            let held = content.ctx().data(|d| d.get_temp::<(u64, egui::TextureHandle)>(held_id));
                            if let Some((_, h)) = held.filter(|h| h.0 == held_for) {
                                let hs = h.size_vec2();
                                let fit = (canvas.width() / hs.x).min(canvas.height() / hs.y);
                                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                                content.image(h.id(), Rect::from_center_size(canvas.center(), hs * fit), uv, Color32::WHITE);
                            }
                        }
                        continue;
                    };
                    if std::mem::take(&mut bottom) {
                        content.ctx().data_mut(|d| d.insert_temp(held_id, (held_for, tex.clone())));
                    }
                    let ts = tex.size_vec2();
                    let fit = (canvas.width() / ts.x).min(canvas.height() / ts.y) * ev.scale;
                    let region = Region {
                        clip: c.id,
                        center,
                        size: ts * fit,
                        angle,
                        tr,
                        crop: ev.crop,
                    };
                    if mixed.is_some() {
                        regions.push(region);
                        continue;
                    }
                    // Zuschnitt als Ausschnitt der Textur; der Rest bleibt an seiner Stelle
                    let k = ev.crop;
                    let uvs = [(k.left, k.top), (1.0 - k.right, k.top), (1.0 - k.right, 1.0 - k.bottom), (k.left, 1.0 - k.bottom)];
                    let shown = uvs.map(|(u, v)| region.corner(2.0 * u - 1.0, 2.0 * v - 1.0));
                    let mut mesh = egui::Mesh::with_texture(tex.id());
                    let col = Color32::from_white_alpha((alpha * 255.0) as u8);
                    for (pos, (u, v)) in shown.into_iter().zip(uvs) {
                        mesh.vertices.push(egui::epaint::Vertex { pos, uv: pos2(u, v), color: col });
                    }
                    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                    content.add(egui::Shape::mesh(mesh));
                    // Maskierte Effekt-/Filter-Elemente darüber: zweite Fassung nur in der Maske
                    let corners = [
                        region.corner(-1.0, -1.0),
                        region.corner(1.0, -1.0),
                        region.corner(1.0, 1.0),
                        region.corner(-1.0, 1.0),
                    ];
                    for (base, did, mk, k) in &self.fx_overlays {
                        if *base != c.id || *k <= 0.001 {
                            continue;
                        }
                        let Some((_, fx_tex)) = self.frames.get(did) else {
                            continue;
                        };
                        draw_fx(content, tex.id(), fx_tex.id(), corners, shown, mk, alpha * k, canvas);
                    }
                    regions.push(region);
                }
                ClipContent::Effect(_) | ClipContent::Filter(_) => {
                    // Maske des ausgewählten Elements als Umriss zeigen
                    if self.selected == Some(c.id) {
                        if let Some(mk) = &c.fx.mask {
                            let (polys, _) = crate::fx_mask::polygons(mk, canvas);
                            for poly in polys {
                                content.add(egui::Shape::closed_line(
                                    poly,
                                    egui::Stroke::new(1.5_f32, theme::ACCENT()),
                                ));
                            }
                        }
                    }
                }
                ClipContent::Text(st) => {
                    let size = self.draw_text(
                        content,
                        st,
                        center,
                        ev.scale * scale,
                        angle,
                        alpha,
                        canvas.width(),
                        st.karaoke_lit(c.duration, l.t_rel),
                    );
                    regions.push(Region {
                        clip: c.id,
                        center,
                        size,
                        angle,
                        tr,
                        crop: Crop::default(),
                    });
                }
            }
        }
        regions
    }

    /// Vorschau in eigenem Fenster (z. B. auf dem zweiten Bildschirm, F11 = Vollbild).
    pub fn external_preview(&mut self, ctx: &egui::Context) {
        let init = egui::Id::new("ext-preview-init");
        if !self.ext_preview {
            ctx.data_mut(|d| d.remove::<u8>(init));
            return;
        }
        let vid = egui::ViewportId::from_hash_of("cutline-preview");
        // In den ersten Bildern nach dem Öffnen ausdrücklich nach vorn holen: eine Angabe im
        // Builder wirkt unter Windows nicht, das Fenster läge hinter dem Hauptfenster
        let n = ctx.data(|d| d.get_temp::<u8>(init)).unwrap_or(0);
        if n < 4 {
            ctx.data_mut(|d| d.insert_temp(init, n + 1));
            ctx.send_viewport_cmd_to(vid, egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd_to(vid, egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(crate::tr("Cutline – Vorschau"))
            .with_inner_size([960.0, 540.0])
            .with_min_inner_size([320.0, 180.0]);
        // Erst auf dem Bildschirm des Hauptfensters öffnen; von dort auf den zweiten ziehen
        // nur beim Öffnen: eine dauerhaft gesetzte Position hält das Fenster fest
        if let Some(r) = ctx.input(|i| i.viewport().outer_rect).filter(|_| n < 4) {
            builder = builder.with_position(r.min + vec2(80.0, 80.0));
        }
        let mut ext_full = self.ext_full;
        let mut toggle_full = false;
        ctx.show_viewport_immediate(vid, builder, |ctx, _| {
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(Color32::BLACK))
                .show(ctx, |ui| {
                    let s = self.project.settings.clone();
                    let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
                    let area = ui.max_rect();
                    let scale = (area.width() / pw).min(area.height() / ph);
                    let canvas =
                        Rect::from_center_size(area.center(), vec2(pw * scale, ph * scale));
                    let layers = self.layers_at(self.playhead);
                    let painter = ui.painter_at(area);
                    self.draw_layers(&painter.with_clip_rect(canvas), canvas, scale, &layers);
                    let r = ui.interact(area, ui.id().with("ext"), egui::Sense::click());
                    if r.double_clicked() {
                        toggle_full = true;
                    }
                });
            if ctx.input(|i| i.key_pressed(egui::Key::F11) || (ext_full && i.key_pressed(egui::Key::Escape))) {
                toggle_full = true;
            }
            // Vollbild als randloses, maximiertes Fenster: echtes Vollbild einer zweiten
            // Ansicht lässt egui/winit unter Windows hängen.
            if toggle_full {
                let full = !ext_full;
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!full));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(full));
                ext_full = full;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                self.toggle_play();
            }
            if ctx.input(|i| i.viewport().close_requested()) {
                self.ext_preview = false;
                ext_full = false;
            }
        });
        self.ext_full = ext_full;
        if self.playing {
            ctx.request_repaint_of(vid);
        }
    }

    /// Text wie im Export: Fläche, Kontur, Schatten, Ausrichtung, Zeilenabstand; `lit`: Wörter
    /// in der Karaoke-Farbe. Gibt die Größe des Textblocks (Bildschirm) zurück.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &self,
        p: &egui::Painter,
        st: &TextStyle,
        center: egui::Pos2,
        k: f32,
        angle: f32,
        alpha: f32,
        canvas_w: f32,
        lit: Option<std::ops::Range<usize>>,
    ) -> egui::Vec2 {
        let txt = if st.uppercase {
            st.text.to_uppercase()
        } else {
            st.text.clone()
        };
        let px = (st.size * k).max(2.0);
        let font = egui::FontId::new(px, self.font_family(st.font_family.as_deref()));
        let wrap = if st.auto_wrap {
            canvas_w * 0.9
        } else {
            f32::INFINITY
        };
        let a = |c: [u8; 4]| {
            Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (c[3] as f32 * alpha) as u8)
        };
        // Karaoke: Byte-Bereich der hervorgehobenen Wörter und ihre Farbe
        let hi = st.karaoke.as_ref().zip(lit).and_then(|(kar, lit)| {
            let w = cl_core::word_ranges(&txt);
            Some((w.get(lit.start)?.start..w.get(lit.end - 1)?.end, a(kar.color)))
        });
        let job = |col: Color32, hi: Option<(std::ops::Range<usize>, Color32)>| {
            let mut j = egui::text::LayoutJob::default();
            let fmt = |color| egui::TextFormat {
                font_id: font.clone(),
                color,
                line_height: Some(px * 1.25 * st.line_spacing.max(0.5)),
                italics: st.italic,
                ..Default::default()
            };
            match hi {
                Some((r, hc)) => {
                    j.append(&txt[..r.start], 0.0, fmt(col));
                    j.append(&txt[r.clone()], 0.0, fmt(hc));
                    j.append(&txt[r.end..], 0.0, fmt(col));
                }
                None => j.append(&txt, 0.0, fmt(col)),
            }
            j.wrap.max_width = wrap;
            j.halign = match st.align {
                x if x < 0 => egui::Align::LEFT,
                x if x > 0 => egui::Align::RIGHT,
                _ => egui::Align::Center,
            };
            j
        };
        let galley = p.layout_job(job(a(st.color), hi.clone()));
        let size = galley.size();
        let rot = |v: egui::Vec2| {
            let (s, c) = angle.sin_cos();
            vec2(v.x * c - v.y * s, v.x * s + v.y * c)
        };
        // Ankerpunkt für TextShape: links oben der Galley (bei Center-Ausrichtung liegt
        // die Galley um x = 0 herum)
        let x_off = match st.align {
            x if x < 0 => -size.x / 2.0,
            x if x > 0 => size.x / 2.0,
            _ => 0.0,
        };
        let anchor = center + rot(vec2(x_off, -size.y / 2.0));
        if let Some(bg) = st.background {
            let pad = (px * 0.25).max(3.0);
            let pts: Vec<egui::Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .iter()
                .map(|(sx, sy)| {
                    center
                        + rot(vec2(
                            sx * (size.x / 2.0 + pad),
                            sy * (size.y / 2.0 + pad * 0.6),
                        ))
                })
                .collect();
            p.add(egui::Shape::convex_polygon(pts, a(bg), egui::Stroke::NONE));
        }
        if st.shadow {
            let sg = p.layout_job(job(Color32::from_black_alpha((150.0 * alpha) as u8), None));
            let off = rot(vec2(1.0, 1.0) * (px / 22.0).clamp(1.0, 4.0));
            p.add(egui::epaint::TextShape::new(anchor + off, sg, Color32::BLACK).with_angle(angle));
        }
        if let Some((w, col)) = st.outline {
            let og = p.layout_job(job(a(col), None));
            let r = (w * k).clamp(0.6, 12.0);
            for i in 0..12 {
                let ang = i as f32 / 12.0 * std::f32::consts::TAU;
                p.add(
                    egui::epaint::TextShape::new(
                        anchor + rot(vec2(ang.cos(), ang.sin()) * r),
                        og.clone(),
                        Color32::BLACK,
                    )
                    .with_angle(angle),
                );
            }
        }
        p.add(egui::epaint::TextShape::new(anchor, galley, a(st.color)).with_angle(angle));
        if st.bold {
            // Fettdruck nachahmen, wenn die Schrift keinen eigenen Schnitt hat
            let g2 = p.layout_job(job(a(st.color), hi));
            p.add(
                egui::epaint::TextShape::new(anchor + rot(vec2(px * 0.03, 0.0)), g2, a(st.color))
                    .with_angle(angle),
            );
        }
        size
    }

    /// Steuerleiste: Zeit links, Play mittig, Chips rechts.
    /// Pegelanzeige (Spitze mit Abfall und Haltelinie), rot ab −1 dBFS
    fn level_meter(&mut self, ui: &mut egui::Ui) {
        let (l, r) = self
            .audio
            .as_ref()
            .map(|a| a.take_peaks())
            .unwrap_or((0.0, 0.0));
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        for (i, v) in [l, r].into_iter().enumerate() {
            let cur = &mut self.meter[i];
            cur.0 = v.max(cur.0 - dt * 1.2);
            if v >= cur.1 {
                cur.1 = v;
                cur.2 = 1.0;
            } else {
                cur.2 -= dt;
                if cur.2 <= 0.0 {
                    cur.1 = (cur.1 - dt * 0.6).max(cur.0);
                }
            }
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 22.0), egui::Sense::hover());
        let db = |v: f32| if v <= 1e-5 { -60.0 } else { 20.0 * v.log10() };
        let y_of = |v: f32| rect.bottom() - rect.height() * ((db(v) + 60.0) / 60.0).clamp(0.0, 1.0);
        let p = ui.painter();
        for i in 0..2 {
            let x0 = rect.left() + i as f32 * 7.0;
            let bar = Rect::from_min_max(pos2(x0, rect.top()), pos2(x0 + 5.0, rect.bottom()));
            p.rect_filled(bar, 1.0, theme::BG_HOVER());
            let (v, hold, _) = self.meter[i];
            let col = if db(v) > -1.0 {
                theme::DANGER
            } else if db(v) > -9.0 {
                theme::WARN
            } else {
                theme::SUCCESS
            };
            p.rect_filled(
                Rect::from_min_max(pos2(bar.left(), y_of(v)), bar.max),
                1.0,
                col,
            );
            if hold > 1e-4 {
                let y = y_of(hold);
                p.line_segment(
                    [pos2(bar.left(), y), pos2(bar.right(), y)],
                    egui::Stroke::new(
                        1.0_f32,
                        if db(hold) > -1.0 {
                            theme::DANGER
                        } else {
                            theme::TEXT_DIM()
                        },
                    ),
                );
            }
        }
        resp.on_hover_text(crate::tr(format!(
            "Pegel L {:.0} dB · R {:.0} dB",
            db(self.meter[0].1),
            db(self.meter[1].1)
        )));
        if self.playing {
            ui.ctx().request_repaint_after(self.play_repaint_delay());
        }
    }

    /// Lautstärke der Wiedergabe setzen; `save` erst, wenn die Änderung abgeschlossen ist
    pub fn set_volume(&mut self, v: crate::audio::Volume, save: bool) {
        self.volume = v;
        crate::audio::set_volume(v);
        if save {
            self.save_last_used();
        }
    }

    pub fn step_volume(&mut self, up: bool) {
        self.set_volume(self.volume.step(up), true);
    }

    /// Lautsprecher neben der Pegelanzeige: Klick = stumm/an, Mausrad = 5-%-Schritte, beim
    /// Überfahren ein Regler darüber (eigene Ebene, nimmt der Leiste keinen Platz).
    fn volume_button(&mut self, ui: &mut egui::Ui) {
        let v = self.volume;
        let glyph = if v.muted || v.percent <= 0.0 {
            icon::SPEAKER_SLASH
        } else if v.percent < 50.0 {
            icon::SPEAKER_LOW
        } else {
            icon::SPEAKER_HIGH
        };
        let tip = if v.muted {
            "Lautstärke der Wiedergabe: stumm".to_string()
        } else {
            format!("Lautstärke der Wiedergabe: {:.0} %", v.percent)
        };
        let r = theme::icon_button(ui, glyph, crate::keymap::with_key(&tip, &self.key_of("playback_mute")), true, false);
        if r.clicked() {
            self.set_volume(crate::audio::Volume { muted: !v.muted, ..v }, true);
        }
        let pop = r.id.with("volume");
        if r.hovered() {
            // shortcut: je Rad-Ereignis ein Schritt (Touchpads liefern viele kleine), reicht für Mausräder
            let steps: Vec<bool> = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => Some(delta.y > 0.0),
                        _ => None,
                    })
                    .collect()
            });
            for up in steps {
                self.step_volume(up);
            }
        }
        // offen, solange die Maus auf dem Symbol oder dem Regler ist (oder ihn zieht)
        let last: Option<Rect> = ui.data(|d| d.get_temp(pop));
        let keep = last.is_some_and(|rect| {
            let near = rect.expand(8.0);
            ui.input(|i| {
                i.pointer.hover_pos().is_some_and(|p| near.contains(p))
                    || (i.pointer.primary_down() && i.pointer.press_origin().is_some_and(|p| near.contains(p)))
            })
        });
        if !(r.hovered() || keep) {
            ui.data_mut(|d| d.remove::<Rect>(pop));
            return;
        }
        let area = egui::Area::new(pop)
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(r.rect.left_top() - vec2(0.0, 2.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(150.0);
                    let mut p = self.volume.percent;
                    let e = crate::widgets::Slider::new(&mut p, 0.0..=100.0).suffix("%").default(100.0).show(ui);
                    if e.changed {
                        self.set_volume(crate::audio::Volume { percent: p, muted: false }, false);
                    }
                    if e.commit {
                        self.save_last_used();
                    }
                });
            });
        ui.data_mut(|d| d.insert_temp(pop, area.response.rect));
    }

    fn player_controls(&mut self, ui: &mut egui::Ui, h: f32) {
        let fps = self.project.settings.fps;
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), egui::Sense::hover());
        let left = Rect::from_min_max(bar.min, pos2(bar.center().x - 24.0, bar.max.y));
        let right = Rect::from_min_max(pos2(bar.center().x + 24.0, bar.min.y), bar.max);
        ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(left)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.add_space(4.0);
                self.level_meter(ui);
                self.volume_button(ui);
                ui.label(
                    RichText::new(crate::tr(self.playhead.timecode(fps)))
                        .color(theme::TEXT())
                        .monospace(),
                );
                let total = format!("/ {}", self.project.duration().timecode(fps));
                let need = ui
                    .painter()
                    .layout_no_wrap(
                        total.clone(),
                        egui::TextStyle::Monospace.resolve(ui.style()),
                        theme::TEXT_DIM(),
                    )
                    .size()
                    .x
                    + 8.0;
                if ui.available_width() > need {
                    ui.label(
                        RichText::new(crate::tr(total))
                            .color(theme::TEXT_DIM())
                            .monospace(),
                    );
                }
            },
        );
        let pr = ui.put(
            Rect::from_center_size(bar.center(), vec2(36.0, 28.0)),
            egui::Button::new("").frame(false),
        );
        // Symbol wechselt weich: Play blendet aus, Pause ein (und umgekehrt)
        let k = theme::fade(ui.ctx(), pr.id.with("glyph"), self.playing, theme::MOTION_FAST);
        for (glyph, a) in [(icon::PLAY, 1.0 - k), (icon::PAUSE, k)] {
            if a > 0.0 {
                // Icon-Glyphe: großer Abspielknopf
                ui.painter().text(pr.rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(19.0), theme::TEXT().gamma_multiply(a));
            }
        }
        if pr
            .on_hover_text(crate::tr(format!(
                "{}  (Leertaste)",
                if self.playing {
                    self.t("pause")
                } else {
                    self.t("play")
                }
            )))
            .clicked()
        {
            self.toggle_play();
        }
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            let compact = ui.available_width() < 200.0;
            let (g, tip) = if self.player_max { (icon::CORNERS_IN, "Normale Ansicht (F)") } else { (icon::CORNERS_OUT, "Großer Player (F)") };
            if theme::icon_button(ui, g, tip, true, self.player_max).clicked() {
                self.player_max = !self.player_max;
            }
            if theme::icon_button(ui, icon::MONITOR_PLAY, "Vorschau in eigenem Fenster (zweiter Bildschirm, Doppelklick oder F11 = Vollbild)", true, self.ext_preview).clicked() {
                self.ext_preview = !self.ext_preview;
            }
            self.player_zoom_chip(ui, compact);
            // Chip kurz (½, ¼), Tooltip mit denselben Wörtern wie überall: Voll, Halb, Viertel
            let (q, word) = match self.settings.preview_div {
                1 => ("Voll", "Voll"),
                2 => ("½", "Halb"),
                _ => ("¼", "Viertel"),
            };
            let qlabel = if compact { icon::MONITOR.to_string() } else { format!("{} {}", icon::MONITOR, crate::tr(q)) };
            if theme::chip(ui, &qlabel).on_hover_text(crate::tr(format!("Vorschauqualität: {}", crate::tr(word)))).clicked() {
                self.settings.preview_div = match self.settings.preview_div {
                    1 => 2,
                    2 => 4,
                    _ => 1,
                };
                self.invalidate_preview();
            }
            // automatische Qualität beim Abspielen: kleine Anzeige neben dem Chip, blendet ein und aus
            let auto_id = egui::Id::new("player-auto-quality");
            let k = theme::fade(ui.ctx(), auto_id, self.core.playing() && self.core.quality.level > 0, theme::MOTION);
            if self.core.quality.level > 0 {
                let (div, half) = self.play_quality();
                let fps = self.project.settings.fps.as_f64();
                let txt = format!(
                    "{}{}",
                    match div {
                        2 => "½",
                        4 => "¼",
                        _ => "",
                    },
                    if half { format!(" {:.0} fps", fps / 2.0) } else { String::new() }
                );
                ui.data_mut(|d| d.insert_temp(auto_id, txt));
            }
            if k > 0.01 {
                let txt = ui.data(|d| d.get_temp::<String>(auto_id)).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(txt.trim()).size(theme::FS_HINT).color(theme::ACCENT().gamma_multiply(k))))
                    .on_hover_text(crate::tr("Vorschauqualität beim Abspielen automatisch verringert"));
            }
            let aspect = AspectPreset::PRESETS.iter().find(|(_, p)| *p == self.project.settings.aspect).map(|(n, _)| *n).unwrap_or("Eigenes");
            let alabel = if compact { icon::FRAME_CORNERS.to_string() } else { format!("{} {aspect}", icon::FRAME_CORNERS) };
            let resp = theme::chip(ui, &alabel).on_hover_text(crate::tr(format!("{}: {aspect}", self.t("aspect"))));
            let popup = ui.make_persistent_id("aspect-popup");
            if resp.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup));
            }
            egui::popup_above_or_below_widget(ui, popup, &resp, egui::AboveOrBelow::Above, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                ui.set_min_width(120.0);
                for (n, p) in AspectPreset::PRESETS {
                    if ui.selectable_label(self.project.settings.aspect == p, crate::tr(n)).clicked() {
                        self.set_aspect(p);
                    }
                }
            });
        });
    }

    /// Rechtsklick in den Player: dieselben Befehle wie in der Player-Leiste und den Schnellaktionen.
    fn player_menu(&mut self, ui: &mut egui::Ui) {
        use crate::timeline::menu_item;
        if menu_item(ui, icon::IMAGE, "Standbild speichern …", "") {
            let ctx = ui.ctx().clone();
            self.tool_thumbnail(&ctx);
        }
        ui.separator();
        if menu_item(ui, icon::MAGNIFYING_GLASS, "Ganzes Bild", "Alt+Umschalt+Z") {
            self.player_zoom = 1.0;
            self.player_pan = egui::Vec2::ZERO;
        }
        let ext = if self.ext_preview { icon::CHECK } else { icon::MONITOR_PLAY };
        if menu_item(ui, ext, "Vorschau in eigenem Fenster", "") {
            self.ext_preview = !self.ext_preview;
        }
        let max = if self.player_max { icon::CHECK } else { icon::CORNERS_OUT };
        if menu_item(ui, max, "Großer Player", &self.key_of("max")) {
            self.player_max = !self.player_max;
        }
        let sa = if self.safe_area { icon::CHECK } else { icon::DEVICE_MOBILE };
        if menu_item(ui, sa, "Safe-Area", "") {
            self.safe_area = !self.safe_area;
            self.save_last_used();
        }
    }

    pub fn set_aspect(&mut self, p: AspectPreset) {
        let mut s = self.project.settings.clone();
        if p == AspectPreset::Original {
            if let Some(m) = self.project.media.iter().find(|m| m.width > 0) {
                s.width = m.width;
                s.height = m.height;
            }
            s.aspect = p;
        } else {
            s.apply_aspect(p);
        }
        if s.platform
            .as_deref()
            .and_then(crate::export_ui::platform)
            .is_some_and(|pl| !pl.fits(&s))
        {
            s.platform = None;
        }
        if s != self.project.settings {
            // Format ändert sich und es gibt Bilder: erst fragen, wie sie neu ins Bild sollen
            if (s.width, s.height) != (self.project.settings.width, self.project.settings.height) && self.has_visual_clips() {
                self.aspect_fit = Some(s);
            } else {
                self.apply(EditOp::SetSettings { settings: s });
            }
        }
    }
}
