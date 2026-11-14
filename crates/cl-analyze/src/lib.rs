//! Lokale Analysen für Phase 3 – alles läuft auf diesem Rechner, nichts geht in eine Cloud:
//!
//! - [`asr`]: Spracherkennung mit **NVIDIA Parakeet TDT 0.6B v3** (ONNX, 25 europäische
//!   Sprachen, Wort-Zeitstempel); Rückfall auf **Whisper** (in FFmpeg 8 eingebaut)
//! - [`matte`]: Hintergrund entfernen mit **RobustVideoMatting** (ONNX) → Masken-Video
//! - [`scenes`]: Szenenwechsel (FFmpeg `scdet`)
//! - [`beats`]: Beat-Erkennung (Onsets + Tempo-Raster)
//! - [`track`]: Bewegung verfolgen (Vorlagenvergleich) → Keyframes
//! - [`reframe`]: Auto-Reframe (Bildschwerpunkt je Zeitpunkt)
//!
//! Modelle werden nicht mitgeliefert, sondern bei Bedarf geladen ([`CATALOG`]).

pub mod asr;
pub mod audio;
pub mod beats;
pub mod matte;
pub mod reframe;
pub mod scenes;
pub mod track;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

/// Lädbare Modelle: (id, Name, Beschreibung, Größe MB, Dateien (Ziel relativ zum Modellordner, URL)).
/// URLs zeigen auf feste Commits; `#sha256=<hex>` am Ende wird von `cl_update::download_to`
/// vor dem Umbenennen geprüft (fehlt bei Dateien ohne veröffentlichte Prüfsumme).
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub size_mb: u32,
    pub files: &'static [(&'static str, &'static str)],
}

const PK: &str = "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/";

