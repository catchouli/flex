fn main() {
    // Windows: embed the app icon in flex.exe (Explorer, shortcuts). The window icon
    // (taskbar/Dock) is set at runtime on every platform.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../assets/flex.rc");
        println!("cargo:rerun-if-changed=../../assets/flex.ico");
        embed_resource::compile("../../assets/flex.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compiling the Windows icon resource");
    }
    // macOS: libmpv's own dependencies are found via @rpath, which is resolved using the
    // executable's rpaths. Cover a future .app bundle and, for development, IINA's copy.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        for rpath in ["@executable_path/../Frameworks", "/Applications/IINA.app/Contents/Frameworks"] {
            println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{rpath}");
        }
    }
}
