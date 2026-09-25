// The built-in manifest is optional at build time (it is gitignored: it holds the server
// address); without it the app relies on the other manifest channels.
//
// The app version comes from pubspec.yaml (`version: 0.1.4+4` → APP_VERSION, APP_BUILD), so
// the update check always compares against what was actually built.
fn main() {
    println!("cargo:rerun-if-changed=../assets/manifest.bin");
    let src = std::path::Path::new("../assets/manifest.bin");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("manifest.bin");
    std::fs::write(out, std::fs::read(src).unwrap_or_default()).unwrap();

    println!("cargo:rerun-if-changed=../pubspec.yaml");
    let pubspec = std::fs::read_to_string("../pubspec.yaml").expect("pubspec.yaml");
    let line = pubspec
        .lines()
        .find_map(|l| l.strip_prefix("version:"))
        .expect("pubspec.yaml: version")
        .trim();
    let (version, build) = line.split_once('+').expect("pubspec version must be x.y.z+build");
    println!("cargo:rustc-env=APP_VERSION={version}");
    println!("cargo:rustc-env=APP_BUILD={build}");
}