pub static CATALOG: &[ModelInfo] = &[
    ModelInfo {
        id: "onnxruntime",
        name: "ONNX Runtime 1.31",
        about: "Laufzeit für Parakeet und Freistellen (Microsoft, MIT-Lizenz)",
        size_mb: 17,
        files: &[("onnxruntime.zip", "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-win-x64-1.31.1.zip#sha256=5869e26bb25314375f2d5ef1877e3ea06b1543d6c710e6e4a1ec2f7177ab4b2c")],
    },
    ModelInfo {
        id: "parakeet",
        name: "Parakeet TDT 0.6B v3",
        about: "Beste Spracherkennung für Deutsch und 24 weitere europäische Sprachen, Wort-genaue Zeiten (NVIDIA, CC-BY-4.0)",
        size_mb: 670,
        files: &[
            ("parakeet-tdt-0.6b-v3/config.json", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/config.json"),
            ("parakeet-tdt-0.6b-v3/vocab.txt", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/vocab.txt"),
            ("parakeet-tdt-0.6b-v3/nemo128.onnx", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/nemo128.onnx#sha256=a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f"),
            ("parakeet-tdt-0.6b-v3/decoder_joint-model.int8.onnx", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/decoder_joint-model.int8.onnx#sha256=eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70"),
            ("parakeet-tdt-0.6b-v3/encoder-model.int8.onnx", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/encoder-model.int8.onnx#sha256=6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09"),
        ],
    },
    ModelInfo {
        id: "whisper",
        name: "Whisper Base",
        about: "Rückfall-Spracherkennung, 99 Sprachen (OpenAI, MIT-Lizenz)",
        size_mb: 148,
        files: &[("ggml-base.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin#sha256=60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe")],
    },
    ModelInfo {
        id: "rvm",
        name: "RobustVideoMatting",
        about: "Personen freistellen ohne Greenscreen (GPL-3.0)",
        size_mb: 15,
        files: &[("rvm_mobilenetv3_fp32.onnx", "https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx")],
    },
];

const _: &str = PK;

/// Modellordner mit Suchlogik für die einzelnen Dateien.
#[derive(Debug, Clone)]
pub struct Models {
    pub dir: PathBuf,
}

impl Models {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `onnxruntime.dll` neben der EXE, im Modellordner oder im entpackten Release-Ordner.
    pub fn ort_dll(&self) -> Option<PathBuf> {
        let name = if cfg!(windows) {
            "onnxruntime.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        };
        let mut cands = vec![];
        if let Some(d) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        {
            cands.push(d.join(name));
        }
        cands.push(self.dir.join(name));
        if let Ok(rd) = std::fs::read_dir(self.dir.join("ort")) {
            for e in rd.flatten() {
                cands.push(e.path().join("lib").join(name));
            }
        }
        cands.into_iter().find(|p| p.is_file())
    }
    pub fn parakeet(&self) -> Option<PathBuf> {
        let d = self.dir.join("parakeet-tdt-0.6b-v3");
        [
            "nemo128.onnx",
            "encoder-model.int8.onnx",
            "decoder_joint-model.int8.onnx",
            "vocab.txt",
        ]
        .iter()
        .all(|f| d.join(f).is_file())
        .then_some(d)
    }
    pub fn whisper(&self) -> Option<PathBuf> {
        [
            "ggml-large-v3-turbo.bin",
            "ggml-small.bin",
            "ggml-base.bin",
            "ggml-tiny.bin",
        ]
        .iter()
        .map(|f| self.dir.join(f))
        .find(|p| p.is_file())
    }
    pub fn rvm(&self) -> Option<PathBuf> {
        Some(self.dir.join("rvm_mobilenetv3_fp32.onnx")).filter(|p| p.is_file())
    }
    /// Ist ein Katalogeintrag vollständig vorhanden?
    pub fn installed(&self, id: &str) -> bool {
        match id {
            "onnxruntime" => self.ort_dll().is_some(),
            "parakeet" => self.parakeet().is_some(),
            "whisper" => self.whisper().is_some(),
            "rvm" => self.rvm().is_some(),
            _ => false,
        }
    }
}

static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();

/// ONNX-Runtime einmalig laden.
pub fn ort_env(models: &Models) -> Result<&'static ort::environment::Environment, String> {
    ENV.get_or_init(|| {
        let dll = models
            .ort_dll()
            .ok_or("ONNX Runtime fehlt (Einstellungen → Modelle)")?;
        ort::init_from(&dll)
            .map_err(|e| e.to_string())?
            .with_name("cutline")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| e.clone())
}

pub(crate) fn session(models: &Models, path: &Path) -> Result<ort::session::Session, String> {
    let env = ort_env(models)?;
    ort::session::Session::builder(env)
        .map_err(|e| e.to_string())?
        .with_intra_threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(8),
        )
        .map_err(|e| e.to_string())?
        .commit_from_file(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Ton als 16-kHz-Mono-f32 (für Spracherkennung und Beats).
pub fn decode_audio(
    path: &Path,
    rate: u32,
    start: f64,
    dur: Option<f64>,
) -> Result<Vec<f32>, String> {
    let mut cmd = cl_media::ffmpeg_command().map_err(|e| e.to_string())?;
    cmd.args(["-v", "error", "-nostdin", "-ss", &format!("{start:.3}")]);
    if let Some(d) = dur {
        cmd.args(["-t", &format!("{d:.3}")]);
    }
    let out = cmd
        .arg("-i")
        .arg(path)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            &rate.to_string(),
            "-f",
            "f32le",
            "-",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .last()
            .unwrap_or("Ton nicht lesbar")
            .to_string());
    }
    Ok(out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// Videobilder als Graustufen (w×h) mit `fps` Bildern pro Sekunde, ab `start` für `dur` Sekunden.
pub fn gray_frames(
    path: &Path,
    w: u32,
    h: u32,
    fps: f64,
    start: f64,
    dur: f64,
    mut each: impl FnMut(usize, &[u8]) -> bool,
) -> Result<usize, String> {
    let mut child = cl_media::ffmpeg_command()
        .map_err(|e| e.to_string())?
        .args([
            "-v",
            "error",
            "-nostdin",
            "-ss",
            &format!("{start:.3}"),
            "-t",
            &format!("{dur:.3}"),
        ])
        .arg("-i")
        .arg(path)
        .args([
            "-an",
            "-vf",
            &format!("fps={fps},scale={w}:{h}:flags=area,format=gray"),
            "-f",
            "rawvideo",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().expect("piped");
    let mut buf = vec![0u8; (w * h) as usize];
    let mut n = 0;
    while out.read_exact(&mut buf).is_ok() {
        if !each(n, &buf) {
            let _ = child.kill();
            break;
        }
        n += 1;
    }
    let _ = child.wait();
    Ok(n)
}

/// Pfad für FFmpeg-Filteroptionen escapen.
pub(crate) fn esc(p: &Path) -> String {
    format!(
        "'{}'",
        p.to_string_lossy()
            .replace('\\', "/")
            .replace(':', "\\:")
            .replace('\'', "")
    )
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// Modellordner der Entwicklungsumgebung (CUTLINE_MODELS oder %LOCALAPPDATA%/cutline-models).
    pub fn models() -> Option<Models> {
        let d = std::env::var_os("CUTLINE_MODELS")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("cutline-models"))
            })?;
        d.is_dir().then(|| Models::new(d))
    }

    /// Gesprochenes Deutsch als Testdatei (Windows-Sprachausgabe), sonst None.
    pub fn speech(dir: &Path, text: &str) -> Option<PathBuf> {
        if !cfg!(windows) {
            return None;
        }
        let wav = dir.join("speech.wav");
        let ps = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $v = $s.GetInstalledVoices() | Where-Object {{ $_.VoiceInfo.Culture.Name -like 'de*' }} | Select-Object -First 1; if ($v) {{ $s.SelectVoice($v.VoiceInfo.Name) }}; $s.Rate = 0; $s.SetOutputToWaveFile('{}'); $s.Speak('{}'); $s.Dispose()",
            wav.display(),
            text
        );
        let ok = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps])
            .status()
            .ok()?
            .success();
        (ok && wav.is_file()).then_some(wav)
    }
}
