use crate::types::Process;

const LAUNCHERS: &[&str] = &[
    "bwrap",
    "flatpak",
    "flatpak-run",
    "zypak-helper",
    "snap-confine",
    "bunx",
    "npx",
    "AppRun",
    "apprun",
    "firejail",
    "xdg-dbus-proxy",
    "zypak-sandbox",
    "startvesktop",
];

/// zypak-helper subcommand, not a payload.
const HINT_SKIP: &[&str] = &["child"];

const GENERICS: &[&str] = &[
    "bun",
    "python",
    "python2",
    "python3",
    "java",
    "node",
    "nodejs",
    "npm",
    "MainThread",
    "perl",
    "ruby",
    "php",
];

const SHELLS: &[&str] = &[
    "bash", "zsh", "fish", "sh", "dash", "ksh", "nu", "tcsh", "csh",
];

const TERMINALS: &[&str] = &[
    "ghostty",
    "gnome-terminal",
    "gnome-terminal-server",
    "kgx",
    "ptyxis",
    "kitty",
    "wezterm",
    "wezterm-gui",
    "alacritty",
    "foot",
    "footclient",
    "konsole",
    "xfce4-terminal",
    "tilix",
    "terminator",
];

const COMPOSITORS: &[&str] = &[
    "gnome-shell",
    "mutter",
    "kwin_wayland",
    "kwin_x11",
    "kwin",
    "sway",
    "hyprland",
    "Hyprland",
    "weston",
    "labwc",
    "wayfire",
    "river",
    "niri",
    "cosmic-comp",
];

const WORKER_COMMS: &[&str] = &["chrome_crashpad_handler", "chrome_crashpad"];

pub fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

pub fn name_of(p: &Process) -> String {
    if let Some(exe) = &p.exe {
        let b = basename(exe);
        if !b.is_empty() && b != "exe" && !b.starts_with('[') {
            return b;
        }
    }
    p.comm.clone()
}

fn norm(s: &str) -> String {
    s.to_ascii_lowercase()
}

pub fn is_launcher(p: &Process) -> bool {
    let named = name_of(p);
    let names = [p.comm.as_str(), named.as_str()];
    for n in names {
        if is_launcher_name(n) {
            return true;
        }
    }
    // `bash /app/bin/startvesktop`: comm is the shell, payload is argv.
    for arg in p.cmdline.iter().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        if is_launcher_name(&basename(arg)) {
            return true;
        }
        break;
    }
    false
}

pub fn is_session_noise(p: &Process) -> bool {
    let l = norm(&name_of(p));
    l == "cat" || norm(&p.comm) == "cat"
}

pub fn is_generic(p: &Process) -> bool {
    let named = name_of(p);
    let names = [p.comm.as_str(), named.as_str()];
    for n in names {
        let l = norm(n);
        if GENERICS.iter().any(|x| norm(x) == l) {
            return true;
        }
        if l.starts_with("python") || l.starts_with("node-") || l.starts_with("npm ") {
            return true;
        }
    }
    false
}

pub fn is_shell_name(name: &str) -> bool {
    let l = norm(name);
    SHELLS.iter().any(|s| norm(s) == l)
}

pub fn is_shell(p: &Process) -> bool {
    is_shell_name(&p.comm) || is_shell_name(&name_of(p))
}

pub fn is_terminal(p: &Process) -> bool {
    let named = name_of(p);
    let names = [p.comm.as_str(), named.as_str()];
    names.iter().any(|n| {
        let l = norm(n);
        TERMINALS.iter().any(|t| norm(t) == l)
    })
}

pub fn is_compositor(p: &Process) -> bool {
    let named = name_of(p);
    let names = [p.comm.as_str(), named.as_str()];
    names.iter().any(|n| {
        let l = norm(n);
        COMPOSITORS.iter().any(|t| norm(t) == l)
    })
}

pub fn is_session_bus(p: &Process) -> bool {
    session_name(p).is_some_and(|n| n.starts_with("dbus-broker"))
}

/// GNOME session / D-Bus user-bus plumbing — never an Applications row.
pub fn is_session_plumbing(p: &Process) -> bool {
    session_name(p).is_some()
}

fn session_name(p: &Process) -> Option<String> {
    for n in [p.comm.as_str(), name_of(p).as_str()] {
        let l = norm(n);
        if l.starts_with("gdm-")
            || l.starts_with("gnome-session")
            || l.starts_with("gnome-keyring")
            || l.starts_with("gnome-shell-")
            || l == "gnome-calendar"
            || l == "gnome-clocks"
            || l.starts_with("dbus-broker")
        {
            return Some(l);
        }
    }
    None
}

/// Interpreters fold into Electron/browser parents, never into a shell or systemd.
pub fn absorbs_generic(parent: &Process) -> bool {
    if is_generic(parent)
        || is_launcher(parent)
        || is_shell(parent)
        || is_terminal(parent)
        || is_compositor(parent)
    {
        return false;
    }
    let n = name_of(parent);
    n != "systemd" && parent.comm != "systemd"
}

pub fn is_interactive_shell(p: &Process) -> bool {
    if !is_shell(p) {
        return false;
    }
    let mut has_c = false;
    let mut has_path = false;
    for arg in p.cmdline.iter().skip(1) {
        if arg == "-c" {
            has_c = true;
        }
        if !arg.starts_with('-') && (arg.contains('/') || looks_script(arg)) {
            has_path = true;
        }
    }
    !has_c && !has_path
}

