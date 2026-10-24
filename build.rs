//! Build script: embed the application icon and version resource into the Windows executable.
//!
//! On Windows the taskbar/Alt-Tab icon comes from the binary's resource section (not from
//! `WindowOptions`), so we compile `assets/icon.ico` into the exe with `embed-resource`. The same
//! resource section carries the `VERSIONINFO` block, which is what Windows' file properties show.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // The `VERSIONINFO` block is built from the package version, so a version bump has to re-run this
    // script even though the icon itself did not change.
    println!("cargo:rerun-if-changed=Cargo.toml");

    #[cfg(windows)]
    {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let icon = manifest_dir.join("assets").join("icon.ico");
        // rc.exe treats backslashes as escapes inside strings, so use forward slashes.
        let icon_rc = icon.display().to_string().replace('\\', "/");
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let rc_path = out_dir.join("app.rc");
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let [major, minor, patch, build] = version_quad(&version);
        let script = format!(
            "IDI_ICON1 ICON \"{icon_rc}\"\n\
             \n\
             1 VERSIONINFO\n\
             FILEVERSION {major},{minor},{patch},{build}\n\
             PRODUCTVERSION {major},{minor},{patch},{build}\n\
             FILEOS 0x40004\n\
             FILETYPE 0x1\n\
             BEGIN\n\
             \x20 BLOCK \"StringFileInfo\"\n\
             \x20 BEGIN\n\
             \x20   BLOCK \"040904b0\"\n\
             \x20   BEGIN\n\
             \x20     VALUE \"FileDescription\", \"League Auto Accept\"\n\
             \x20     VALUE \"FileVersion\", \"{version}\"\n\
             \x20     VALUE \"ProductName\", \"League Auto Accept\"\n\
             \x20     VALUE \"ProductVersion\", \"{version}\"\n\
             \x20     VALUE \"OriginalFilename\", \"league-auto-accept.exe\"\n\
             \x20   END\n\
             \x20 END\n\
             \x20 BLOCK \"VarFileInfo\"\n\
             \x20 BEGIN\n\
             \x20   VALUE \"Translation\", 0x409, 0x4b0\n\
             \x20 END\n\
             END\n"
        );
        std::fs::write(&rc_path, script).expect("failed to write resource script");
        let result = embed_resource::compile(&rc_path, embed_resource::NONE);
        eprintln!("embed-resource: {result}");
        result
            .manifest_optional()
            .expect("failed to embed the application icon");
    }
}

/// The four numbers a `VERSIONINFO` block wants, e.g. `1.4.1` → `[1, 4, 1, 0]`.
///
/// Windows pads the fourth field, which a three-part semver has no equivalent of, so it stays zero.
/// Any part that is not a plain number — a pre-release suffix, say — becomes zero as well: the
/// resource only has to be valid, and a build must not fail over a version string's shape.
#[cfg(windows)]
fn version_quad(version: &str) -> [u16; 4] {
    let mut quad = [0u16; 4];
    let parts = version.split(['.', '-', '+']).zip(quad.iter_mut());
    for (part, slot) in parts {
        *slot = part.parse().unwrap_or(0);
    }
    quad
}
