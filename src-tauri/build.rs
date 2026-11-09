fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let minimum = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| {
            if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
                "11.0".into()
            } else {
                "10.13".into()
            }
        });
        cc::Build::new()
            .file("src/macos_writing_tools.m")
            .flag("-fobjc-arc")
            .flag(format!("-mmacosx-version-min={minimum}"))
            .compile("argus_writing_tools");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rerun-if-changed=src/macos_writing_tools.m");
    }
    tauri_build::build()
}