pub fn is_worker(p: &Process) -> bool {
    if WORKER_COMMS
        .iter()
        .any(|c| p.comm == *c || name_of(p) == *c)
    {
        return true;
    }
    p.cmdline.iter().any(|a| a.starts_with("--type="))
}

pub fn is_foldable_helper(p: &Process) -> bool {
    is_launcher(p) || (is_shell(p) && !is_interactive_shell(p))
}

pub fn is_launcher_name(name: &str) -> bool {
    let l = norm(name);
    LAUNCHERS.iter().any(|x| norm(x) == l) || l.ends_with(".appimage")
}

pub fn launcher_payload_hint(p: &Process) -> Option<String> {
    let named = name_of(p);
    if let Some(stem) = named.strip_suffix(".appimage")
        && !stem.is_empty()
    {
        return Some(stem.to_string());
    }
    if let Some(i) = p.cmdline.iter().rposition(|a| a == "--") {
        for arg in &p.cmdline[i + 1..] {
            if arg.starts_with('-') {
                continue;
            }
            let b = basename(arg);
            if b.is_empty() || is_launcher_name(&b) || is_hint_skip(&b) {
                continue;
            }
            return Some(b);
        }
    }
    None
}

fn is_hint_skip(name: &str) -> bool {
    let l = norm(name);
    HINT_SKIP.iter().any(|x| norm(x) == l)
}

pub fn looks_script(arg: &str) -> bool {
    let b = basename(arg);
    matches!(
        b.rsplit_once('.').map(|(_, e)| e),
        Some("js" | "mjs" | "cjs" | "ts" | "py" | "rb" | "pl" | "php")
    )
}

pub fn script_basename(cmdline: &[String]) -> Option<String> {
    for arg in cmdline.iter().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        if arg.contains('/') {
            let b = basename(arg);
            if !b.is_empty() {
                return Some(b);
            }
        }
        if looks_script(arg) {
            return Some(arg.clone());
        }
    }
    None
}

pub fn cmdline_flag_value<'a>(cmdline: &'a [String], flag: &str) -> Option<&'a str> {
    let mut i = 0;
    while i < cmdline.len() {
        if cmdline[i] == flag {
            return cmdline.get(i + 1).map(String::as_str);
        }
        if let Some(rest) = cmdline[i].strip_prefix(flag)
            && let Some(v) = rest.strip_prefix('=')
        {
            return Some(v);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(comm: &str, cmd: &[&str]) -> Process {
        Process {
            comm: comm.into(),
            exe: Some(format!("/usr/bin/{comm}")),
            cmdline: cmd.iter().map(|s| (*s).to_string()).collect(),
            ..Process::default()
        }
    }

    #[test]
    fn launcher_and_worker() {
        assert!(is_launcher(&p("bunx", &["bunx", "@x"])));
        assert!(is_launcher(&Process {
            comm: "cursor.appimage".into(),
            exe: Some("/home/x/cursor.appimage".into()),
            ..Process::default()
        }));
        assert!(is_launcher(&p("startvesktop", &["startvesktop"])));
        assert!(is_launcher(&p("bash", &["bash", "/app/bin/startvesktop"])));
        assert!(is_session_noise(&p("cat", &["cat"])));
        assert!(is_worker(&p("cursor", &["cursor", "--type=renderer"])));
        assert!(is_interactive_shell(&p("bash", &["-bash"])));
        assert!(!is_interactive_shell(&p(
            "bash",
            &["bash", "/app/bin/startvesktop"]
        )));
        assert!(is_generic(&p(
            "npm",
            &["npm", "exec", "@upstash/context7-mcp"]
        )));
        assert!(is_generic(&Process {
            comm: "npm exec @upsta".into(),
            exe: Some("/usr/bin/node-24".into()),
            cmdline: vec!["npm".into(), "exec".into(), "@upstash/context7-mcp".into()],
            ..Process::default()
        }));
        assert!(is_session_plumbing(&p(
            "gnome-shell-calendar-server",
            &["/usr/libexec/gnome-shell-calendar-server"]
        )));
        assert!(is_session_plumbing(&Process {
            comm: "gdm-wayland-ses".into(),
            exe: Some("/usr/libexec/gdm-wayland-session".into()),
            ..Process::default()
        }));
        assert!(is_session_bus(&p(
            "dbus-broker-launch",
            &["dbus-broker-launch", "--scope", "user"]
        )));
        assert!(!is_session_plumbing(&p("vivaldi-bin", &["vivaldi-bin"])));
        assert!(!is_session_plumbing(&p("claude", &["claude"])));
        let zypak = p(
            "bwrap",
            &[
                "bwrap",
                "--args",
                "72",
                "--",
                "/app/bin/zypak-helper",
                "child",
                "-",
                "/app/bin/vesktop/vesktop.bin",
                "--type=zygote",
            ],
        );
        assert_eq!(
            launcher_payload_hint(&zypak).as_deref(),
            Some("vesktop.bin")
        );
        assert_eq!(
            launcher_payload_hint(&p("bwrap", &["/usr/bin/bwrap", "--", "startvesktop"]))
                .as_deref(),
            None
        );
    }
}
