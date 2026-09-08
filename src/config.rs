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
    serde_json::from_str(&text).unwrap_or_default()
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
    let data = serde_json::to_string_pretty(view)?;
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
        Ok(serde_json::from_str(text)?)
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
fn overrides_path() -> PathBuf {
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
