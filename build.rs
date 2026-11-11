use std::{fs, path::Path};
fn digest(path: &Path, hash: &mut u64) {
    if path.is_dir() {
        let mut files: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        for file in files {
            digest(&file, hash);
        }
    } else {
        for byte in path
            .to_string_lossy()
            .bytes()
            .chain(fs::read(path).unwrap())
        {
            *hash = (*hash ^ byte as u64).wrapping_mul(0x100000001b3);
        }
    }
}
fn main() {
    let mut hash = 0xcbf29ce484222325;
    for path in ["src", "web", "Cargo.toml", "Cargo.lock", "build.rs"] {
        println!("cargo:rerun-if-changed={path}");
        digest(Path::new(path), &mut hash);
    }
    println!("cargo:rustc-env=RELAY_BUILD_ID={hash:016x}");
}
