// The built-in manifest is optional at build time (it is gitignored: it holds the server
// address); without it the app relies on the other manifest channels.
fn main() {
    println!("cargo:rerun-if-changed=../assets/manifest.bin");
    let src = std::path::Path::new("../assets/manifest.bin");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("manifest.bin");
    std::fs::write(out, std::fs::read(src).unwrap_or_default()).unwrap();
}
