//! Embeds the built-in layouts (`layouts/*.json`) into the binary.

use std::fmt::Write;

fn main() {
    // Read when the script runs, not baked in when it was compiled: a moved checkout reuses the
    // compiled script, and the old path would embed no layouts at all.
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("layouts");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut out = String::from("&[\n");
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let name = f.file_name().unwrap().to_string_lossy();
        writeln!(out, "    ({name:?}, include_str!({:?})),", f.display().to_string()).unwrap();
    }
    out.push(']');
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("builtin_layouts.rs");
    std::fs::write(dest, out).unwrap();
}
