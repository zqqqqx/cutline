//! Spracherkennung: Parakeet TDT 0.6B v3 (bevorzugt) mit Whisper als Rückfall.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ort::value::Tensor;

use crate::{decode_audio, esc, session, Models};

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Eine Untertitelzeile.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Parakeet,
    Whisper,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Parakeet => "Parakeet TDT 0.6B v3",
            Engine::Whisper => "Whisper",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Sprache für Whisper (`de`, `en`, `auto`); Parakeet erkennt sie selbst
    pub language: String,
    /// Höchstens so viele Zeichen je Zeile
    pub max_chars: usize,
    /// Höchstdauer je Zeile (s)
    pub max_dur: f64,
    /// Engine erzwingen (sonst die beste verfügbare)
    pub force: Option<Engine>,
}

impl Default for Options {
    fn default() -> Self {
        Self { language: "auto".into(), max_chars: 42, max_dur: 5.0, force: None }
    }
}

/// Ton einer Datei transkribieren (Bereich `start`..`start+dur` in Quellzeit).
/// Zeiten im Ergebnis sind relativ zu `start`.
pub fn transcribe(models: &Models, path: &Path, start: f64, dur: f64, opt: &Options, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(Engine, Vec<Segment>), String> {
    let use_parakeet = match opt.force {
        Some(Engine::Whisper) => false,
        Some(Engine::Parakeet) => true,
        None => models.parakeet().is_some() && models.ort_dll().is_some(),
    };
    if use_parakeet {
        match parakeet(models, path, start, dur, progress, cancel) {
            Ok(words) => return Ok((Engine::Parakeet, group_words(&words, opt.max_chars, opt.max_dur))),
            Err(e) if opt.force.is_some() || models.whisper().is_none() => return Err(e),
            Err(e) => eprintln!("Parakeet fehlgeschlagen, nehme Whisper: {e}"),
        }
    }
    let model = models.whisper().ok_or("Kein Spracherkennungsmodell installiert (Einstellungen → Modelle)")?;
    whisper(&model, path, start, dur, &opt.language, progress).map(|s| (Engine::Whisper, s))
}

// ---------------------------------------------------------------- Parakeet

const SR: u32 = 16_000;
/// Sekunden je Encoder-Schritt (10 ms Hop × 8 Unterabtastung)
const FRAME: f64 = 0.08;
const CHUNK_S: f64 = 20.0;

struct Parakeet {
    pre: ort::session::Session,
    enc: ort::session::Session,
    dec: ort::session::Session,
    vocab: Vec<String>,
    blank: usize,
}

impl Parakeet {
    fn load(models: &Models) -> Result<Self, String> {
        let d = models.parakeet().ok_or("Parakeet-Modell fehlt")?;
        let vocab_txt = std::fs::read_to_string(d.join("vocab.txt")).map_err(|e| e.to_string())?;
        let mut vocab: Vec<(usize, String)> = vocab_txt
            .lines()
            .filter_map(|l| {
                let (tok, id) = l.rsplit_once(' ')?;
                Some((id.trim().parse().ok()?, tok.to_string()))
            })
            .collect();
        vocab.sort_by_key(|v| v.0);
        let vocab: Vec<String> = vocab.into_iter().map(|v| v.1).collect();
        let blank = vocab.iter().position(|t| t == "<blk>").unwrap_or(vocab.len() - 1);
        Ok(Self { pre: session(models, &d.join("nemo128.onnx"))?, enc: session(models, &d.join("encoder-model.int8.onnx"))?, dec: session(models, &d.join("decoder_joint-model.int8.onnx"))?, vocab, blank })
    }

    /// Encoder-Ausgabe als Zeilen je Zeitschritt (T × D).
    fn encode(&mut self, wave: &[f32]) -> Result<(Vec<Vec<f32>>, usize), String> {
        let e = |e: ort::Error| e.to_string();
        let w = Tensor::from_array(([1usize, wave.len()], wave.to_vec())).map_err(e)?;
        let l = Tensor::from_array(([1usize], vec![wave.len() as i64])).map_err(e)?;
        let out = self.pre.run(ort::inputs!["waveforms" => w, "waveforms_lens" => l]).map_err(e)?;
        let (fs, feats) = out["features"].try_extract_tensor::<f32>().map_err(e)?;
        let (_, flen) = out["features_lens"].try_extract_tensor::<i64>().map_err(e)?;
        let shape: Vec<usize> = fs.iter().map(|d| *d as usize).collect();
        let feats = Tensor::from_array((shape, feats.to_vec())).map_err(e)?;
        let flen = Tensor::from_array(([1usize], vec![flen[0]])).map_err(e)?;
        drop(out);
        let out = self.enc.run(ort::inputs!["audio_signal" => feats, "length" => flen]).map_err(e)?;
        let (s, data) = out["outputs"].try_extract_tensor::<f32>().map_err(e)?;
        let (_, elen) = out["encoded_lengths"].try_extract_tensor::<i64>().map_err(e)?;
        let (dim, t) = (s[1] as usize, s[2] as usize);
        let n = (elen[0] as usize).min(t);
        // [1, D, T] → T Zeilen à D
        let rows = (0..n).map(|ti| (0..dim).map(|di| data[di * t + ti]).collect()).collect();
        Ok((rows, dim))
    }

    /// TDT-Greedy-Dekodierung: (Token-ID, Zeitschritt).
    fn decode(&mut self, frames: &[Vec<f32>], dim: usize) -> Result<Vec<(usize, usize)>, String> {
        let e = |e: ort::Error| e.to_string();
        let mut s1 = vec![0f32; 2 * 640];
        let mut s2 = vec![0f32; 2 * 640];
        let mut last = self.blank as i32;
        let mut out = Vec::new();
        let (mut t, mut emitted) = (0usize, 0usize);
        let vocab_n = self.vocab.len();
        while t < frames.len() {
            let enc = Tensor::from_array(([1usize, dim, 1], frames[t].clone())).map_err(e)?;
            let tg = Tensor::from_array(([1usize, 1], vec![last])).map_err(e)?;
            let tl = Tensor::from_array(([1usize], vec![1i32])).map_err(e)?;
            let i1 = Tensor::from_array(([2usize, 1, 640], s1.clone())).map_err(e)?;
            let i2 = Tensor::from_array(([2usize, 1, 640], s2.clone())).map_err(e)?;
            let res = self.dec.run(ort::inputs!["encoder_outputs" => enc, "targets" => tg, "target_length" => tl, "input_states_1" => i1, "input_states_2" => i2]).map_err(e)?;
            let (_, logits) = res["outputs"].try_extract_tensor::<f32>().map_err(e)?;
            let argmax = |s: &[f32]| s.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|x| x.0).unwrap_or(0);
            let tok = argmax(&logits[..vocab_n]);
            let step = argmax(&logits[vocab_n..]);
            if tok != self.blank {
                let (_, n1) = res["output_states_1"].try_extract_tensor::<f32>().map_err(e)?;
                let (_, n2) = res["output_states_2"].try_extract_tensor::<f32>().map_err(e)?;
                s1 = n1.to_vec();
                s2 = n2.to_vec();
                last = tok as i32;
                out.push((tok, t));
                emitted += 1;
            }
            if step > 0 {
                t += step;
                emitted = 0;
            } else if tok == self.blank || emitted >= 10 {
                t += 1;
                emitted = 0;
            }
        }
        Ok(out)
    }

    fn words(&self, toks: &[(usize, usize)], offset: f64) -> Vec<Word> {
        let mut words: Vec<Word> = Vec::new();
        for (tok, t) in toks {
            let piece = &self.vocab[*tok];
            if piece.starts_with('<') && piece.ends_with('>') {
                continue;
            }
            let time = offset + *t as f64 * FRAME;
            let new_word = piece.starts_with('\u{2581}') || words.is_empty();
            let clean = piece.replace('\u{2581}', "");
            // Satzzeichen hängen am vorigen Wort
            let punct = !clean.is_empty() && clean.chars().all(|c| c.is_ascii_punctuation() || "„“«»…".contains(c));
            if new_word && !(punct && !words.is_empty()) {
                if let Some(w) = words.last_mut() {
                    w.end = w.end.max(time);
                }
                words.push(Word { start: time, end: time + FRAME, text: clean });
            } else if let Some(w) = words.last_mut() {
                w.text.push_str(&clean);
                w.end = time + FRAME;
            }
        }
        words.retain(|w| !w.text.trim().is_empty());
        words
    }
}

