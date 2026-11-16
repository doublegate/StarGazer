//! Embeds the application icon and version information into the Windows
//! executable. Does nothing for other targets.
//!
//! `winresource` is a Windows-host-only build dependency (see Cargo.toml):
//! it drives the Windows SDK resource compiler, which the release builds
//! use on their Windows runner. The host check (`cfg(windows)`) guards the
//! dependency; the target check makes a Windows host building for another
//! target skip the step.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/stargazer.ico");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // File version, product version and name come from Cargo.toml.
        res.set_icon("assets/icons/stargazer.ico");
        res.set(
            "FileDescription",
            "StarGazer: browse your GitHub stars in the terminal",
        );
        res.set("LegalCopyright", "MIT License");
        if let Err(e) = res.compile() {
            // A missing icon would silently ship a generic executable;
            // fail the build instead.
            panic!("failed to embed Windows resources: {e}");
        }
    }
}
