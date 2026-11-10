fn main() {
    // tauri-build requires the bundled resources to exist, also in a plain `cargo build`; only `pnpm build:web` fills it.
    std::fs::create_dir_all("../dist-web").expect("cannot create dist-web");
    tauri_build::build()
}
