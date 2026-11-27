//! Local analyses for phase 3 – everything runs on this computer, nothing goes to a cloud:
//!
//! - [`asr`]: speech recognition with **NVIDIA Parakeet TDT 0.6B v3** (ONNX, 25 European
//!   languages, word timestamps); fallback to **Whisper** (built into FFmpeg 8)
//! - [`matte`]: background removal with **RobustVideoMatting** (ONNX) → mask video
//! - [`scenes`]: scene changes (FFmpeg `scdet`)
//! - [`beats`]: beat detection (onsets + tempo grid)
//! - [`track`]: motion tracking (template matching) → keyframes
//! - [`reframe`]: auto reframe (image center of interest per point in time)
//!
//! Models are not bundled but downloaded on demand ([`CATALOG`]).

pub mod asr;
pub mod audio;
pub mod beats;
pub mod matte;
pub mod reframe;
pub mod roto;
pub mod scenes;
pub mod track;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Downloadable models: (id, name, description, size MB, files (target relative to the model folder, URL)).
/// URLs point to fixed commits; `#sha256=<hex>` at the end is checked by `cl_update::download_to`
/// before renaming (missing for files without a published checksum).
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub size_mb: u32,
    pub files: &'static [(&'static str, &'static str)],
}

pub static CATALOG: &[ModelInfo] = &[
    ModelInfo {
        id: "onnxruntime",
        name: "ONNX Runtime 1.31",
        about: "Runtime for Parakeet and cutouts (Microsoft, MIT license)",
        size_mb: 17,
        files: &[("onnxruntime.zip", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/onnxruntime-win-x64-1.31.1.zip#sha256=5869e26bb25314375f2d5ef1877e3ea06b1543d6c710e6e4a1ec2f7177ab4b2c")],
    },
    ModelInfo {
        id: "parakeet",
        name: "Parakeet TDT 0.6B v3",
        about: "Best speech recognition for 25 European languages, word-level timing (NVIDIA, CC-BY-4.0)",
        size_mb: 670,
        files: &[
            ("parakeet-tdt-0.6b-v3/config.json", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/config.json#sha256=666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466"),
            ("parakeet-tdt-0.6b-v3/vocab.txt", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/vocab.txt#sha256=d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"),
            ("parakeet-tdt-0.6b-v3/nemo128.onnx", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/nemo128.onnx#sha256=a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f"),
            ("parakeet-tdt-0.6b-v3/decoder_joint-model.int8.onnx", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/decoder_joint-model.int8.onnx#sha256=eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70"),
            ("parakeet-tdt-0.6b-v3/encoder-model.int8.onnx", "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/encoder-model.int8.onnx#sha256=6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09"),
        ],
    },
    ModelInfo {
        id: "whisper",
        name: "Whisper Base",
        about: "Fallback speech recognition, 99 languages (OpenAI, MIT license)",
        size_mb: 148,
        files: &[("ggml-base.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin#sha256=60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe")],
    },
    ModelInfo {
        id: "rvm",
        name: "RobustVideoMatting",
        about: "Cut out people without a green screen (GPL-3.0)",
        size_mb: 15,
        files: &[("rvm_mobilenetv3_fp32.onnx", "https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx#sha256=88d4531297118f595bf2fd60f6f566aec2e559393802d1f436c380f0cbbd2828")],
    },
    // export: huggingface.co/diffusionstudio/sam2.1-tiny-video-onnx-fp16 (Apache-2.0), mirrored
    ModelInfo {
        id: "sam2",
        name: "SAM 2.1 Tiny",
        about: "Cut out and track objects with a click (Meta, Apache-2.0)",
        size_mb: 80,
        files: &[
            ("sam2.1-tiny-512/onnx/vision_encoder.onnx", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/sam2.1-tiny-512-fp16-vision_encoder.onnx#sha256=24e9ccf5994be5c8dd93ebe771a4cc4f8bb122d0524555ac3ba4e7141710b133"),
            ("sam2.1-tiny-512/onnx/mask_decoder.onnx", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/sam2.1-tiny-512-fp16-mask_decoder.onnx#sha256=6bc158b0d028777ca2e25203ece0f27429c436de39a5c7fe301b3858d6059789"),
            ("sam2.1-tiny-512/onnx/memory_encoder.onnx", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/sam2.1-tiny-512-fp16-memory_encoder.onnx#sha256=8b2bb6810444cee684ef938b7464c5645653f236014f5497c3db3654818cc7ce"),
            ("sam2.1-tiny-512/onnx/memory_attention.onnx", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/sam2.1-tiny-512-fp16-memory_attention.onnx#sha256=67c4e4da01bc7f501b88294bb19a8200362915ece04e9d7e367a437658f342c9"),
            ("sam2.1-tiny-512/onnx/pointer_tpos.onnx", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/sam2.1-tiny-512-fp16-pointer_tpos.onnx#sha256=3ab5bfce40ed01208301b2d8f596b9c9f7f493dcd7e09ef01ec22e5be3694bd7"),
            ("sam2.1-tiny-512/constants.json", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/sam2.1-tiny-512-fp16-constants.json#sha256=67583810f4b80b762285377a5cc1066bbe19cdf8ef7fc87ff6aa78fc79830ef2"),
            ("sam2.1-tiny-512/LICENSE.txt", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/sam2.1-LICENSE-Apache-2.0.txt#sha256=cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"),
        ],
    },
    // Windows only (the settings leave it out elsewhere). Microsoft stopped the DirectML build of
    // ONNX Runtime at 1.24.4; DirectML itself is proprietary (redistribution with an application allowed)
    ModelInfo {
        id: "directml",
        name: "Graphics card acceleration",
        about: "Object cut-out on the graphics card (Microsoft DirectML 1.15.4, proprietary license; ONNX Runtime 1.24.4, MIT license)",
        size_mb: 35,
        files: &[
            ("directml/onnxruntime.dll", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/onnxruntime-directml-1.24.4-win-x64.dll#sha256=e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40"),
            ("directml/DirectML.dll", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/DirectML-1.15.4-win-x64.dll#sha256=9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1"),
            ("directml/LICENSE-DirectML.txt", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/DirectML-1.15.4-LICENSE.txt#sha256=a05138e3a085ff60a44881eedfa58dccb03ecc1d7b1f6ae888418e8c2fec4b8d"),
            ("directml/LICENSE-onnxruntime.txt", "https://github.com/zqqqqx/cutline/releases/download/ci-tools/onnxruntime-directml-1.24.4-LICENSE.txt#sha256=c250d6278f0b47a6439fb7592b08b58a55eb9f535aa49a1db63211c3f982b674"),
        ],
    },
];

/// Catalog entry for this system? (the graphics card runtime exists only for Windows)
pub fn offered(id: &str) -> bool {
    id != "directml" || cfg!(windows)
}

/// Model folder with search logic for the individual files.
#[derive(Debug, Clone)]
pub struct Models {
    pub dir: PathBuf,
}

impl Models {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `onnxruntime.dll` next to the EXE, in the model folder or in the unpacked release folder.
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
    /// Folder of the object model (five graphs and constants)
    pub fn sam2(&self) -> Option<PathBuf> {
        let d = self.dir.join(roto::MODEL_DIR);
        (d.join("constants.json").is_file() && roto::GRAPHS.iter().all(|g| d.join("onnx").join(format!("{g}.onnx")).is_file())).then_some(d)
    }
    /// Folder with the DirectML build of the runtime and `DirectML.dll` (Windows only)
    pub fn gpu_runtime(&self) -> Option<PathBuf> {
        let d = self.dir.join("directml");
        (cfg!(windows) && ["onnxruntime.dll", "DirectML.dll"].iter().all(|f| d.join(f).is_file())).then_some(d)
    }
    /// Is a catalog entry completely present?
    pub fn installed(&self, id: &str) -> bool {
        match id {
            "onnxruntime" => self.ort_dll().is_some(),
            "parakeet" => self.parakeet().is_some(),
            "whisper" => self.whisper().is_some(),
            "rvm" => self.rvm().is_some(),
            "sam2" => self.sam2().is_some(),
            "directml" => self.gpu_runtime().is_some(),
            _ => false,
        }
    }
}

static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
/// Checked folder of the graphics card runtime (see [`use_gpu_runtime`])
static GPU_DIR: OnceLock<PathBuf> = OnceLock::new();
static GPU_LOADED: AtomicBool = AtomicBool::new(false);
static NOTE: Mutex<Option<String>> = Mutex::new(None);

/// Load the runtime from `dir` (DirectML build + `DirectML.dll`, files checked by the caller)
/// instead of the bundled one, if the runtime is not loaded yet. Only one runtime per process:
/// it then serves every model.
pub fn use_gpu_runtime(dir: PathBuf) {
    let _ = GPU_DIR.set(dir);
}

/// The loaded runtime is the DirectML build.
pub fn gpu_runtime_loaded() -> bool {
    GPU_LOADED.load(Ordering::Relaxed)
}

/// Has a runtime been loaded (successfully or not)?
pub fn runtime_loaded() -> bool {
    ENV.get().is_some()
}

/// Why the graphics card is not used (load or session error), for a status line.
pub fn runtime_note() -> Option<String> {
    NOTE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn note_runtime(s: String) {
    eprintln!("ONNX Runtime: {s}");
    *NOTE.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
}

/// Load `DirectML.dll` from its full path first: the runtime's import of it then resolves to this
/// module instead of the older copy in System32 (Windows searches the exe folder, not the runtime's).
#[cfg(windows)]
fn preload(dll: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut std::ffi::c_void;
    }
    let w: Vec<u16> = dll.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: plain Win32 call with a NUL-terminated path; the module stays loaded on purpose
    !unsafe { LoadLibraryW(w.as_ptr()) }.is_null()
}

/// Load the ONNX runtime once: the graphics card build if [`use_gpu_runtime`] named one and it loads,
/// else the bundled one (a failed attempt leaves nothing behind, see `ort::init_from`).
pub fn ort_env(models: &Models) -> Result<&'static ort::environment::Environment, String> {
    ENV.get_or_init(|| {
        #[cfg(windows)]
        if let Some(d) = GPU_DIR.get() {
            if !preload(&d.join("DirectML.dll")) {
                note_runtime("DirectML.dll cannot be loaded".into());
            } else {
                match ort::init_from(d.join("onnxruntime.dll")) {
                    Ok(b) => {
                        GPU_LOADED.store(true, Ordering::Relaxed);
                        return b.with_name("cutline").build().map_err(|e| e.to_string());
                    }
                    Err(e) => note_runtime(e.to_string()),
                }
            }
        }
        let dll = models
            .ort_dll()
            .ok_or("ONNX Runtime missing (Settings → Models)")?;
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

/// Audio as 16 kHz mono f32 (for speech recognition and beats).
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
            .unwrap_or("Audio can't be read")
            .to_string());
    }
    Ok(out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// Video frames as grayscale (w×h) at `fps` frames per second, from `start` for `dur` seconds.
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

/// Every catalog file is checked against a SHA-256 sum after download (`#sha256=` at the
/// address, evaluated by `cl_update::download_to`) – a new file without a sum shows up here.
/// Guarantees only that each address carries a well-formed 64-hex sum, not that it matches the file.
#[test]
fn every_model_url_carries_a_sha256() {
    for m in CATALOG {
        for (name, url) in m.files {
            let sum = url.rsplit_once("#sha256=").map(|(_, h)| h).unwrap_or("");
            assert!(sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()), "{}: {name} without checksum", m.id);
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// Model folder of the development environment (CUTLINE_MODELS or %LOCALAPPDATA%/cutline-models).
    pub fn models() -> Option<Models> {
        let d = std::env::var_os("CUTLINE_MODELS")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("cutline-models"))
            });
        let m = d.filter(|d| d.is_dir()).map(Models::new);
        if m.is_none() {
            eprintln!("skipped (no model folder, set CUTLINE_MODELS)");
        }
        m
    }

    /// Spoken German as a test file (Windows speech output), otherwise None (not Windows).
    /// Panics if Windows has no German voice: an English voice would make the check meaningless.
    pub fn speech(dir: &Path, text: &str) -> Option<PathBuf> {
        if !cfg!(windows) {
            eprintln!("skipped (speech output needs Windows)");
            return None;
        }
        let wav = dir.join("speech.wav");
        let ps = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $v = $s.GetInstalledVoices() | Where-Object {{ $_.VoiceInfo.Culture.Name -like 'de*' }} | Select-Object -First 1; if (-not $v) {{ exit 3 }}; $s.SelectVoice($v.VoiceInfo.Name); $s.Rate = 0; $s.SetOutputToWaveFile('{}'); $s.Speak('{}'); $s.Dispose()",
            wav.display(),
            text
        );
        let st = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps])
            .status()
            .expect("powershell");
        assert_ne!(st.code(), Some(3), "no German Windows voice installed (Settings > Time & language > Speech)");
        assert!(st.success() && wav.is_file(), "speech output failed: {st}");
        Some(wav)
    }
}
