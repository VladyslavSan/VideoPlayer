//! Gets libmpv for the build. mpv is a C library: the `libmpv2` crate only
//! declares its functions, so a compiled libmpv has to be there to link to
//! and to ship. A pinned build is downloaded into `third_party/mpv` the first
//! time, checked against its SHA-256, and reused after that.
//!
//! `MPV_LIB_DIR` points at a libmpv of your own instead (nothing is
//! downloaded): `libmpv-2.dll` and `mpv.lib` on Windows, `libmpv.dylib` and
//! the dylibs it needs on macOS.

use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// A libmpv build: where to get it, and what it must hash to.
pub struct Pinned {
    pub url: &'static str,
    pub sha256: &'static str,
    unpack: fn(&[u8], &Path) -> Result<(), String>,
}

/// shinchiro's mpv-winbuild-cmake: libmpv-2.dll (x64).
pub const WINDOWS: Pinned = Pinned {
    url: "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261106/mpv-dev-x86_64-20261106-git-9028187538.7z",
    sha256: "b5a3cc91b5dabf88ad5bb21d000834da4fd0bbda30f4584d39c082127bd62414",
    unpack: unpack_windows,
};

/// media-kit's libmpv-darwin-build: universal (arm64 + x86_64) dylibs with
/// OpenGL rendering and VideoToolbox decoding, all using @rpath.
pub const MACOS: Pinned = Pinned {
    url: "https://github.com/media-kit/libmpv-darwin-build/releases/download/v0.7.3/libmpv-libs_v0.7.3_macos-universal-video-full.tar.gz",
    sha256: "f570695d8ba8534835dd4cba0b6e275579c1c22c01a9da0442748ca37e2daac1",
    unpack: unpack_macos,
};

/// Written last, so a folder without it (or with an older pin) is fetched
/// again.
const STAMP: &str = ".pinned";

/// The folder holding libmpv: `MPV_LIB_DIR`, or `dir` with `pinned` in it.
pub fn find_or_fetch(pinned: &Pinned, dir: &Path) -> Result<PathBuf, String> {
    println!("cargo:rerun-if-env-changed=MPV_LIB_DIR");
    if let Some(own) = std::env::var_os("MPV_LIB_DIR") {
        return Ok(PathBuf::from(own));
    }
    let stamp = dir.join(STAMP);
    println!("cargo:rerun-if-changed={}", stamp.display());
    if fs::read_to_string(&stamp).is_ok_and(|s| s.trim() == pinned.sha256) {
        return Ok(dir.to_path_buf());
    }
    fetch(pinned, dir)?;
    Ok(dir.to_path_buf())
}

/// Downloads and unpacks `pinned` into `dir`, replacing what was there.
pub fn fetch(pinned: &Pinned, dir: &Path) -> Result<(), String> {
    println!(
        "cargo:warning=Downloading libmpv (first build only): {}",
        pinned.url
    );
    let archive = download(pinned)?;
    // Unpacked beside it and swapped in, so an interrupted build never
    // leaves half a library behind.
    let partial = dir.with_extension(format!("partial-{}", std::process::id()));
    let _ = fs::remove_dir_all(&partial);
    fs::create_dir_all(&partial).map_err(|e| format!("creating {}: {e}", partial.display()))?;
    let unpacked = (pinned.unpack)(&archive, &partial)
        .and_then(|()| {
            fs::write(partial.join(STAMP), pinned.sha256).map_err(|e| format!("stamp: {e}"))
        })
        .and_then(|()| {
            let _ = fs::remove_dir_all(dir);
            fs::rename(&partial, dir).map_err(|e| format!("moving into {}: {e}", dir.display()))
        });
    if unpacked.is_err() {
        let _ = fs::remove_dir_all(&partial);
    }
    unpacked
}

