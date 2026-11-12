//! Windows: App-Icon und Versionsinfo in die cutline.exe einbetten.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/cutline.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/cutline.ico");
        res.set("ProductName", "Cutline");
        res.set("FileDescription", "Cutline video editor");
        res.set("LegalCopyright", "MPL-2.0");
        if let Err(e) = res.compile() {
            // Ohne Resource Compiler trotzdem bauen, nur ohne Icon
            println!("cargo:warning=Icon nicht eingebettet: {e}");
        }
    }
}
