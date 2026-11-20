//! Where cargo puts a build script's output, as the build script needs it.

use std::path::Path;

/// The profile folder (`target/release`, `target/<triple>/release`) that
/// holds the `build` folder `out` (OUT_DIR) is under. Cargo has put OUT_DIR
/// at `<profile>/build/<crate>-<hash>/out` and at
/// `<profile>/build/<crate>/<hash>/out`, so this looks for the nearest
/// `build` folder instead of counting levels.
pub fn profile_dir(out: &Path) -> Option<&Path> {
    out.ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "build"))
        .and_then(Path::parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_dir_for_both_layouts() {
        let profile = Some(Path::new("/w/target/release"));
        // Older cargo: build/<crate>-<hash>/out.
        let old = Path::new("/w/target/release/build/video-player-5ed77130dbeacfb0/out");
        assert_eq!(profile_dir(old), profile);
        // Newer cargo: build/<crate>/<hash>/out.
        let new = Path::new("/w/target/release/build/video-player/5ed77130dbeacfb0/out");
        assert_eq!(profile_dir(new), profile);
    }

    #[test]
    fn profile_dir_with_an_explicit_target() {
        let out = Path::new("/w/target/x86_64-pc-windows-msvc/release/build/video-player/ab/out");
        assert_eq!(
            profile_dir(out),
            Some(Path::new("/w/target/x86_64-pc-windows-msvc/release"))
        );
    }

    #[test]
    fn no_build_folder_is_none() {
        assert_eq!(profile_dir(Path::new("/w/target/release/out")), None);
    }
}
