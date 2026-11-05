//! Fixture locations, shared with the `transcript` crate's tests (they include this file with
//! `#[path]`), so it must not depend on codeck-core.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// `tests/fixtures` of codeck-core; the `..` hop resolves from either crate's manifest dir.
pub const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../codeck-core/tests/fixtures");

/// Session ids and encoded project directory names of the fixtures.
pub mod ids {
    pub const BASIC: &str = "86d20d64-9fdd-5e64-aae4-93895b13068d";
    pub const BASIC_DIR: &str = "-Users-dev-code-lumen-api";
    pub const BRANCH: &str = "6d4df888-616c-528f-b5b9-674beb27f393";
    pub const BRANCH_DIR: &str = "-Users-dev-code-orbit-web";
    /// compact: boundary whose logicalParentUuid exists and has the `/compact` prompt as a sibling
    pub const COMPACT_SIBLING: &str = "699c395a-2b04-59eb-bb3a-eb0e0847717b";
    /// compact: boundary whose logicalParentUuid is missing from the file
    pub const COMPACT_MISSING: &str = "d131117c-dd2d-5403-9be8-9b5c492229a9";
    /// compact: boundary whose logicalParentUuid points at a LATER entry (parent cycle)
    pub const COMPACT_CYCLE: &str = "6a5f53ed-a977-58c4-a74d-a5e9d4877d22";
    pub const COMPACT_DIR: &str = "-Users-dev-code-tidepool";
    pub const FORK_ORIGIN: &str = "d3b58148-dbd3-54d8-a4e3-f272df8eac0d";
    pub const FORK_CHILD: &str = "d6ac0c3a-1415-5f66-bbd9-6cc17c0eb4f5";
    pub const FORK_DIR: &str = "-Users-dev-code-tidepool";
    /// copies: byte-identical in both project dirs
    pub const COPIES_IDENTICAL: &str = "2000072b-1616-53ff-b8dd-0b509f44339e";
    /// copies: `-old` holds a byte prefix (2 turns), the main dir the superset (4 turns)
    pub const COPIES_SUPERSET: &str = "d7b7f7c0-01f9-5dc2-b356-815f6c8d6cfa";
    pub const COPIES_DIR: &str = "-Users-dev-code-kestrel-cli";
    pub const COPIES_DIR_OLD: &str = "-Users-dev-code-kestrel-cli-old";
    /// name of the symlinked project dir `fixture_root("copies")` adds (points at `COPIES_DIR`)
    pub const COPIES_LINK: &str = "-Users-dev-code-kestrel-cli-link";
    pub const SUBAGENTS: &str = "ee98ea57-9b48-56b0-9860-5c8105c7f967";
    pub const SUBAGENTS_DIR: &str = "-Users-dev-code-tidepool";
    pub const WORKFLOW: &str = "418be169-3e5e-5397-b9f9-862014ebf2fa";
    pub const WORKFLOW_DIR: &str = "-Users-dev-code-kestrel-cli";
    pub const PERSISTED: &str = "3e7603aa-503a-511b-8228-4e9155f1d5e4";
    pub const PERSISTED_DIR: &str = "-Users-dev-code-lumen-api";
    /// live: transcripts exist for these two; the other two sessions json files have none
    pub const LIVE_BUSY: &str = "b5805323-fcce-58f1-bcf9-40e720af88f4";
    pub const LIVE_SHELL: &str = "45470335-35d2-5185-ad2c-5f6ca8463a06";
    pub const LIVE_DEAD: &str = "6c8546d4-6e10-5c8d-9cc3-b6e13f66eef1";
    pub const LIVE_REUSED: &str = "1fc71c8b-e8ea-52c0-83bd-ad683944a26c";
    pub const LIVE_DIR: &str = "-Users-dev-code-orbit-web";
}

/// Path of a scenario inside the repo (read-only; never write here).
pub fn fixture_dir(name: &str) -> PathBuf {
    Path::new(FIXTURES).join(name)
}
