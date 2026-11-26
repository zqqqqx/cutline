#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod applog;
mod arrange;
mod assets;
mod captions;
mod audio;
mod brand;
mod chrome;
mod contents;
mod curves;
mod devinput;
mod dialog;
mod exports;
mod file_drop;
mod creator;
mod edit_tools;
mod epidemic;
mod export_ui;
mod fx_mask;
mod home;
mod import_ui;
mod inspector;
mod keymap;
mod layout;
mod library;
mod library_ui;
mod multi_edit;
mod navigate;
mod onboarding;
mod palette;
mod paths;
mod playcore;
mod player;
mod player_edit;
mod presets;
mod preview;
mod pro;
mod quick;
mod project_tools;
mod props;
mod proxy;
mod roto_ui;
mod record;
mod settings;
mod soundbars;
mod shorts;
mod stock;
mod story;
mod tasks;
mod templates;
mod thumb;
mod theme;
mod timeline;
mod update;
mod saving;
mod loading;
mod voiceover;
mod widgets;

pub use cl_i18n::tr;

use std::sync::{Arc, Mutex, OnceLock};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};

/// Main window as parent window for file dialogs (otherwise they open on another monitor).
struct Parent {
    w: RawWindowHandle,
    d: RawDisplayHandle,
}
// The handles are only passed to the dialog, which runs on the UI thread.
unsafe impl Send for Parent {}
unsafe impl Sync for Parent {}
impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(self.w) })
    }
}
impl HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(self.d) })
    }
}
static PARENT: OnceLock<Parent> = OnceLock::new();

pub fn set_dialog_parent(cc: &eframe::CreationContext<'_>) {
    if let (Ok(w), Ok(d)) = (cc.window_handle(), cc.display_handle()) {
        let _ = PARENT.set(Parent {
            w: w.as_raw(),
            d: d.as_raw(),
        });
    }
}

/// File dialog that appears above the Cutline window.
pub fn dialog() -> rfd::FileDialog {
    let d = rfd::FileDialog::new();
    match PARENT.get() {
        Some(p) => d.set_parent(p),
        None => d,
    }
}

/// Project to open from the command line: the first argument that is not an option. An empty
/// argument (start script with `"$1"` without a project) is none – otherwise Windows reported „Pfad nicht
/// gefunden (os error 3)“ at startup.
fn open_arg(args: &[String]) -> Option<std::path::PathBuf> {
    args.iter().skip(1).find(|a| !a.starts_with("--") && !a.trim().is_empty()).map(std::path::PathBuf::from)
}

#[test]
fn empty_argument_opens_no_project() {
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(open_arg(&a(&["cutline", ""])), None);
    assert_eq!(open_arg(&a(&["cutline", " ", "--x", "p.cutline"])), Some("p.cutline".into()));
    assert_eq!(open_arg(&a(&["cutline"])), None);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::perf_mark("main");
    let args: Vec<String> = std::env::args().collect();

    // Headless MCP over stdio: `cutline --mcp projekt.cutline`
    if let Some(i) = args.iter().position(|a| a == "--mcp") {
        return run_mcp_stdio(args.get(i + 1).map(std::path::PathBuf::from));
    }

    applog::init();
    let open = open_arg(&args);
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Cutline")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([900.0, 560.0])
        .with_decorations(false) // own title bar (chrome.rs)
        .with_resizable(true)
        .with_drag_and_drop(true);
    if let Some(icon) = theme::window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    // Optional start position, e.g. CUTLINE_WINDOW_POS="-1900,40" (development/tests on a second monitor)
    if let Some((x, y)) = std::env::var("CUTLINE_WINDOW_POS").ok().and_then(|s| {
        let (x, y) = s.split_once(',')?;
        Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
    }) {
        viewport = viewport.with_position([x, y]);
    }
    if let Some((w, h)) = std::env::var("CUTLINE_WINDOW_SIZE").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))
    }) {
        viewport = viewport.with_inner_size([w, h]);
    }
    if std::env::var("CUTLINE_MAXIMIZED").is_ok_and(|v| v == "1") {
        viewport = viewport.with_maximized(true);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        // Present without blocking: with fixed VSync, Windows stalls drawing as soon as
        // the main window is covered (e.g. by the fullscreen preview window). The app limits
        // the frame rate itself during playback (~120/s).
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            present_mode: eframe::wgpu::PresentMode::AutoNoVsync,
            ..Default::default()
        },
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        "Cutline",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, open)))),
    )
    .map_err(|e| {
        log::error!("Fenster: {e}");
        applog::flush();
        e.to_string().into()
    })
}

/// MCP mode: write the project only if it changed since the last save and no other running
/// Cutline window holds its lock (otherwise a stale copy would overwrite the user's edits).
fn mcp_save(p: &std::path::Path, d: &cl_core::Document, saved: &mut u64, me: u32) {
    if d.revision() == *saved {
        return;
    }
    if paths::lock_held_by_other(&paths::lock_path(p), me) {
        eprintln!("Nicht gespeichert: {} ist in einem anderen Cutline-Fenster geöffnet", p.display());
        return;
    }
    match paths::write_atomic(p, d.project().to_json()) {
        Ok(()) => *saved = d.revision(),
        Err(e) => eprintln!("Speichern fehlgeschlagen: {e}"),
    }
}

#[test]
fn mcp_save_skips_unchanged_and_locked_projects() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.cutline");
    let mut d = cl_core::Document::new(cl_core::Project::new("A"));
    let mut saved = d.revision();
    let me = std::process::id();
    // unchanged: nothing is written (the exit used to write unconditionally)
    mcp_save(&p, &d, &mut saved, me + 1);
    assert!(!p.exists());
    d.apply(cl_core::EditOp::AddTrack { kind: cl_core::TrackKind::Audio }, cl_core::OpSource::Ui).unwrap();
    // another running window (this test process, seen from another PID) holds the lock
    std::fs::write(paths::lock_path(&p), me.to_string()).unwrap();
    mcp_save(&p, &d, &mut saved, me + 1);
    assert!(!p.exists());
    std::fs::remove_file(paths::lock_path(&p)).unwrap();
    mcp_save(&p, &d, &mut saved, me + 1);
    assert!(p.exists() && saved == d.revision());
}

fn run_mcp_stdio(project: Option<std::path::PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    use cl_core::{Document, Project};
    let proj = match &project {
        Some(p) if p.is_file() => Project::from_json(&std::fs::read_to_string(p)?)?,
        Some(p) => Project::new(
            p.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Projekt".into()),
        ),
        None => Project::new("MCP-Projekt"),
    };
    let doc = Arc::new(Mutex::new(Document::new(proj)));
    let fonts = cl_fonts::FontLibrary::open(paths::font_dir())
        .ok()
        .map(|f| Arc::new(Mutex::new(f)));
    let server = cl_mcp::McpServer::new(doc.clone(), fonts);
    // save after every change: an abort by the client or a broken line loses nothing
    let mut saved = doc.lock().unwrap().revision();
    let me = std::process::id();
    let r = server.serve_stdio(|| {
        if let Some(p) = &project {
            mcp_save(p, &doc.lock().unwrap_or_else(|e| e.into_inner()), &mut saved, me);
        }
    });
    // last chance for a change the callback could not write; unchanged state is not rewritten
    if let Some(p) = &project {
        mcp_save(p, &doc.lock().unwrap_or_else(|e| e.into_inner()), &mut saved, me);
    }
    r?;
    Ok(())
}
