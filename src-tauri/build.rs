fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let minimum = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| {
            if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
                "11.0".into()
            } else {
                "10.13".into()
            }
        });
        let mut native = cc::Build::new();
        native
            .file("src/macos_writing_tools.m")
            .flag("-fobjc-arc")
            .flag(format!("-mmacosx-version-min={minimum}"));
        let compiler = native.get_compiler();
        native.compile("argus_writing_tools");

        // @available emits __isPlatformVersionAtLeast. Clang normally links
        // this compiler-rt archive automatically, but rustc uses -nodefaultlibs.
        // Query the same compiler that built the ObjC file, so Xcode versions,
        // custom CC settings and Intel/ARM targets use the matching runtime.
        let output = compiler
            .to_command()
            .arg("--print-file-name=libclang_rt.osx.a")
            .output()
            .expect("Could not query the macOS Clang runtime");
        assert!(
            output.status.success(),
            "Could not query the macOS Clang runtime: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let runtime = std::path::PathBuf::from(
            String::from_utf8(output.stdout)
                .expect("Clang runtime path is not UTF-8")
                .trim(),
        );
        assert!(
            runtime.is_file(),
            "macOS Clang runtime not found: {}",
            runtime.display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            runtime.parent().unwrap().display()
        );
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
        println!("cargo:rerun-if-changed={}", runtime.display());
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rerun-if-changed=src/macos_writing_tools.m");
    }
    tauri_build::build()
}
