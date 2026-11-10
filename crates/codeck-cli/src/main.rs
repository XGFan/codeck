fn main() {
    std::process::exit(codeck_cli::run(std::env::args().skip(1).collect()));
}