fn download(pinned: &Pinned) -> Result<Vec<u8>, String> {
    let failed = |e: &dyn std::fmt::Display| {
        format!(
            "downloading libmpv from {} failed: {e}\n\
             Without internet, set MPV_LIB_DIR to a folder with libmpv in it.",
            pinned.url
        )
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into();
    let mut response = agent.get(pinned.url).call().map_err(|e| failed(&e))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(1 << 30)
        .read_to_vec()
        .map_err(|e| failed(&e))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    if sha256 != pinned.sha256 {
        return Err(format!(
            "libmpv from {} isn't the pinned build (SHA-256 {sha256}, expected {})",
            pinned.url, pinned.sha256
        ));
    }
    Ok(bytes)
}

/// The .7z has libmpv-2.dll but no import library, which the MSVC linker
/// needs: that's made from the DLL's exports with Visual Studio's lib.exe.
fn unpack_windows(archive: &[u8], dir: &Path) -> Result<(), String> {
    let all = dir.join("archive");
    sevenz_rust2::decompress(Cursor::new(archive), &all)
        .map_err(|e| format!("unpacking libmpv: {e}"))?;
    let dll = dir.join("libmpv-2.dll");
    fs::rename(all.join("libmpv-2.dll"), &dll)
        .map_err(|e| format!("libmpv-2.dll isn't in the archive: {e}"))?;
    fs::remove_dir_all(&all).map_err(|e| format!("cleaning up: {e}"))?;
    import_library(&dll, &dir.join("mpv.lib"))
}

/// Writes `lib`, the import library for `dll`'s `mpv_*` functions.
fn import_library(dll: &Path, lib: &Path) -> Result<(), String> {
    use object::Object;
    let data = fs::read(dll).map_err(|e| format!("reading {}: {e}", dll.display()))?;
    let file = object::File::parse(&*data).map_err(|e| format!("reading libmpv-2.dll: {e}"))?;
    let exports = file
        .exports()
        .map_err(|e| format!("reading libmpv-2.dll's exports: {e}"))?;
    let mut def = String::from("LIBRARY libmpv-2.dll\nEXPORTS\n");
    for export in exports {
        if let Ok(name) = std::str::from_utf8(export.name())
            && name.starts_with("mpv_")
        {
            def.push_str(name);
            def.push('\n');
        }
    }
    let def_path = lib.with_extension("def");
    fs::write(&def_path, def).map_err(|e| format!("writing {}: {e}", def_path.display()))?;
    let target = std::env::var("TARGET").unwrap_or_else(|_| "x86_64-pc-windows-msvc".into());
    let tool = cc::windows_registry::find_tool(&target, "lib.exe")
        .ok_or("Visual Studio's lib.exe wasn't found (install the C++ build tools)")?;
    let status = tool
        .to_command()
        .arg("/nologo")
        .arg("/machine:x64")
        .arg(format!("/def:{}", def_path.display()))
        .arg(format!("/out:{}", lib.display()))
        .status()
        .map_err(|e| format!("running lib.exe: {e}"))?;
    let _ = fs::remove_file(&def_path);
    let _ = fs::remove_file(lib.with_extension("exp"));
    if !status.success() {
        return Err(format!("lib.exe failed ({status})"));
    }
    Ok(())
}

/// The .tar.gz has one folder of dylibs; some are symlinks to others.
fn unpack_macos(archive: &[u8], dir: &Path) -> Result<(), String> {
    let failed = |e: io::Error| format!("unpacking libmpv: {e}");
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut links = Vec::new();
    for entry in tar.entries().map_err(failed)? {
        let mut entry = entry.map_err(failed)?;
        let path = entry.path().map_err(failed)?.into_owned();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        // Only the dylibs in the top folder.
        if !name.ends_with(".dylib") || path.components().count() != 2 {
            continue;
        }
        match entry.header().entry_type() {
            tar::EntryType::Regular => {
                let mut file = fs::File::create(dir.join(&name)).map_err(failed)?;
                io::copy(&mut entry, &mut file).map_err(failed)?;
            }
            tar::EntryType::Symlink => {
                if let Some(target) = entry.link_name().map_err(failed)? {
                    links.push((name, target.into_owned()));
                }
            }
            _ => {}
        }
    }
    for (name, target) in links {
        let target = dir.join(target.file_name().unwrap_or_default());
        fs::copy(&target, dir.join(&name))
            .map_err(|e| format!("{name} points at {}: {e}", target.display()))?;
    }
    if !dir.join("libmpv.dylib").exists() {
        return Err("libmpv.dylib isn't in the archive".into());
    }
    Ok(())
}
