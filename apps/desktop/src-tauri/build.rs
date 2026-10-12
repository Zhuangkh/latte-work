fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/macos_quit.m")
            .file("src/macos_browser.m")
            .flag("-fobjc-arc")
            .compile("latte_macos_quit");
        println!("cargo:rustc-link-arg=-ObjC");
        println!("cargo:rerun-if-changed=src/macos_quit.m");
        println!("cargo:rerun-if-changed=src/macos_browser.m");
    }
    tauri_build::build()
}