/// Grenzen für Abschnitte: leise Stellen nahe an jeweils `CHUNK_S` Sekunden.
fn chunk_bounds(wave: &[f32]) -> Vec<(usize, usize)> {
    let total = wave.len();
    let target = (CHUNK_S * SR as f64) as usize;
    let mut out = Vec::new();
    let mut a = 0;
    while a < total {
        if total - a <= target + SR as usize * 4 {
            out.push((a, total));
            break;
        }
        // leisestes 50-ms-Fenster in den letzten 4 s vor dem Ziel
        let win = SR as usize / 20;
        let search = (a + target - SR as usize * 4)..(a + target);
        let cut = search
            .step_by(win)
            .min_by(|x, y| {
                let e = |s: usize| wave[s..(s + win).min(total)].iter().map(|v| v * v).sum::<f32>();
                e(*x).total_cmp(&e(*y))
            })
            .unwrap_or(a + target);
        out.push((a, cut));
        a = cut;
    }
    out
}

pub fn parakeet(models: &Models, path: &Path, start: f64, dur: f64, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Vec<Word>, String> {
    let wave = decode_audio(path, SR, start, Some(dur))?;
    if wave.is_empty() {
        return Ok(Vec::new());
    }
    let mut m = Parakeet::load(models)?;
    let chunks = chunk_bounds(&wave);
    let mut words = Vec::new();
    for (i, (a, b)) in chunks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen".into());
        }
        let (frames, dim) = m.encode(&wave[*a..*b])?;
        let toks = m.decode(&frames, dim)?;
        words.extend(m.words(&toks, *a as f64 / SR as f64));
        progress((i + 1) as f32 / chunks.len() as f32);
    }
    Ok(words)
}

