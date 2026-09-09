/// The TUI key bindings.
///
/// Their own file because two things need them and neither can reach the
/// other: `ui` is in the library, `build.rs` compiles standalone files to
/// generate the man page, and `cli.rs` is the binary's. One definition, so
/// `man heft`'s KEYS section and the `?` overlay cannot drift apart the way
/// they did when a key was added to only one of them.
///
/// `{up}` `{down}` `{left}` `{right}` are placeholders. The TUI fills them
/// with whatever `glyph` resolved for the terminal; the man page spells the
/// arrows out, since roff has no locale to consult.
pub(crate) const KEYS: &[(&str, &str)] = &[
    ("q  Esc  Ctrl-C", "quit"),
    ("{up} {down}  j k", "move"),
    ("PgUp PgDn  Home End", "page or jump"),
    ("{left} {right}  h l", "expand / collapse"),
    ("Enter  Space", "expand / collapse"),
    ("/", "filter by regex (Enter apply, Esc cancel)"),
    ("c", "cycle sort column"),
    ("d", "reverse sort"),
    ("H", "hide sort column"),
    ("u", "unhide last column"),
    ("i", "detail for the selected row"),
    ("p", "pause (the footer says how stale the view is)"),
    ("s", "save view"),
    ("[ ]  < >", "scroll columns"),
    ("?  F1", "toggle this help"),
];
