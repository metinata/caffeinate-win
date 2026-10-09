//! Embeds version information in caffeinate.exe (Properties → Details, Task Manager).

fn main() {
    // The embedded version comes from Cargo.toml; nothing else affects the output.
    // Without these lines, cargo would rerun this script whenever any file changes.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Needs the resource compiler from the Windows SDK, which MSVC builds already
    // require. Other toolchains (e.g. windows-gnu) build without the metadata.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set("FileDescription", "caffeinate - keep Windows awake")
        .set("ProductName", "caffeinate for Windows")
        .set("InternalName", "caffeinate")
        .set("OriginalFilename", "caffeinate.exe")
        .set(
            "LegalCopyright",
            "Copyright (c) 2026 Metin Ata YETER. MIT License.",
        );
    if let Err(e) = resource.compile() {
        panic!("failed to embed Windows resources: {e}");
    }
}
