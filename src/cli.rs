use clap::{Parser, ValueEnum};

/// Which characters the bars, rules and markers use. Defined here rather than
/// beside the glyph table because `build.rs` compiles this file standalone to
/// generate the completions and man page.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Glyphs {
    /// Unicode when the locale names a UTF-8 charmap, ASCII otherwise
    Auto,
    /// Block and box-drawing characters
    Unicode,
    /// Block bars, but an ASCII TREND ramp: a font without the eighth blocks
    Legacy,
    /// One-column ASCII substitutes, for a console without the fonts
    Ascii,
}

/// How the TREND column is drawn.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Trend {
    /// Ask the terminal, and draw an image if it answers that it can
    Auto,
    /// Rising block characters, in whatever `--glyphs` resolved
    Chars,
    /// A kitty-graphics-protocol image: needs kitty or ghostty
    Kitty,
    /// A sixel image: needs xterm, foot, wezterm, konsole, iTerm2 or Windows Terminal
    Sixel,
}

#[derive(Parser)]
#[command(
    name = "heft",
    // `build.rs` sets HEFT_VERSION for the crate but compiles this file
    // before it has, so it falls back here and sets the man page's itself.
    version = option_env!("HEFT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    about = "Read-only Linux application-weight process monitor"
)]
pub(crate) struct Cli {
    /// Print one table and exit
    #[arg(long)]
    pub once: bool,
    /// Print one JSON document and exit
    #[arg(long)]
    pub json: bool,
    /// Seconds between catch-all samples (TUI tick and `--once` / `--json` gap)
    #[arg(long, default_value_t = 1.0)]
    pub interval: f64,
    /// TUI seconds between PSS (`smaps_rollup`) reads (≥ `--interval`). `--once` / `--json` always read PSS
    #[arg(long, default_value_t = 5.0)]
    pub pss_interval: f64,
    /// Sort by this column label: the labels Shift-arrows cycle and `view.json` saves; a bad one lists them all
    #[arg(long, value_name = "COLUMN")]
    pub sort: Option<String>,
    /// Keep only rows whose name matches this regex, and their parents
    #[arg(long, value_name = "REGEX", conflicts_with = "json")]
    pub filter: Option<String>,
    /// Show only this user's branch; repeat for several. Takes a name or a uid
    #[arg(long = "user", value_name = "NAME|UID")]
    pub user: Vec<String>,
    /// Characters for bars, rules and markers
    #[arg(long, value_enum, default_value_t = Glyphs::Auto, value_name = "SET")]
    pub glyphs: Glyphs,
    /// Say where this pid landed in the tree and the grouping.json key that moves it
    #[arg(long, value_name = "PID", conflicts_with_all = ["once", "json", "follow"])]
    pub explain: Option<u32>,
    /// How TREND is drawn. `auto` asks the terminal and draws an image if it can
    #[arg(long, value_enum, default_value_t = Trend::Auto, value_name = "MODE", conflicts_with_all = ["once", "json"])]
    pub trend: Trend,
    /// Keep sampling: one table or one JSON line per interval. Needs --once or --json
    #[arg(long)]
    pub follow: bool,
    /// Keep only the N heaviest rows in each list the sort ordered
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..), conflicts_with = "json")]
    pub top: Option<u32>,
    /// Sort high to low, whatever the saved view holds
    #[arg(long, conflicts_with = "asc")]
    pub desc: bool,
    /// Sort low to high, whatever the saved view holds
    #[arg(long)]
    pub asc: bool,
    /// Leave this column out of the table; repeat. The labels Shift-arrows cycle. Refused with --json
    #[arg(long, value_name = "COLUMN", conflicts_with = "json")]
    pub hide: Vec<String>,
    /// Left-to-right column order; repeat. Unlisted keep default order after these. name stays first unless listed. Refused with --json
    #[arg(long, value_name = "COLUMN", conflicts_with = "json")]
    pub order: Vec<String>,
    /// Read /proc and /sys under this directory instead of /: another mount namespace, or a captured tree
    #[arg(long, value_name = "DIR")]
    pub proc_root: Option<String>,
}