/// Wörter zu gut lesbaren Untertitelzeilen gruppieren.
pub fn group_words(words: &[Word], max_chars: usize, max_dur: f64) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut cur: Option<Segment> = None;
    for (i, w) in words.iter().enumerate() {
        let gap = i > 0 && w.start - words[i - 1].end > 0.7;
        let flush = match &cur {
            Some(c) => {
                let len = c.text.chars().count() + 1 + w.text.chars().count();
                // Ein Satzende darf die Zeile etwas überziehen, statt allein zu stehen
                let ends = w.text.ends_with(['.', '!', '?']);
                let too_long = if ends { len > max_chars + 14 } else { len > max_chars };
                gap || too_long || w.end - c.start > max_dur + if ends { 1.5 } else { 0.0 }
            }
            None => false,
        };
        if flush {
            out.extend(cur.take());
        }
        match &mut cur {
            Some(c) => {
                c.text.push(' ');
                c.text.push_str(&w.text);
                c.end = w.end;
            }
            None => cur = Some(Segment { start: w.start, end: w.end, text: w.text.clone() }),
        }
        // Satzende beendet die Zeile
        if w.text.ends_with(['.', '!', '?']) {
            out.extend(cur.take());
        }
    }
    out.extend(cur);
    // Mindestdauer und keine Überlappung
    for i in 0..out.len() {
        let next = out.get(i + 1).map(|n| n.start);
        let s = &mut out[i];
        s.end = s.end.max(s.start + 0.8);
        if let Some(n) = next {
            s.end = s.end.min(n);
        }
    }
    out
}

// ---------------------------------------------------------------- Whisper

