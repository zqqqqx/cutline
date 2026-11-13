#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod audio;
mod chrome;
mod creator;
mod edit_tools;
mod epidemic;
mod export_ui;
mod fx_mask;
mod home;
mod import_ui;
mod inspector;
mod keymap;
mod library;
mod library_ui;
mod multi_edit;
mod navigate;
mod onboarding;
mod paths;
mod player;
mod player_edit;
mod presets;
mod preview;
mod pro;
mod project_tools;
mod proxy;
mod record;
mod settings;
mod stock;
mod story;
mod tasks;
mod templates;
mod theme;
mod timeline;
mod update;
mod voiceover;
mod widgets;

pub use cl_i18n::tr;

use std::sync::{Arc, Mutex, OnceLock};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};

/// Hauptfenster als Elternfenster für Dateidialoge (sonst öffnen sie auf einem anderen Monitor).
struct Parent {
    w: RawWindowHandle,
    d: RawDisplayHandle,
}
// Die Handles werden nur an den Dialog weitergereicht, der auf dem UI-Thread läuft.
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

/// Dateidialog, der über dem Cutline-Fenster erscheint.
pub fn dialog() -> rfd::FileDialog {
    let d = rfd::FileDialog::new();
    match PARENT.get() {
        Some(p) => d.set_parent(p),
        None => d,
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Headless-MCP über stdio: `cutline --mcp projekt.cutline`
    if let Some(i) = args.iter().position(|a| a == "--mcp") {
        return run_mcp_stdio(args.get(i + 1).map(std::path::PathBuf::from));
    }

    let open = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(std::path::PathBuf::from);
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Cutline")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([900.0, 560.0])
        .with_decorations(false) // eigene Titelleiste (chrome.rs)
        .with_resizable(true)
        .with_drag_and_drop(true);
    if let Some(icon) = theme::window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    // Optionale Startposition, z. B. CUTLINE_WINDOW_POS="-1900,40" (Entwicklung/Tests auf Zweitmonitor)
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
        // Nicht blockierend präsentieren: Mit festem VSync hängt Windows das Zeichnen auf, sobald
        // das Hauptfenster verdeckt ist (z. B. vom Vollbild-Vorschaufenster). Die Bildrate
        // begrenzt die App beim Abspielen selbst (~120/s).
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
    .map_err(|e| anyhow::anyhow!("{e}"))
}

fn run_mcp_stdio(project: Option<std::path::PathBuf>) -> anyhow::Result<()> {
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
    server.serve_stdio()?;
    if let Some(p) = project {
        std::fs::write(&p, doc.lock().unwrap().project().to_json())?;
        eprintln!("Projekt gespeichert: {}", p.display());
    }
    Ok(())
}
