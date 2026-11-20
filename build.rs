use std::{env, fs, path::Path, path::PathBuf};

#[path = "build/libmpv.rs"]
mod libmpv;

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=build/libmpv.rs");
    let mpv = |pinned| {
        libmpv::find_or_fetch(pinned, &root.join("third_party/mpv"))
            .unwrap_or_else(|e| panic!("{e}"))
    };

    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => windows(&root, &mpv(&libmpv::WINDOWS), &out),
        Ok("macos") => macos(&mpv(&libmpv::MACOS)),
        _ => {}
    }
}

fn windows(root: &Path, mpv: &Path, out: &Path) {
    let dll = mpv.join("libmpv-2.dll");
    println!("cargo:rerun-if-changed={}", dll.display());
    assert!(dll.exists(), "libmpv-2.dll isn't in {}", mpv.display());
    println!("cargo:rustc-link-search=native={}", mpv.display());

    // App icon and version info (assets/app.ico is drawn by scripts/make_icon.py).
    // The version comes from Cargo.toml, which release builds take from the tag.
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let part = |name: &str| env::var(format!("CARGO_PKG_VERSION_{name}")).unwrap();
    let numbers = format!("{},{},{},0", part("MAJOR"), part("MINOR"), part("PATCH"));
    let icon = root
        .join("assets/app.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let rc = fs::read_to_string(root.join("assets/app.rc"))
        .unwrap()
        .replace("@ICON@", &icon)
        .replace("@VERSION_NUMBERS@", &numbers)
        .replace("@VERSION@", &version);
    let generated = out.join("app.rc");
    fs::write(&generated, rc).unwrap();
    embed_resource::compile(&generated, embed_resource::NONE)
        .manifest_optional()
        .expect("compiling Windows resources");

    // Cargo puts a link-search path inside OUT_DIR on PATH when it runs
    // tests and `cargo run`; the DLL has to be there, as newer rustc keeps
    // test binaries under OUT_DIR, not next to the profile dir.
    fs::copy(&dll, out.join("libmpv-2.dll")).unwrap();
    println!("cargo:rustc-link-search=native={}", out.display());

    // OUT_DIR is target/<profile>/build/<crate>/out.
    let profile_dir = out.ancestors().nth(3).unwrap();
    for dir in [profile_dir.to_path_buf(), profile_dir.join("examples")] {
        fs::create_dir_all(&dir).unwrap();
        fs::copy(&dll, dir.join("libmpv-2.dll")).unwrap();
    }
}

fn macos(mpv: &Path) {
    let dylib = mpv.join("libmpv.dylib");
    println!("cargo:rerun-if-changed={}", dylib.display());
    assert!(dylib.exists(), "libmpv.dylib isn't in {}", mpv.display());
    println!("cargo:rustc-link-search=native={}", mpv.display());
    // The app bundle keeps the dylibs in Contents/Frameworks; development
    // builds and tests load them from third_party/mpv. The bundle script
    // removes that second path.
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", mpv.display());
}
