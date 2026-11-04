//! Refreshes the embedded price snapshot from LiteLLM's price list (`claude-*` entries only).
//!
//! ```sh
//! curl -sSf https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json \
//!   | cargo run -p cv-core --example update_pricing
//! ```

use std::io::Read;

use transcript::pricing::PriceTable;

fn main() {
    let mut json = String::new();
    std::io::stdin()
        .read_to_string(&mut json)
        .expect("read stdin");
    let table = PriceTable::from_litellm(&json).expect("no claude-* prices in the input");
    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/src/pricing-snapshot.json");
    std::fs::write(out, table.to_litellm_json() + "\n").expect("write snapshot");
    eprintln!("{} models → {out}", table.len());
}
