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

const WORKER_COMMS: &[&str] = &["chrome_crashpad_handler", "chrome_crashpad", "crashhelper"];

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
    names_of(p).iter().any(|n| n.starts_with("dbus-broker"))
}

/// GNOME session / D-Bus user-bus plumbing — never an Applications row.
pub fn is_session_plumbing(p: &Process) -> bool {
    session_helper_ident(p).is_some()
}

/// User Services identity for session helpers. PPID is usually user systemd.
///
/// A logical group is a documented unit/package/D-Bus/architecture family,
/// not a comm prefix. Prefix-only lookalikes with a different product stay out.
pub fn session_helper_ident(p: &Process) -> Option<(String, String)> {
    if is_session_bus(p) {
        return Some(ident("dbus-broker"));
    }
    if gnome_shell_helper(p) {
        return Some(ident("gnome-shell"));
    }
    if ibus_family(p) {
        return Some(ident("ibus-daemon"));
    }
    if is_atspi_registry(p) {
        return Some(ident("at-spi-bus-launcher"));
    }
    if is_goa_helper(p) {
        return Some(ident("goa-daemon"));
    }
    if names_match(p, |n| n.starts_with("p11-kit")) {
        return Some(ident("p11-kit"));
    }
    if is_gsd_disk_utility_notify(p) {
        return Some(ident("gsd-disk-utility-notify"));
    }
    if is_gsd_plugin(p) {
        return Some(ident("gnome-settings-daemon"));
    }
    if is_gvfs_stack(p) {
        return Some(ident("gvfs"));
    }
    if is_flatpak_session_infra(p) {
        return Some(ident("flatpak"));
    }
    if is_xdg_desktop_portal_family(p) {
        return Some(ident("xdg-desktop-portal"));
    }
    if is_evolution_data_server(p) {
        return Some(ident("evolution-data-server"));
    }
    if is_pipewire(p) {
        return Some(ident("pipewire"));
    }
    if is_gcr_ssh_agent(p) {
        return Some(ident("gcr-ssh-agent"));
    }
    if names_match(p, |n| n == "abrt-applet") {
        return Some(ident("abrt-applet"));
    }
    None
}

fn ident(name: &str) -> (String, String) {
    (name.to_string(), name.to_string())
}

fn names_of(p: &Process) -> [String; 2] {
    [norm(&p.comm), norm(&name_of(p))]
}

fn gnome_shell_helper(p: &Process) -> bool {
    for n in names_of(p) {
        if n.starts_with("gdm-")
            || n.starts_with("gnome-session")
            || n.starts_with("gnome-keyring")
            || n.starts_with("gnome-shell-")
            || n == "gnome-calendar"
            || n == "gnome-clocks"
            || n == "xwayland"
        {
            return true;
        }
    }
    if names_of(p).iter().any(|n| n == "gjs" || n == "gjs-console") {
        return p.cmdline.iter().any(|a| {
            let al = a.to_ascii_lowercase();
            al.contains("gnome-shell") || al.contains("org.gnome.shell")
        });
    }
    false
}

fn names_match(p: &Process, pred: impl Fn(&str) -> bool) -> bool {
    names_of(p).iter().any(|n| pred(n))
}

fn ibus_family(p: &Process) -> bool {
    names_match(p, |n| {
        n == "ibus-daemon"
            || n == "ibus-portal"
            || n == "ibus-dconf"
            || n == "ibus-x11"
            || n.starts_with("ibus-engine")
            || n.starts_with("ibus-extension")
            || n.starts_with("ibus-ui-")
    })
}

fn is_atspi_registry(p: &Process) -> bool {
    names_match(p, |n| n.starts_with("at-spi2-registr"))
}

fn is_goa_helper(p: &Process) -> bool {
    // gvfs-goa-volume-monitor is GVFS, not GOA.
    names_match(p, |n| n.starts_with("goa-"))
}

