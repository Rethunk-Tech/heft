//! Helpers shared by the integration tests that drive the built binary.
//!
//! `grouping.rs` links the library instead, so it needs none of this.

use std::process::Command;

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_heft");

/// Point the child at a config directory and a rules path that do not exist,
/// so a developer's saved view or rules.d files, XDG or `/etc`, cannot reshape
/// the tree under test.
pub fn heft(args: &[&str]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env(
            "XDG_CONFIG_HOME",
            std::env::temp_dir().join("heft-no-config"),
        )
        .env(
            "HEFT_RULES_PATH",
            std::env::temp_dir().join("heft-no-rules"),
        );
    cmd
}

/// Every pid `/proc` lists, read straight from the directory rather than
/// through heft: a suite checking heft against `/proc` must not ask heft.
pub fn pids() -> Vec<u64> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse().ok()))
        .collect()
}

/// A JSON array field, or an empty slice where the key is absent: a folder
/// with nothing in it is left out of the tree rather than emitted empty.
pub fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// Every identity row in the tree, with the folder path that leads to it.
///
/// A project row's `containers[]` members are not walked: they republish
/// processes their project row already bills, so a walk over them would count
/// those pids twice.
pub fn idents(host: &Value) -> Vec<(String, &Value)> {
    let mut out = Vec::new();
    for user in arr(host, "users") {
        let uid = user["uid"].as_u64().expect("a user node carries its uid");
        for folder in ["applications", "user_services", "containers"] {
            for ident in arr(user, folder) {
                out.push((format!("uid{uid}/{folder}"), ident));
            }
        }
    }
    for folder in ["containers", "system"] {
        for ident in arr(host, folder) {
            out.push((format!("host/{folder}"), ident));
        }
    }
    out
}

/// Flatten one node's process forest. Depth is bounded by the real ancestry
/// heft copied out of `/proc`, so recursion cannot outrun the stack.
pub fn procs_of(parent: &Value) -> Vec<&Value> {
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
        out.push(node);
        for child in arr(node, "children") {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    for root in arr(parent, "processes") {
        walk(root, &mut out);
    }
    out
}
