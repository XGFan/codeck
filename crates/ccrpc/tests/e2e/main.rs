//! End to end against a real interactive Claude Code in an isolated ToH clay Pane.
//!
//! `CCRPC_E2E_API_KEY=… cargo test --test e2e -- --ignored --nocapture --test-threads=1`
//! Optional: `CCRPC_E2E_BASE_URL` (default DeepSeek's Anthropic endpoint), `CCRPC_E2E_MODEL`
//! (default `deepseek-flash`), `CCRPC_E2E_TOH` (the `toh` binary; default the sibling checkout's
//! `../toh/target/debug/toh` (next to the repository) when built, else `toh` on PATH — its Holders run that build).
//! Spends model credits. Needs `claude` on PATH.
//!
//! Isolation: a fresh `TOH_STATE_DIR` under /tmp (unix socket paths must stay short) with clay
//! enabled, loopback-only and mDNS off; a fresh `CLAUDE_CONFIG_DIR` pre-seeded to skip onboarding
//! and folder trust, in manual permission mode; a fresh `CCRPC_STATE_DIR` under /tmp. The ccrpc
//! Plugin is loaded with `claude --plugin-dir` (generated for this build), never installed.
//! Cleanup kills only the recorded Holder pid.

mod harness;
mod mock_model;
mod real;