fn is_gsd_disk_utility_notify(p: &Process) -> bool {
    names_match(p, |n| n.starts_with("gsd-disk-utilit"))
        || p.exe
            .as_deref()
            .is_some_and(|e| e.contains("gsd-disk-utility-notify"))
        || p.cgroup.contains("DiskUtilityNotify")
}

fn is_gsd_plugin(p: &Process) -> bool {
    names_match(p, |n| n.starts_with("gsd-"))
}

fn is_gvfs_stack(p: &Process) -> bool {
    if names_match(p, |n| {
        n == "gvfsd" || n.starts_with("gvfsd-") || n.starts_with("gvfs-")
    }) {
        return true;
    }
    // wsdd is a separate RPM; fold only when the gvfs daemon unit spawned it.
    names_match(p, |n| n == "wsdd") && p.cgroup.contains("gvfs-")
}

fn is_flatpak_session_infra(p: &Process) -> bool {
    if names_match(p, |n| {
        n.starts_with("flatpak-session") || n == "flatpak-portal"
    }) {
        return true;
    }
    // App-bound proxy already bills to that Flatpak app via launcher folding.
    names_match(p, |n| n == "xdg-dbus-proxy") && !p.cgroup.contains("app-flatpak-")
}

fn is_xdg_desktop_portal_family(p: &Process) -> bool {
    if names_match(p, |n| {
        n == "xdg-desktop-portal"
            || n.starts_with("xdg-desktop-portal-")
            || n == "xdg-document-portal"
            || n == "xdg-permission-store"
    }) {
        return true;
    }
    names_match(p, |n| n.starts_with("fusermount")) && p.cgroup.contains("xdg-document-portal")
}

fn is_evolution_data_server(p: &Process) -> bool {
    names_match(p, |n| {
        n.starts_with("evolution-addressbook")
            || n.starts_with("evolution-calendar")
            || n.starts_with("evolution-source")
            || n.starts_with("evolution-alarm")
            || n == "evolution-data-server"
    }) || p
        .exe
        .as_deref()
        .is_some_and(|e| e.contains("/evolution-data-server/"))
}

fn is_pipewire(p: &Process) -> bool {
    names_match(p, |n| n == "pipewire" || n == "pipewire-pulse")
}

fn is_gcr_ssh_agent(p: &Process) -> bool {
    if names_match(p, |n| n.starts_with("gcr-ssh-agent")) {
        return true;
    }
    names_match(p, |n| n == "ssh-agent") && p.cgroup.contains("gcr-ssh-agent")
}

/// Firefox/Chromium crash helper whose parent is often user systemd.
pub fn crash_helper_app(p: &Process) -> Option<String> {
    if !names_of(p).iter().any(|n| is_crash_helper_name(n)) {
        return None;
    }
    for s in p.exe.iter().chain(p.cmdline.iter()) {
        if let Some(app) = app_from_crash_helper_path(s) {
            return Some(app);
        }
    }
    None
}

fn is_crash_helper_name(n: &str) -> bool {
    matches!(
        n,
        "crashhelper" | "chrome_crashpad_handler" | "chrome_crashpad"
    )
}

