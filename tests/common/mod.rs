//! Helpers shared by the integration tests that drive the built binary.
//!
//! `grouping.rs` links the library instead, so it needs none of this.

use std::process::Command;

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_heft");

/// Point the child at a config directory that does not exist, so a developer's
/// saved view or grouping overrides cannot reshape the tree under test.
pub fn heft(args: &[&str]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(args).env(
        "XDG_CONFIG_HOME",
        std::env::temp_dir().join("heft-no-config"),
    );
    cmd
}

/// A JSON array field, or an empty slice where the key is absent: a folder
/// with nothing in it is left out of the tree rather than emitted empty.
pub fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}
