use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::CommandFactory;
use clap_complete::{Shell, generate_to};

// The one definition of the flags, shared with src/main.rs so the generated
// completions and man page cannot drift from what the binary accepts.
include!("src/cli.rs");
// The TUI keys, so the KEYS section below and the `?` overlay are one list.
include!("src/keys.rs");

/// Sections clap cannot generate, because none of this is a flag: the TUI key
/// bindings, the files heft reads, and the environment it consults. heft's
/// default mode is the TUI, so a man page of flags alone documented every mode
/// except the one someone typing `heft` actually gets.
fn extra_sections() -> String {
    let mut s = String::from(
        ".SH KEYS\nThe fullscreen TUI, which is what \\fBheft\\fR runs with no mode flag.\n",
    );
    for (k, desc) in KEYS {
        let k = k
            .replace("{up}", "Up")
            .replace("{down}", "Down")
            .replace("{left}", "Left")
            .replace("{right}", "Right")
            .replace('-', "\\-");
        let _ = write!(s, ".TP\n\\fB{k}\\fR\n{desc}\n");
    }
    s.push_str(
        ".SH FILES
.TP
\\fB$XDG_CONFIG_HOME/heft/view.json\\fR
Sort column, direction, filter, hidden columns and column order, written by
the \\fBs\\fR key. Defaults to ~/.config/heft/view.json. Never written except
by \\fBs\\fR.
.TP
\\fB$XDG_CONFIG_HOME/heft/rules.d/\\fR, \\fB/etc/heft/rules.d/\\fR
Grouping rule files, read in that order ahead of the built\\-in set. Read only,
never created. A malformed file warns once and the rest still load.
.SH ENVIRONMENT
.TP
\\fBNO_COLOR\\fR
Any non\\-empty value draws the TUI without hue. Presence decides, not the
value, so NO_COLOR=0 disables colour too.
.TP
\\fBDOCKER_HOST\\fR
A unix:// socket is tried before /var/run/docker.sock and the Podman sockets.
.TP
\\fBXDG_CONFIG_HOME\\fR
Where view.json and the user rules.d live. Defaults to ~/.config.
.TP
\\fBLC_ALL\\fR, \\fBLC_CTYPE\\fR, \\fBLANG\\fR
Read in that order by \\fB\\-\\-glyphs auto\\fR: block characters only when one
of them names a UTF\\-8 charmap, ASCII substitutes otherwise.
",
    );
    s
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// `0.7.0~1a2b3c4`, so two builds of main can be told apart in a bug report.
///
/// Git is asked only when this crate is the top of its own checkout: an AUR
/// `heft` build extracts the tag tarball inside the AUR package's clone, and
/// `rev-parse` there answers with that repository's commit. A tarball has no
/// `.git` of its own, so `.git-sha` carries the commit through `export-subst`;
/// unsubstituted it reads `$Format:%H$`, fails the hex check, and the version
/// is the bare release number.
fn version() -> String {
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let own = git(&["rev-parse", "--show-toplevel"])
        .is_some_and(|t| Path::new(&t).canonicalize().ok() == Path::new(&root).canonicalize().ok());
    let sha = if own {
        // The reflog is appended on every commit, checkout and reset, so it
        // is the one file that moves whenever HEAD does.
        for p in ["logs/HEAD", "HEAD"] {
            if let Some(p) = git(&["rev-parse", "--git-path", p]).filter(|p| Path::new(p).exists())
            {
                println!("cargo::rerun-if-changed={p}");
                break;
            }
        }
        git(&["rev-parse", "HEAD"])
    } else {
        println!("cargo::rerun-if-changed=.git-sha");
        std::fs::read_to_string(".git-sha").ok()
    };
    let pkg = env!("CARGO_PKG_VERSION");
    match sha.as_deref().map(str::trim) {
        Some(s) if s.len() >= 7 && s.bytes().all(|b| b.is_ascii_hexdigit()) => {
            format!("{pkg}~{}", &s[..7])
        }
        _ => pkg.to_owned(),
    }
}

/// `OUT_DIR/builtin_rules.rs`: every `rules.d/*.json`, sorted bytewise, as a
/// `(name, include_str!)` table. Generated from `read_dir` rather than a
/// hand-kept list so a file added to the directory cannot be left out of the
/// binary, and sorted because `read_dir` order is machine-dependent.
fn embed_rules(out: &Path) -> std::io::Result<()> {
    println!("cargo::rerun-if-changed=rules.d");
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dir = root.join("rules.d");
    let mut names: Vec<String> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut s = String::from("pub static BUILTIN: &[(&str, &str)] = &[\n");
    for n in names {
        let _ = writeln!(
            s,
            "    ({n:?}, include_str!({:?})),",
            dir.join(&n).display()
        );
    }
    s.push_str("];\n");
    std::fs::write(out.join("builtin_rules.rs"), s)
}

fn main() -> std::io::Result<()> {
    println!("cargo::rerun-if-changed=src/cli.rs");
    println!("cargo::rerun-if-changed=src/keys.rs");
    embed_rules(&PathBuf::from(
        std::env::var_os("OUT_DIR").expect("OUT_DIR"),
    ))?;

    let version = version();
    println!("cargo::rustc-env=HEFT_VERSION={version}");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("assets");
    std::fs::create_dir_all(&out)?;

    let mut cmd = Cli::command().version(&*version.leak());
    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
        generate_to(shell, &mut cmd, "heft", &out)?;
    }
    // Rendered piecewise rather than through `generate_to`, so KEYS, FILES and
    // ENVIRONMENT land after OPTIONS and before VERSION, where a reader looks
    // for them.
    let man = clap_mangen::Man::new(cmd);
    let mut page: Vec<u8> = Vec::new();
    man.render_title(&mut page)?;
    man.render_name_section(&mut page)?;
    man.render_synopsis_section(&mut page)?;
    man.render_description_section(&mut page)?;
    man.render_options_section(&mut page)?;
    page.extend_from_slice(extra_sections().as_bytes());
    man.render_version_section(&mut page)?;
    std::fs::write(out.join("heft.1"), page)?;
    Ok(())
}
