use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct View {
    pub sort: String,
    pub desc: bool,
    #[serde(default)]
    pub filter: String,
}

impl Default for View {
    fn default() -> Self {
        Self {
            sort: "pss".into(),
            desc: true,
            filter: String::new(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join("heft")
}

fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    if let Ok(v) = std::env::var(var)
        && !v.is_empty()
    {
        return PathBuf::from(v);
    }
    home().join(fallback)
}

fn home() -> PathBuf {
    std::env::var("HOME").map_or_else(|_| PathBuf::from("/"), PathBuf::from)
}

pub fn view_path() -> PathBuf {
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
pub fn save_view(view: &View) -> Result<(), Error> {
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
