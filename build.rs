//! Build script for Ferrite
//!
//! This script handles platform-specific build tasks:
//! - Windows: Embeds the application icon into the executable

fn main() {
    // Windows: Embed icon into the executable
    #[cfg(target_os = "windows")]
    {
        let rc_path = std::path::Path::new("assets/icons/windows/app.rc");
        let ico_path = std::path::Path::new("assets/icons/windows/app.ico");

        // Only embed if both the .rc and .ico files exist
        if rc_path.exists() && ico_path.exists() {
            embed_resource::compile("assets/icons/windows/app.rc", embed_resource::NONE);
            println!("cargo:rerun-if-changed=assets/icons/windows/app.rc");
            println!("cargo:rerun-if-changed=assets/icons/windows/app.ico");
        } else {
            println!(
                "cargo:warning=Windows icon files not found at {:?} and {:?}. \
                 The executable will not have an embedded icon.",
                rc_path, ico_path
            );
        }
    }

    // Rerun build script if build.rs changes
    println!("cargo:rerun-if-changed=build.rs");
    // Translations are embedded at compile time by `rust_i18n::i18n!` in
    // `main.rs`; without this, cargo would not notice edited locale files and
    // changes to strings would only appear after a manual rebuild.
    println!("cargo:rerun-if-changed=locales");
}
