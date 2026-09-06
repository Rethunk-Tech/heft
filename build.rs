use std::path::PathBuf;

use clap::CommandFactory;
use clap_complete::{Shell, generate_to};

// The one definition of the flags, shared with src/main.rs so the generated
// completions and man page cannot drift from what the binary accepts.
include!("src/cli.rs");

fn main() -> std::io::Result<()> {
    println!("cargo::rerun-if-changed=src/cli.rs");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("assets");
    std::fs::create_dir_all(&out)?;

    let mut cmd = Cli::command();
    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
        generate_to(shell, &mut cmd, "heft", &out)?;
    }
    clap_mangen::Man::new(cmd).generate_to(&out)?;
    Ok(())
}
