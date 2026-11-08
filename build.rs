use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let mpv = root.join("third_party/mpv");
    let dll = mpv.join("libmpv-2.dll");
    println!("cargo:rerun-if-changed={}", dll.display());
    assert!(
        dll.exists(),
        "libmpv is missing: run `bash scripts/fetch-libmpv.sh` first"
    );
    println!("cargo:rustc-link-search=native={}", mpv.display());

    // App icon and version info (assets/app.ico is drawn by scripts/make_icon.py).
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    embed_resource::compile("assets/app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("compiling Windows resources");

    // OUT_DIR is target/<profile>/build/<crate>/out.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let profile_dir = out.ancestors().nth(3).unwrap();
    for dir in [profile_dir.to_path_buf(), profile_dir.join("examples")] {
        fs::create_dir_all(&dir).unwrap();
        fs::copy(&dll, dir.join("libmpv-2.dll")).unwrap();
    }
}
