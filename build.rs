use std::fmt::Write;
use std::path::PathBuf;

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
\\fB$XDG_CONFIG_HOME/heft/grouping.json\\fR
Optional grouping overrides. Read only, never created. A malformed file warns
once and grouping continues with the built\\-in tables.
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
Where the two files above live. Defaults to ~/.config.
.TP
\\fBLC_ALL\\fR, \\fBLC_CTYPE\\fR, \\fBLANG\\fR
Read in that order by \\fB\\-\\-glyphs auto\\fR: block characters only when one
of them names a UTF\\-8 charmap, ASCII substitutes otherwise.
",
    );
    s
}

fn main() -> std::io::Result<()> {
    println!("cargo::rerun-if-changed=src/cli.rs");
    println!("cargo::rerun-if-changed=src/keys.rs");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("assets");
    std::fs::create_dir_all(&out)?;

    let mut cmd = Cli::command();
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