fn app_from_crash_helper_path(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    if lower.contains("/firefox/") || lower.ends_with("/firefox") {
        return Some("firefox".to_string());
    }
    if !is_crash_helper_name(&basename(s)) {
        return None;
    }
    let dir = s.rsplit_once('/')?.0;
    let owner = basename(dir);
    if owner.is_empty() || matches!(owner.as_str(), "bin" | "libexec" | "lib" | "lib64") {
        return None;
    }
    Some(owner)
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
        assert!(!is_session_plumbing(&p("cursor", &["cursor"])));
        assert!(!is_session_plumbing(&p("vesktop.bin", &["vesktop.bin"])));
        assert_eq!(
            session_helper_ident(&Process {
                comm: "gjs".into(),
                exe: Some("/usr/bin/gjs-console".into()),
                cmdline: vec![
                    "/usr/bin/gjs".into(),
                    "/usr/share/gnome-shell/org.gnome.Shell.Notifications".into(),
                ],
                ..Process::default()
            })
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("gnome-shell")
        );
        assert!(
            session_helper_ident(&p("gjs-console", &["gjs", "/home/u/my-app.js"])).is_none(),
            "unrelated gjs is not gnome-shell"
        );
        assert_eq!(
            session_helper_ident(&p("ibus-portal", &["/usr/libexec/ibus-portal"]))
                .as_ref()
                .map(|(k, _)| k.as_str()),
            Some("ibus-daemon")
        );
        assert_eq!(
            session_helper_ident(&p(
                "at-spi2-registryd",
                &["/usr/libexec/at-spi2-registryd", "--use-gnome-session"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("at-spi-bus-launcher")
        );
        assert_eq!(
            session_helper_ident(&p(
                "goa-identity-service",
                &["/usr/libexec/goa-identity-service"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("goa-daemon")
        );
        assert_eq!(
            session_helper_ident(&p("goa-daemon", &["/usr/libexec/goa-daemon"]))
                .as_ref()
                .map(|(k, _)| k.as_str()),
            Some("goa-daemon")
        );
        assert_eq!(
            session_helper_ident(&p(
                "p11-kit-server",
                &["/usr/libexec/p11-kit/p11-kit-server"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("p11-kit")
        );
        assert_eq!(
            session_helper_ident(&p(
                "p11-kit-remote",
                &["/usr/libexec/p11-kit/p11-kit-remote"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("p11-kit")
        );
        assert!(
            session_helper_ident(&Process {
                comm: "cursor".into(),
                exe: Some("/tmp/.mount_cursor/usr/share/cursor/cursor".into()),
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice/flatpak-session-helper.service".into(),
                ..Process::default()
            })
            .is_none(),
            "Cursor in the lying flatpak-session-helper unit is not p11-kit"
        );
        assert_eq!(
            session_helper_ident(&p(
                "gsd-disk-utility-notify",
                &["/usr/libexec/gsd-disk-utility-notify"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("gsd-disk-utility-notify")
        );
        assert_eq!(
            session_helper_ident(&p("gsd-color", &["/usr/libexec/gsd-color"]))
                .as_ref()
                .map(|(k, _)| k.as_str()),
            Some("gnome-settings-daemon")
        );
        assert_eq!(
            session_helper_ident(&p("gvfsd", &["/usr/libexec/gvfsd"]))
                .as_ref()
                .map(|(k, _)| k.as_str()),
            Some("gvfs")
        );
        assert_eq!(
            session_helper_ident(&p(
                "gvfs-goa-volume-monitor",
                &["/usr/libexec/gvfs-goa-volume-monitor"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("gvfs"),
            "gvfs GOA volume monitor is GVFS, not goa-daemon"
        );
        assert_eq!(
            session_helper_ident(&Process {
                comm: "wsdd".into(),
                exe: Some("/usr/bin/wsdd".into()),
                cmdline: vec!["/usr/bin/python3".into(), "/usr/bin/wsdd".into()],
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/session.slice/gvfs-daemon.service".into(),
                ..Process::default()
            })
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("gvfs")
        );
        assert!(
            session_helper_ident(&p("wsdd", &["/usr/bin/wsdd"])).is_none(),
            "independent wsdd is not gvfs"
        );
        assert_eq!(
            session_helper_ident(&p(
                "flatpak-session-helper",
                &["/usr/libexec/flatpak-session-helper"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("flatpak")
        );
        assert_eq!(
            session_helper_ident(&p("flatpak-portal", &["/usr/libexec/flatpak-portal"]))
                .as_ref()
                .map(|(k, _)| k.as_str()),
            Some("flatpak")
        );
        assert_eq!(
            session_helper_ident(&Process {
                comm: "xdg-dbus-proxy".into(),
                exe: Some("/usr/bin/xdg-dbus-proxy".into()),
                cmdline: vec!["/usr/bin/xdg-dbus-proxy".into()],
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/session.slice/xdg-desktop-portal.service".into(),
                ..Process::default()
            })
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("flatpak"),
            "unbound xdg-dbus-proxy is Flatpak portal plumbing"
        );
        assert!(
            session_helper_ident(&Process {
                comm: "xdg-dbus-proxy".into(),
                exe: Some("/usr/bin/xdg-dbus-proxy".into()),
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-dev.vencord.Vesktop-1.scope".into(),
                ..Process::default()
            })
            .is_none(),
            "app-bound xdg-dbus-proxy must not become the flatpak service"
        );
        assert_eq!(
            session_helper_ident(&p(
                "xdg-desktop-portal-gnome",
                &["/usr/libexec/xdg-desktop-portal-gnome"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("xdg-desktop-portal")
        );
        assert_eq!(
            session_helper_ident(&p(
                "evolution-addressbook-factory",
                &["/usr/libexec/evolution-addressbook-factory"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("evolution-data-server")
        );
        assert!(
            session_helper_ident(&p("evolution", &["/usr/bin/evolution"])).is_none(),
            "Evolution GUI is not evolution-data-server"
        );
        assert_eq!(
            session_helper_ident(&p("pipewire-pulse", &["/usr/bin/pipewire-pulse"]))
                .as_ref()
                .map(|(k, _)| k.as_str()),
            Some("pipewire")
        );
        assert!(
            session_helper_ident(&p("wireplumber", &["/usr/bin/wireplumber"])).is_none(),
            "wireplumber is a different package than pipewire"
        );
        assert_eq!(
            session_helper_ident(&p("ibus-x11", &["/usr/libexec/ibus-x11"]))
                .as_ref()
                .map(|(k, _)| k.as_str()),
            Some("ibus-daemon")
        );
        assert_eq!(
            session_helper_ident(&p("ibus-dconf", &["/usr/libexec/ibus-dconf"]))
                .as_ref()
                .map(|(k, _)| k.as_str()),
            Some("ibus-daemon")
        );
        assert_eq!(
            session_helper_ident(&Process {
                comm: "Xwayland".into(),
                exe: Some("/usr/bin/Xwayland".into()),
                ..Process::default()
            })
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("gnome-shell")
        );
        assert_eq!(
            session_helper_ident(&Process {
                comm: "ssh-agent".into(),
                exe: Some("/usr/bin/ssh-agent".into()),
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice/gcr-ssh-agent.service".into(),
                ..Process::default()
            })
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("gcr-ssh-agent")
        );
        assert!(
            session_helper_ident(&p("ssh-agent", &["/usr/bin/ssh-agent"])).is_none(),
            "ssh-agent outside gcr-ssh-agent is not that service"
        );
        assert!(
            session_helper_ident(&p("majordomo", &["majordomo", "lead", "get"])).is_none(),
            "majordomo CLI is not a user service"
        );
        assert_eq!(
            session_helper_ident(&p(
                "abrt-applet",
                &["/usr/bin/abrt-applet", "--gapplication-service"]
            ))
            .as_ref()
            .map(|(k, _)| k.as_str()),
            Some("abrt-applet")
        );
        assert!(!is_session_plumbing(&p("gnome-abrt", &["gnome-abrt"])));
        assert!(is_worker(&Process {
            comm: "crashhelper".into(),
            exe: Some("/usr/lib64/firefox/crashhelper".into()),
            cmdline: vec![
                "crashhelper".into(),
                "12766".into(),
                "9".into(),
                "/tmp/".into()
            ],
            ..Process::default()
        }));
        assert_eq!(
            crash_helper_app(&Process {
                comm: "crashhelper".into(),
                exe: Some("/usr/lib64/firefox/crashhelper".into()),
                cmdline: vec![
                    "crashhelper".into(),
                    "12766".into(),
                    "9".into(),
                    "/tmp/".into(),
                    "11".into(),
                ],
                ppid: 6475,
                ..Process::default()
            })
            .as_deref(),
            Some("firefox")
        );
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
