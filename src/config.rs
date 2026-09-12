use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::{Error, Folder};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct View {
    pub sort: String,
    pub desc: bool,
    #[serde(default)]
    pub filter: String,
    /// Column labels left out of the table. A hide list rather than a show
    /// list: heft grows columns, and a show list would silently withhold every
    /// column added after the file was written.
    #[serde(default)]
    pub hide_columns: Vec<String>,
    /// Column labels left to right. Listed columns come first, in this order;
    /// anything not listed keeps its default place after them. `name` stays
    /// first unless this list names it. Empty is the compiled table order.
    #[serde(default)]
    pub column_order: Vec<String>,
    /// uids from `--user`. Deliberately not serialized: sort, filter, hidden
    /// columns and column order are preferences a `s` press should outlive the
    /// session, but a saved user cut would hide most of the machine on every
    /// later run for a reason the file, not the command line, was keeping. It
    /// rides in `View` only because every surface already takes one.
    #[serde(skip)]
    pub users: Vec<u32>,
    /// `--top`, and not serialized for the same reason `users` is not: a saved
    /// row limit would quietly hide most of the machine on every later run.
    #[serde(skip)]
    pub top: Option<usize>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            sort: "pss".into(),
            desc: true,
            filter: String::new(),
            hide_columns: Vec::new(),
            column_order: Vec::new(),
            users: Vec::new(),
            top: None,
        }
    }
}

