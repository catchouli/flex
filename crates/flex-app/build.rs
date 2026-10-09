fn main() {
    // macOS: libmpv's own dependencies are found via @rpath, which is resolved using the
    // executable's rpaths. Cover a future .app bundle and, for development, IINA's copy.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        for rpath in ["@executable_path/../Frameworks", "/Applications/IINA.app/Contents/Frameworks"] {
            println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{rpath}");
        }
    }
}
