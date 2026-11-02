fn main() {
    if let Err(e) = ccrpc::cli::run(std::env::args().skip(1).collect()) {
        eprintln!("ccrpc: {e:#}");
        std::process::exit(1);
    }
}