pub fn whisper(model: &Path, path: &Path, start: f64, dur: f64, language: &str, progress: &dyn Fn(f32)) -> Result<Vec<Segment>, String> {
    let dir = std::env::temp_dir().join(format!("cutline-whisper-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let srt = dir.join("out.srt");
    let _ = std::fs::remove_file(&srt);
    progress(0.05);
    let af = format!("aresample=16000,whisper=model={}:language={}:format=srt:destination={}:queue=10", esc(model), language, esc(&srt));
    let out = cl_media::ffmpeg_command()
        .map_err(|e| e.to_string())?
        .args(["-v", "error", "-nostdin", "-ss", &format!("{start:.3}"), "-t", &format!("{dur:.3}")])
        .arg("-i")
        .arg(path)
        .args(["-vn", "-af", &af, "-f", "null", "-"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("Whisper: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")));
    }
    progress(1.0);
    let text = std::fs::read_to_string(&srt).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(parse_srt(&text))
}

/// SRT lesen (Zeilen mit `-->`).
pub fn parse_srt(text: &str) -> Vec<Segment> {
    let ts = |s: &str| -> Option<f64> {
        let s = s.trim().replace(',', ".");
        let mut it = s.split(':');
        Some(it.next()?.parse::<f64>().ok()? * 3600.0 + it.next()?.parse::<f64>().ok()? * 60.0 + it.next()?.parse::<f64>().ok()?)
    };
    text.replace('\r', "")
        .split("\n\n")
        .filter_map(|block| {
            let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
            let tl = lines.iter().position(|l| l.contains("-->"))?;
            let (a, b) = lines[tl].split_once("-->")?;
            let txt = lines[tl + 1..].join(" ").trim().to_string();
            if txt.is_empty() { return None; }
            Some(Segment { start: ts(a)?, end: ts(b)?, text: txt })
        })
        .collect()
}

/// Rückgabe für Aufrufer, die einen Pfad brauchen.
pub fn _unused(_: PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn grouping_respects_limits_and_sentences() {
        let w = |s: f64, t: &str| Word { start: s, end: s + 0.3, text: t.into() };
        let words = vec![w(0.0, "Hallo"), w(0.4, "Welt."), w(1.0, "Das"), w(1.4, "ist"), w(1.8, "ein"), w(2.2, "ziemlich"), w(2.6, "langer"), w(3.0, "Satz"), w(3.4, "der"), w(3.8, "umbrechen"), w(4.2, "muss"), w(6.0, "Pause")];
        let s = group_words(&words, 24, 5.0);
        assert_eq!(s[0].text, "Hallo Welt.");
        assert!(s.iter().all(|x| x.text.chars().count() <= 24 + 14), "{s:?}");
        assert_eq!(s.last().unwrap().text, "Pause");
        assert!(s.windows(2).all(|p| p[0].end <= p[1].start));
    }

    #[test]
    fn srt_parsing() {
        let s = parse_srt("1\n00:00:01,000 --> 00:00:02,500\nHallo\n\n2\n00:00:03,000 --> 00:00:04,000\nzwei\nZeilen\n");
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].text, "zwei Zeilen");
        assert!((s[0].end - 2.5).abs() < 1e-9);
    }

    /// Echter Durchlauf mit deutscher Sprachausgabe (nur wenn Modelle + Windows-Stimme da sind).
    #[test]
    fn transcribes_german_with_parakeet_and_whisper() {
        let Some(models) = testutil::models() else { return };
        let dir = tempfile::tempdir().unwrap();
        let Some(wav) = testutil::speech(dir.path(), "Guten Morgen. Heute schneiden wir ein Video mit Cutline.") else { return };
        let dur = cl_media::probe(&wav).unwrap().duration.as_secs_f64();
        let cancel = AtomicBool::new(false);
        if models.parakeet().is_some() && models.ort_dll().is_some() {
            let t0 = std::time::Instant::now();
            let (eng, segs) = transcribe(&models, &wav, 0.0, dur, &Options::default(), &|_| {}, &cancel).unwrap();
            let all = segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
            println!("{} ({:.1} s): {all:?} {segs:?}", eng.name(), t0.elapsed().as_secs_f64());
            assert_eq!(eng, Engine::Parakeet);
            let low = all.to_lowercase();
            assert!(low.contains("morgen") && low.contains("video"), "{all}");
            assert!(segs.first().unwrap().start < 1.5 && segs.last().unwrap().end <= dur + 0.5);
        }
        if models.whisper().is_some() {
            let o = Options { force: Some(Engine::Whisper), language: "de".into(), ..Default::default() };
            let (_, segs) = transcribe(&models, &wav, 0.0, dur, &o, &|_| {}, &cancel).unwrap();
            let all = segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
            println!("Whisper: {all:?}");
            assert!(all.to_lowercase().contains("video"), "{all}");
        }
    }
}
