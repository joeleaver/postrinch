//! Embeds Postrinch's icon, name and version in `postrinch.exe` when building
//! for Windows, the only system where a program carries its own icon:
//! Explorer, the Start menu, shortcuts and a pinned taskbar button read it
//! from the file, and show a blank one without it. Elsewhere this does
//! nothing.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/postrinch.ico");
    println!("cargo:rerun-if-changed={icon}");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(icon)
        .set("ProductName", "Postrinch")
        .set("FileDescription", "Postrinch")
        .set("CompanyName", "LBME Publishing")
        .set("LegalCopyright", "Copyright (C) 2026 LBME Publishing");
    // A failure is a build failure: a Windows program without its icon is
    // what this exists to prevent.
    resource
        .compile()
        .expect("embed Postrinch's icon in postrinch.exe");
}
