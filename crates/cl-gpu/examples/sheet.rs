//! Check images of all shader transitions of the bundled packages:
//! `cargo run --release -p cl-gpu --example sheet -- <a.rgba> <b.rgba> <width> <height> <output-folder> [progress values…]`
//! Writes per transition and progress `<id>_<p>.rgba` (RGBA, same size as the inputs),
//! e.g. for a contact sheet tile with FFmpeg.

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 5 {
        eprintln!("Aufruf: sheet <a.rgba> <b.rgba> <breite> <höhe> <ausgabeordner> [Fortschritte…]");
        std::process::exit(2);
    }
    let (w, h): (u32, u32) = (a[2].parse().expect("Breite"), a[3].parse().expect("Höhe"));
    let (fa, fb) = (std::fs::read(&a[0]).expect("Bild A"), std::fs::read(&a[1]).expect("Bild B"));
    let out = std::path::PathBuf::from(&a[4]);
    std::fs::create_dir_all(&out).expect("Ausgabeordner");
    let steps: Vec<f32> = if a.len() > 5 {
        a[5..].iter().map(|s| s.parse().expect("Fortschritt")).collect()
    } else {
        vec![0.25, 0.5, 0.75]
    };
    let gpu = cl_gpu::gpu().expect("keine GPU");
    println!("{}", gpu.adapter);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
    let reg = cl_plugin::Registry::scan([root.as_path()]);
    let (ia, ib) = (cl_gpu::Frame { rgba: &fa, width: w, height: h }, cl_gpu::Frame { rgba: &fb, width: w, height: h });
    for (p, asset) in reg.assets(cl_plugin::AssetKind::Transition) {
        let Some(sh) = &asset.shader else { continue };
        let src = std::fs::read_to_string(p.dir.join(sh)).expect("Shader");
        let mut prm = [0.0; cl_gpu::PARAMS];
        let v = asset.shader_params().expect("Parameter");
        prm[..v.len()].copy_from_slice(&v);
        for s in &steps {
            let t = std::time::Instant::now();
            let img = gpu.render(&src, &prm, ia, ib, (w, h), *s).expect("Rendern");
            println!("{} {s}: {:.1} ms", asset.id, t.elapsed().as_secs_f64() * 1000.0);
            std::fs::write(out.join(format!("{}_{s}.rgba", asset.id)), img).expect("Schreiben");
        }
    }
}
