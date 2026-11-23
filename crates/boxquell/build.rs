//! The web UI is embedded from `ui/dist`. A checkout without a built UI still
//! has to compile, so make sure the directory exists.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed={}", dist.display());
}
