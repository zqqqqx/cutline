//! Ein Bewegungssystem: Dauern nur aus den drei Konstanten in `theme.rs`, kein Standard-Kreisel,
//! kein Neuzeichnen ohne Bedingung.

fn sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    files
        .into_iter()
        .filter(|f| f.extension().is_some_and(|e| e == "rs"))
        .map(|f| {
            let src = std::fs::read_to_string(&f).unwrap();
            // Tests am Dateiende zählen nicht
            let src = src.split("mod tests {").next().unwrap_or(&src).to_string();
            (f.file_name().unwrap().to_string_lossy().into_owned(), src)
        })
        .collect()
}

/// Argumente aller Aufrufe `anim(…)` (nicht `xanim(`)
fn anim_args(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("anim(") {
        let prev = rest[..i].chars().next_back();
        rest = &rest[i + 5..];
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        out.push(rest[..rest.find(')').unwrap_or(0)].trim().to_string());
    }
    out
}

#[test]
fn durations_only_from_the_three_constants() {
    let ok = ["MOTION_FAST", "MOTION", "MOTION_SLOW"];
    let mut bad = Vec::new();
    for (name, src) in sources() {
        for a in anim_args(&src) {
            let a = a.trim_start_matches("theme::").trim_start_matches("crate::theme::");
            // `theme.rs` selbst: Definition und die Helfer, die eine Konstante weiterreichen
            if !ok.contains(&a) && !(name == "theme.rs" && (a == "secs" || a == "secs: f32")) {
                bad.push(format!("{name}: anim({a})"));
            }
        }
    }
    assert!(bad.is_empty(), "Dauern außerhalb von MOTION_FAST/MOTION/MOTION_SLOW:\n{}", bad.join("\n"));
}

#[test]
fn no_standard_spinner_and_no_eased_overshoot() {
    for (name, src) in sources() {
        assert!(!src.contains("Spinner::new"), "{name}: Kreisel statt busy_line/progress_line/skeleton");
        assert!(!src.contains("animate_bool_with_time("), "{name}: theme::fade nutzen (eine Kurve)");
    }
}

#[test]
fn repaint_never_requested_unconditionally_in_a_frame() {
    // `request_repaint()` direkt im Rumpf einer Funktion (nächster umschließender Block ist die
    // Funktion selbst, kein `if`/`match`/Thread) liefe in jedem Bild: Dauerlast im Stand.
    let mut bad = Vec::new();
    for (name, src) in sources() {
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            if !t.starts_with("ui.ctx().request_repaint();") && !t.starts_with("ctx.request_repaint();") {
                continue;
            }
            let depth = line.len() - t.len();
            let open = lines[..i].iter().rev().find(|l| {
                let lt = l.trim_start();
                l.len() - lt.len() < depth && l.trim_end().ends_with('{')
            });
            if open.is_some_and(|l| {
                let lt = l.trim_start();
                lt.starts_with("fn ") || lt.starts_with("pub fn ") || lt.starts_with("pub(crate) fn ")
            }) {
                bad.push(format!("{name}:{}: {t}", i + 1));
            }
        }
    }
    assert!(bad.is_empty(), "Neuzeichnen ohne Bedingung:\n{}", bad.join("\n"));
}