/// Blank out `//` and `/* */` comments so the config files can carry them.
///
/// Hand-rolled rather than a JSON5 or JSONC crate for the same reason the
/// `/proc` parsers and the base64 encoder are: it is a scanner over one string
/// and the alternative is a dependency tree for a file read once at startup.
///
/// Comment bytes become spaces and newlines inside a block comment are kept,
/// so a serde error still names the line and column the reader is looking at.
/// A `//` inside a string stays put -- a saved filter is a regex, and
/// `"https?://"` is a pattern rather than the start of a comment.
fn strip_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let mut in_string = false;
    while i < b.len() {
        if in_string {
            // A backslash escapes the next byte, so an escaped quote does not
            // end the string and `"\\"` does not swallow the one after it.
            if b[i] == b'\\' && i + 1 < b.len() {
                out.push(b[i]);
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if b[i] == b'"' {
                in_string = false;
            }
            out.push(b[i]);
            i += 1;
            continue;
        }
        match (b[i], b.get(i + 1)) {
            (b'"', _) => {
                in_string = true;
                out.push(b[i]);
                i += 1;
            }
            (b'/', Some(b'/')) => {
                while i < b.len() && b[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            (b'/', Some(b'*')) => {
                out.push(b' ');
                out.push(b' ');
                i += 2;
                while i < b.len() {
                    if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        out.push(b' ');
                        out.push(b' ');
                        i += 2;
                        break;
                    }
                    // Newlines survive, so the line numbers a parse error
                    // reports are the ones in the file the user is editing.
                    out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
                // An unterminated block comment runs to the end of the file,
                // which is what every other reader does with one.
            }
            _ => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    // Every byte written was either copied from valid UTF-8 or is ASCII.
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// The header `s` writes above the saved view. heft regenerates it on every
/// save, so the file explains itself rather than sending the reader to the man
/// page -- and the label list is generated, so it cannot drift from the
/// columns the binary actually has.
///
/// A save rewrites the whole file, so comments of your own elsewhere in it do
/// not survive one. That is the trade for `s` staying a single atomic write.
fn view_header() -> String {
    let labels = crate::once::column_labels().join(" ");
    format!(
        "// heft view -- sort, direction, filter, hidden columns, column order.\n\
         // Written by `s` in the TUI, and rewritten whole by the next `s`.\n\
         // `//` and `/* */` comments are allowed here and in grouping.json.\n\
         //\n\
         // sort:          one of the labels below\n\
         // desc:          true for high to low\n\
         // filter:        a regex, case-insensitive unless it says (?-i)\n\
         // hide_columns:  labels to leave out; `name` cannot be hidden\n\
         // column_order:  labels left to right; the rest keep their place\n\
         //\n\
         // labels: {labels}\n"
    )
}

fn config_dir() -> PathBuf {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => std::env::var("HOME")
            .map_or_else(|_| PathBuf::from("/"), PathBuf::from)
            .join(".config"),
    };
    base.join("heft")
}

pub(crate) fn view_path() -> PathBuf {
    config_dir().join("view.json")
}

pub fn load_view() -> View {
    let path = view_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return View::default();
    };
    serde_json::from_str(&strip_comments(&text)).unwrap_or_default()
}

/// # Errors
///
/// Returns an error if the XDG config directory cannot be created or
/// permissioned, the view cannot be serialized, or the file cannot be written.
pub(crate) fn save_view(view: &View) -> Result<(), Error> {
    let dir = config_dir();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let path = view_path();
    let data = format!("{}{}\n", view_header(), serde_json::to_string_pretty(view)?);
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(data.as_bytes())?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Grouping overrides, read from `grouping.json` beside the saved view.
///
/// heft never writes this file and never creates the directory for it; the
/// only config write is still an explicit view save.
///
/// Every field names a decision the grouping code already makes, so an
/// override replaces a verdict rather than adding a grouping concept. Keys are
/// the identities the tree shows — the row title, not a pid or a comm — so a
/// user names what they can see.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    /// Identities forced into Applications.
    #[serde(default)]
    pub applications: HashSet<String>,
    /// Identities forced into User Services.
    #[serde(default)]
    pub user_services: HashSet<String>,
    /// Identity -> the identity it bills to instead.
    #[serde(default)]
    pub fold: HashMap<String, String>,
    /// Container name -> owning uid.
    #[serde(default)]
    pub container_owners: HashMap<String, u32>,
}

impl Overrides {
    /// # Errors
    ///
    /// Returns an error if the text is not an object of the documented keys.
    pub fn parse(text: &str) -> Result<Self, Error> {
        Ok(serde_json::from_str(&strip_comments(text))?)
    }

    /// The folder this identity is pinned to, if the user pinned it.
    ///
    /// Applications wins an identity listed in both: an entry in the user's own
    /// application list is the reading that keeps the row where they look.
    pub(crate) fn folder_for(&self, ident: &str) -> Option<Folder> {
        if self.applications.contains(ident) {
            Some(Folder::Applications)
        } else if self.user_services.contains(ident) {
            Some(Folder::UserServices)
        } else {
            None
        }
    }

    /// The identity this one bills to instead, if the user redirected it.
    pub(crate) fn fold_key(&self, ident: &str) -> Option<&str> {
        self.fold.get(ident).map(String::as_str)
    }

    /// True when no override can move a row between folders, so the
    /// per-process path skips the lookups on the common no-config run.
    /// `container_owners` is deliberately not counted: it is consulted in
    /// `containers::insert_resolved`, and an override can never move a
    /// container row anyway.
    pub(crate) fn no_placement_overrides(&self) -> bool {
        self.applications.is_empty() && self.user_services.is_empty() && self.fold.is_empty()
    }
    pub(crate) fn container_owner(&self, name: &str) -> Option<u32> {
        self.container_owners.get(name).copied()
    }
}
pub(crate) fn overrides_path() -> PathBuf {
    config_dir().join("grouping.json")
}

/// Read `grouping.json`, or the built-in behaviour when it is absent or bad.
///
/// A monitor that dies on a typo in a config file is worse than one with no
/// config at all, so a parse failure warns once and grouping continues.
pub(crate) fn load_overrides() -> Overrides {
    let path = overrides_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Overrides::default();
    };
    Overrides::parse(&text).unwrap_or_else(|e| {
        eprintln!("heft: ignoring {}: {e}", path.display());
        Overrides::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_blanked_without_moving_anything_else() {
        let text = "{\n  // the column to sort by\n  \"sort\": \"pss\",\n  \"desc\": true\n}";
        let out = strip_comments(text);
        assert_eq!(out.len(), text.len(), "byte offsets are preserved");
        assert_eq!(out.lines().count(), text.lines().count());
        let v: View = serde_json::from_str(&out).unwrap();
        assert_eq!(v.sort, "pss");
        assert!(v.desc);
    }

    #[test]
    fn a_block_comment_keeps_its_newlines() {
        let text = "{\n/* two\n   lines */\n\"sort\": \"rss\", \"desc\": false\n}";
        let out = strip_comments(text);
        assert_eq!(
            out.lines().count(),
            text.lines().count(),
            "line numbers hold"
        );
        assert!(!out.contains("two"));
        assert_eq!(serde_json::from_str::<View>(&out).unwrap().sort, "rss");
    }

    /// A saved filter is a regex, so `//` inside a string is a pattern and not
    /// the start of a comment.
    #[test]
    fn a_comment_marker_inside_a_string_survives() {
        let text = r#"{"sort":"pss","desc":true,"filter":"https?://host /* keep */"}"#;
        let v: View = serde_json::from_str(&strip_comments(text)).unwrap();
        assert_eq!(v.filter, "https?://host /* keep */");
        // An escaped quote does not end the string, so the comment marker
        // after it is still inside one.
        let text = r#"{"sort":"pss","desc":true,"filter":"say \" then //x"}"#;
        let v: View = serde_json::from_str(&strip_comments(text)).unwrap();
        assert_eq!(v.filter, r#"say " then //x"#);
    }

    #[test]
    fn overrides_take_comments_too() {
        let o = Overrides::parse("{\n // mine\n \"user_services\": [\"mydaemon\"] /* pinned */\n}")
            .unwrap();
        assert!(o.user_services.contains("mydaemon"));
    }

    /// What `s` writes must be what `load_view` reads back, header and all.
    #[test]
    fn a_written_view_round_trips_through_its_own_header() {
        let view = View {
            sort: "core".into(),
            filter: "a//b".into(),
            hide_columns: vec!["vram".into()],
            ..Default::default()
        };
        let text = format!(
            "{}{}\n",
            view_header(),
            serde_json::to_string_pretty(&view).unwrap()
        );
        assert!(text.starts_with("// heft view"));
        // The label list is generated, so it cannot drift from the binary.
        assert!(text.contains("labels: name spark"), "{text}");
        let back: View = serde_json::from_str(&strip_comments(&text)).unwrap();
        assert_eq!(back.sort, "core");
        assert_eq!(back.filter, "a//b");
        assert_eq!(back.hide_columns, ["vram"]);
    }

    #[test]
    fn an_unterminated_block_comment_does_not_panic() {
        assert!(serde_json::from_str::<View>(&strip_comments("{ /* oops")).is_err());
        assert_eq!(strip_comments(""), "");
    }
}
