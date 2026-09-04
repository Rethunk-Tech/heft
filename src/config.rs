use std::fs;
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
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
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

pub fn save_view(view: &View) -> Result<(), Error> {
    fs::create_dir_all(config_dir())?;
    fs::write(view_path(), serde_json::to_string_pretty(view)?)?;
    Ok(())
}
