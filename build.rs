//! Build script: embed the application icon into the Windows executable.
//!
//! On Windows the taskbar/Alt-Tab icon comes from the binary's resource section (not from
//! `WindowOptions`), so we compile `assets/icon.ico` into the exe with `embed-resource`.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    #[cfg(windows)]
    {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let icon = manifest_dir.join("assets").join("icon.ico");
        // rc.exe treats backslashes as escapes inside strings, so use forward slashes.
        let icon_rc = icon.display().to_string().replace('\\', "/");
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let rc_path = out_dir.join("app.rc");
        std::fs::write(&rc_path, format!("IDI_ICON1 ICON \"{icon_rc}\"\n"))
            .expect("failed to write resource script");
        let result = embed_resource::compile(&rc_path, embed_resource::NONE);
        eprintln!("embed-resource: {result}");
        result
            .manifest_optional()
            .expect("failed to embed the application icon");
    }
}
