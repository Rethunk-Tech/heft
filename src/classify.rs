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
    "firejail",
    "xdg-dbus-proxy",
    "zypak-sandbox",
];

const GENERICS: &[&str] = &[
    "bun",
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
    "Hyprland",
    "weston",
    "labwc",
    "wayfire",
    "river",
    "niri",
    "cosmic-comp",
];

pub(crate) fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

pub(crate) fn name_of(p: &Process) -> String {
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

pub(crate) fn is_launcher(p: &Process) -> bool {
    if names_match(&names_of(p), is_launcher_name) {
        return true;
    }
    // A shell wrapper reports the shell as comm; the launcher it execs is the
    // first positional argument.
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

pub(crate) fn is_session_noise(p: &Process) -> bool {
    names_match(&names_of(p), |n| n == "cat")
}

pub(crate) fn is_generic(p: &Process) -> bool {
    names_match(&names_of(p), |n| {
        GENERICS.iter().any(|x| x.eq_ignore_ascii_case(n))
            || n.starts_with("python")
            || n.starts_with("node-")
            || n.starts_with("npm ")
    })
}

fn is_shell_name(name: &str) -> bool {
    SHELLS.iter().any(|s| s.eq_ignore_ascii_case(name))
}

fn is_shell(p: &Process) -> bool {
    names_match(&names_of(p), is_shell_name)
}

pub(crate) fn is_terminal_name(name: &str) -> bool {
    TERMINALS.iter().any(|t| t.eq_ignore_ascii_case(name))
}

pub(crate) fn is_terminal(p: &Process) -> bool {
    names_match(&names_of(p), is_terminal_name)
}

pub(crate) fn is_compositor(p: &Process) -> bool {
    names_match(&names_of(p), |n| {
        COMPOSITORS.iter().any(|t| t.eq_ignore_ascii_case(n))
    })
}

fn is_session_bus(n: &[String]) -> bool {
    n.iter().any(|x| x.starts_with("dbus-broker"))
}

/// User Services identity for session helpers. PPID is usually user systemd.
///
/// A logical group is a documented unit/package/D-Bus/architecture family,
/// not a comm prefix. Prefix-only lookalikes with a different product stay out.
pub(crate) fn session_helper_ident(p: &Process) -> Option<&'static str> {
    let n = names_of(p);
    if is_session_bus(&n) {
        return Some("dbus-broker");
    }
    if gnome_shell_helper(&n, p) {
        return Some("gnome-shell");
    }
    if plasma_workspace(&n) {
        return Some("plasmashell");
    }
    if kwin_family(&n) {
        return Some("kwin");
    }
    if ibus_family(&n) {
        return Some("ibus-daemon");
    }
    if is_atspi_registry(&n) {
        return Some("at-spi-bus-launcher");
    }
    if is_goa_helper(&n) {
        return Some("goa-daemon");
    }
    if names_match(&n, |x| x.starts_with("p11-kit")) {
        return Some("p11-kit");
    }
    if is_gsd_disk_utility_notify(&n, p) {
        return Some("gsd-disk-utility-notify");
    }
    if is_gsd_plugin(&n) {
        return Some("gnome-settings-daemon");
    }
    if is_gvfs_stack(&n, p) {
        return Some("gvfs");
    }
    if is_flatpak_session_infra(&n, p) {
        return Some("flatpak");
    }
    if is_xdg_desktop_portal_family(&n, p) {
        return Some("xdg-desktop-portal");
    }
    if is_evolution_data_server(&n, p) {
        return Some("evolution-data-server");
    }
    if is_pipewire(&n) {
        return Some("pipewire");
    }
    if is_gcr_ssh_agent(&n, p) {
        return Some("gcr-ssh-agent");
    }
    if names_match(&n, |x| x == "abrt-applet") {
        return Some("abrt-applet");
    }
    None
}

pub(crate) fn names_of(p: &Process) -> [String; 2] {
    [norm(&p.comm), norm(&name_of(p))]
}

fn gnome_shell_helper(names: &[String], p: &Process) -> bool {
    if names_match(names, |n| {
        n.starts_with("gdm-")
            || n.starts_with("gnome-session")
            || n.starts_with("gnome-keyring")
            || n.starts_with("gnome-shell-")
            || matches!(n, "gnome-calendar" | "gnome-clocks" | "xwayland")
    }) {
        return true;
    }
    if names_match(names, |n| n == "gjs" || n == "gjs-console") {
        return p.cmdline.iter().any(|a| {
            let al = a.to_ascii_lowercase();
            al.contains("gnome-shell") || al.contains("org.gnome.shell")
        });
    }
    false
}

/// plasma-workspace / kactivitymanagerd / kglobalacceld / kded session
/// processes, from those RPMs' `/usr/bin` and `/usr/libexec` files. Not a
/// `plasma-` prefix: `plasma-nm`, `plasma-discover` and `krunner` are
/// independent apps.
fn plasma_workspace(names: &[String]) -> bool {
    names_match(names, |n| {
        matches!(
            n,
            "plasmashell"
                | "plasma_session"
                | "startplasma"
                | "startplasma-wayland"
                | "ksmserver"
                | "ksplashqml"
                | "plasma-shutdown"
                | "xembedsniproxy"
                | "gmenudbusmenuproxy"
                | "plasma_waitforname"
                | "kde-systemd-start-condition"
                | "kcminit"
                | "kcminit_startup"
                | "kaccess"
                | "kactivitymanagerd"
                | "kglobalacceld"
                | "kded"
                | "kded5"
                | "kded6"
        )
    })
}

/// kwin and the helpers kwin-common ships beside it. The compositor binaries
/// are already in `COMPOSITORS`; this is the merge key so `kwin_wayland` and
/// `kwin_wayland_wrapper` are one User Services row.
fn kwin_family(names: &[String]) -> bool {
    names_match(names, |n| {
        n == "kwin" || n.starts_with("kwin_") || n.starts_with("kwin-")
    })
}

pub(crate) fn names_match(names: &[String], pred: impl Fn(&str) -> bool) -> bool {
    names.iter().any(|n| pred(n))
}

fn ibus_family(names: &[String]) -> bool {
    names_match(names, |n| {
        n == "ibus-daemon"
            || n == "ibus-portal"
            || n == "ibus-dconf"
            || n == "ibus-x11"
            || n.starts_with("ibus-engine")
            || n.starts_with("ibus-extension")
            || n.starts_with("ibus-ui-")
    })
}

fn is_atspi_registry(names: &[String]) -> bool {
    names_match(names, |n| n.starts_with("at-spi2-registr"))
}

fn is_goa_helper(names: &[String]) -> bool {
    // gvfs-goa-volume-monitor is GVFS, not GOA.
    names_match(names, |n| n.starts_with("goa-"))
}

fn is_gsd_disk_utility_notify(names: &[String], p: &Process) -> bool {
    names_match(names, |n| n.starts_with("gsd-disk-utilit"))
        || p.exe
            .as_deref()
            .is_some_and(|e| e.contains("gsd-disk-utility-notify"))
        || p.cgroup.contains("DiskUtilityNotify")
}

fn is_gsd_plugin(names: &[String]) -> bool {
    names_match(names, |n| n.starts_with("gsd-"))
}

fn is_gvfs_stack(names: &[String], p: &Process) -> bool {
    if names_match(names, |n| {
        n == "gvfsd" || n.starts_with("gvfsd-") || n.starts_with("gvfs-")
    }) {
        return true;
    }
    // wsdd is a separate RPM; fold only when the gvfs daemon unit spawned it.
    names_match(names, |n| n == "wsdd") && p.cgroup.contains("gvfs-")
}

fn is_flatpak_session_infra(names: &[String], p: &Process) -> bool {
    if names_match(names, |n| {
        n.starts_with("flatpak-session") || n == "flatpak-portal"
    }) {
        return true;
    }
    // App-bound proxy already bills to that Flatpak app via launcher folding.
    names_match(names, |n| n == "xdg-dbus-proxy") && !p.cgroup.contains("app-flatpak-")
}

fn is_xdg_desktop_portal_family(names: &[String], p: &Process) -> bool {
    if names_match(names, |n| {
        n == "xdg-desktop-portal"
            || n.starts_with("xdg-desktop-portal-")
            || n == "xdg-document-portal"
            || n == "xdg-permission-store"
    }) {
        return true;
    }
    names_match(names, |n| n.starts_with("fusermount")) && p.cgroup.contains("xdg-document-portal")
}

fn is_evolution_data_server(names: &[String], p: &Process) -> bool {
    names_match(names, |n| {
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

fn is_pipewire(names: &[String]) -> bool {
    names_match(names, |n| n == "pipewire" || n == "pipewire-pulse")
}

fn is_gcr_ssh_agent(names: &[String], p: &Process) -> bool {
    if names_match(names, |n| n.starts_with("gcr-ssh-agent")) {
        return true;
    }
    names_match(names, |n| n == "ssh-agent") && p.cgroup.contains("gcr-ssh-agent")
}

/// An app's own helper binary that is not a `--type=` worker, so it would
/// otherwise split into a row of its own. Editor install and extension trees
/// decide by `exe` path rather than a PPID walk: `claude` started from the
/// integrated terminal is a real app and keeps its row, while `codex`,
/// `language_server_linux_x64` or `vscode-tailscale` shipped by an extension
/// are that editor whoever spawned them.
pub(crate) fn bundled_helper_app(p: &Process) -> Option<&'static str> {
    const TREES: &[(&str, &str)] = &[
        ("/.vscode/extensions/", "code"),
        ("/usr/share/code/", "code"),
        ("/.vscode-insiders/extensions/", "code-insiders"),
        ("/usr/share/code-insiders/", "code-insiders"),
        ("/.cursor/extensions/", "cursor"),
    ];
    if let Some(exe) = p.exe.as_deref()
        && let Some((_, app)) = TREES.iter().find(|(dir, _)| exe.contains(dir))
    {
        return Some(app);
    }
    names_match(&names_of(p), |n| n == "deskflow-core").then_some("deskflow")
}

/// Firefox/Chromium crash helper whose parent is often user systemd.
pub(crate) fn crash_helper_app(p: &Process) -> Option<String> {
    if !names_match(&names_of(p), is_crash_helper_name) {
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
    if !is_crash_helper_name(&norm(&basename(s))) {
        return None;
    }
    let dir = s.rsplit_once('/')?.0;
    let owner = basename(dir);
    if owner.is_empty() || matches!(owner.as_str(), "bin" | "libexec" | "lib" | "lib64") {
        return None;
    }
    // AppImage mounts are per-run (`/tmp/mount`, `/tmp/.mount_cursorAb12Cd`)
    // and never an identity. A stable directory nested under the mount
    // (`…/usr/share/cursor/chrome_crashpad_handler`) is the same owner
    // `/opt/cursor/…` would name. Chromium reparents the helper to user
    // systemd, so there is no ancestor to fall back to when the parent dir
    // *is* the mount — declining that case still lets `group` walk PPID.
    if is_temp_unpack_root(&lower) && is_ephemeral_mount_dir(&owner) {
        return None;
    }
    Some(owner)
}

fn is_temp_unpack_root(lower: &str) -> bool {
    ["/tmp/", "/var/tmp/", "/run/"]
        .iter()
        .any(|root| lower.starts_with(root))
}

fn is_ephemeral_mount_dir(owner: &str) -> bool {
    let n = norm(owner);
    n == "mount" || n.starts_with(".mount") || n == "appimage"
}

/// Interpreters fold into Electron/browser parents, never into a shell or systemd.
pub(crate) fn absorbs_generic(parent: &Process) -> bool {
    if is_generic(parent)
        || is_launcher(parent)
        || is_shell(parent)
        || is_terminal(parent)
        || is_compositor(parent)
    {
        return false;
    }
    // The raw parent process's names, deliberately not the resolved Place `key`
    // that `group::compute_place` tests: this runs before the parent is placed,
    // so a process whose identity later folds elsewhere is still judged here by
    // what it actually is.
    !names_match(&names_of(parent), |n| n == "systemd")
}

pub(crate) fn is_interactive_shell(p: &Process) -> bool {
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

pub(crate) fn is_worker(p: &Process) -> bool {
    names_match(&names_of(p), is_crash_helper_name)
        || p.cmdline.iter().any(|a| a.starts_with("--type="))
}

/// Launchers and shells share unique-payload folding: the helper has no
/// top-level row when a single child identity exists. Interactive shells
/// are included so a `bash` that launched `claude` bills there; an idle
/// leftover folds into the terminal in `group`, not here.
pub(crate) fn is_foldable_helper(p: &Process) -> bool {
    is_launcher(p) || is_shell(p)
}

pub(crate) fn is_launcher_name(name: &str) -> bool {
    LAUNCHERS.iter().any(|x| x.eq_ignore_ascii_case(name)) || norm(name).ends_with(".appimage")
}

pub(crate) fn launcher_payload_hint(p: &Process) -> Option<String> {
    let named = name_of(p);
    // Case-folded the way `is_launcher_name` folds it: real AppImages ship as
    // `Cursor-x86_64.AppImage`, and `name_of` does not lowercase. A launcher
    // that yields no hint falls through to `user_place` and takes the
    // top-level row launchers are not supposed to have. The stem keeps its
    // own casing because it becomes the displayed identity.
    if norm(&named).ends_with(".appimage") {
        let stem = &named[..named.len() - ".appimage".len()];
        if !stem.is_empty() {
            return Some(stem.to_string());
        }
    }
    if let Some(i) = p.cmdline.iter().rposition(|a| a == "--") {
        for arg in &p.cmdline[i + 1..] {
            if arg.starts_with('-') {
                continue;
            }
            let b = basename(arg);
            // "child" is a zypak-helper subcommand, not a payload.
            if b.is_empty() || is_launcher_name(&b) || b.eq_ignore_ascii_case("child") {
                continue;
            }
            return Some(b);
        }
    }
    None
}

fn looks_script(arg: &str) -> bool {
    let b = basename(arg);
    matches!(
        b.rsplit_once('.').map(|(_, e)| e),
        Some("js" | "mjs" | "cjs" | "ts" | "py" | "rb" | "pl" | "php")
    )
}

pub(crate) fn script_basename(cmdline: &[String]) -> Option<String> {
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

pub(crate) fn cmdline_flag_value<'a>(cmdline: &'a [String], flag: &str) -> Option<&'a str> {
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
    fn appimage_launcher_hint_ignores_case() {
        // Shipping AppImages are mixed case; every other fixture here is not.
        let cursor = Process {
            comm: "Cursor-x86_64.AppImage".into(),
            exe: Some("/opt/Cursor-x86_64.AppImage".into()),
            ..Process::default()
        };
        assert!(is_launcher(&cursor));
        assert_eq!(
            launcher_payload_hint(&cursor).as_deref(),
            Some("Cursor-x86_64"),
            "a launcher with no hint takes a top-level row of its own"
        );
    }

    #[test]
    fn launcher_and_worker() {
        assert!(is_launcher(&p("bunx", &["bunx", "@x"])));
        assert!(is_launcher(&Process {
            comm: "cursor.appimage".into(),
            exe: Some("/home/x/cursor.appimage".into()),
            ..Process::default()
        }));
        assert!(is_launcher(&p("zypak-helper", &["zypak-helper", "child"])));
        assert!(is_launcher(&p("bash", &["bash", "/app/bin/AppRun"])));
        assert!(is_session_noise(&p("cat", &["cat"])));
        assert!(is_worker(&p("cursor", &["cursor", "--type=renderer"])));
        assert!(is_interactive_shell(&p("bash", &["-bash"])));
        assert!(
            is_interactive_shell(&p("bash", &["/bin/bash", "--posix"])),
            "ghostty's login bash is interactive"
        );
        assert!(
            is_foldable_helper(&p("bash", &["-bash"])),
            "interactive shells share unique-payload folding with launchers"
        );
        assert!(is_terminal_name("ghostty"));
        assert!(is_terminal(&p("ghostty", &["/usr/bin/ghostty"])));
        let wrapper = p(
            "bash",
            &["bash", "/app/bin/zypak-wrapper", "/app/extra/vscode/code"],
        );
        assert!(!is_interactive_shell(&wrapper));
        assert!(
            is_foldable_helper(&wrapper),
            "a flatpak wrapper script folds as a non-interactive shell"
        );
        assert!(is_generic(&p("python3.12", &["python3.12", "x.py"])));
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
        assert!(
            session_helper_ident(&p(
                "gnome-shell-calendar-server",
                &["/usr/libexec/gnome-shell-calendar-server"]
            ))
            .is_some()
        );
        assert!(
            session_helper_ident(&Process {
                comm: "gdm-wayland-ses".into(),
                exe: Some("/usr/libexec/gdm-wayland-session".into()),
                ..Process::default()
            })
            .is_some()
        );
        assert_eq!(
            session_helper_ident(&p("ksmserver", &["/usr/bin/ksmserver"])),
            Some("plasmashell")
        );
        assert_eq!(
            session_helper_ident(&p("kded6", &["/usr/bin/kded6"])),
            Some("plasmashell")
        );
        assert_eq!(
            session_helper_ident(&p(
                "kwin_wayland_wrapper",
                &["/usr/bin/kwin_wayland_wrapper"]
            )),
            Some("kwin")
        );
        assert_eq!(
            session_helper_ident(&p("kwin_wayland", &["/usr/bin/kwin_wayland"])),
            Some("kwin")
        );
        assert!(session_helper_ident(&p("krunner", &["/usr/bin/krunner"])).is_none());
        assert!(session_helper_ident(&p("plasma-discover", &["plasma-discover"])).is_none());
        assert!(session_helper_ident(&p("kwindowprop", &["kwindowprop"])).is_none());
        assert!(is_session_bus(&names_of(&p(
            "dbus-broker-launch",
            &["dbus-broker-launch", "--scope", "user"]
        ))));
        assert!(session_helper_ident(&p("vivaldi-bin", &["vivaldi-bin"])).is_none());
        assert!(session_helper_ident(&p("claude", &["claude"])).is_none());
        assert!(session_helper_ident(&p("cursor", &["cursor"])).is_none());
        assert!(session_helper_ident(&p("code", &["code"])).is_none());
        assert_eq!(
            session_helper_ident(&Process {
                comm: "gjs".into(),
                exe: Some("/usr/bin/gjs-console".into()),
                cmdline: vec![
                    "/usr/bin/gjs".into(),
                    "/usr/share/gnome-shell/org.gnome.Shell.Notifications".into(),
                ],
                ..Process::default()
            }),
            Some("gnome-shell")
        );
        assert!(
            session_helper_ident(&p("gjs-console", &["gjs", "/home/u/my-app.js"])).is_none(),
            "unrelated gjs is not gnome-shell"
        );
        assert_eq!(
            session_helper_ident(&p("ibus-portal", &["/usr/libexec/ibus-portal"])),
            Some("ibus-daemon")
        );
        assert_eq!(
            session_helper_ident(&p(
                "at-spi2-registryd",
                &["/usr/libexec/at-spi2-registryd", "--use-gnome-session"]
            )),
            Some("at-spi-bus-launcher")
        );
        assert_eq!(
            session_helper_ident(&p(
                "goa-identity-service",
                &["/usr/libexec/goa-identity-service"]
            )),
            Some("goa-daemon")
        );
        assert_eq!(
            session_helper_ident(&p("goa-daemon", &["/usr/libexec/goa-daemon"])),
            Some("goa-daemon")
        );
        assert_eq!(
            session_helper_ident(&p(
                "p11-kit-server",
                &["/usr/libexec/p11-kit/p11-kit-server"]
            )),
            Some("p11-kit")
        );
        assert_eq!(
            session_helper_ident(&p(
                "p11-kit-remote",
                &["/usr/libexec/p11-kit/p11-kit-remote"]
            )),
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
            )),
            Some("gsd-disk-utility-notify")
        );
        assert_eq!(
            session_helper_ident(&p("gsd-color", &["/usr/libexec/gsd-color"])),
            Some("gnome-settings-daemon")
        );
        assert_eq!(
            session_helper_ident(&p("gvfsd", &["/usr/libexec/gvfsd"])),
            Some("gvfs")
        );
        assert_eq!(
            session_helper_ident(&p(
                "gvfs-goa-volume-monitor",
                &["/usr/libexec/gvfs-goa-volume-monitor"]
            )),
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
            }),
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
            )),
            Some("flatpak")
        );
        assert_eq!(
            session_helper_ident(&p("flatpak-portal", &["/usr/libexec/flatpak-portal"])),
            Some("flatpak")
        );
        assert_eq!(
            session_helper_ident(&Process {
                comm: "xdg-dbus-proxy".into(),
                exe: Some("/usr/bin/xdg-dbus-proxy".into()),
                cmdline: vec!["/usr/bin/xdg-dbus-proxy".into()],
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/session.slice/xdg-desktop-portal.service".into(),
                ..Process::default()
            }),
            Some("flatpak"),
            "unbound xdg-dbus-proxy is Flatpak portal plumbing"
        );
        assert!(
            session_helper_ident(&Process {
                comm: "xdg-dbus-proxy".into(),
                exe: Some("/usr/bin/xdg-dbus-proxy".into()),
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-com.visualstudio.code-1.scope".into(),
                ..Process::default()
            })
            .is_none(),
            "app-bound xdg-dbus-proxy must not become the flatpak service"
        );
        assert_eq!(
            session_helper_ident(&p(
                "xdg-desktop-portal-gnome",
                &["/usr/libexec/xdg-desktop-portal-gnome"]
            )),
            Some("xdg-desktop-portal")
        );
        assert_eq!(
            session_helper_ident(&p(
                "evolution-addressbook-factory",
                &["/usr/libexec/evolution-addressbook-factory"]
            )),
            Some("evolution-data-server")
        );
        assert!(
            session_helper_ident(&p("evolution", &["/usr/bin/evolution"])).is_none(),
            "Evolution GUI is not evolution-data-server"
        );
        assert_eq!(
            session_helper_ident(&p("pipewire-pulse", &["/usr/bin/pipewire-pulse"])),
            Some("pipewire")
        );
        assert!(
            session_helper_ident(&p("wireplumber", &["/usr/bin/wireplumber"])).is_none(),
            "wireplumber is a different package than pipewire"
        );
        assert_eq!(
            session_helper_ident(&p("ibus-x11", &["/usr/libexec/ibus-x11"])),
            Some("ibus-daemon")
        );
        assert_eq!(
            session_helper_ident(&p("ibus-dconf", &["/usr/libexec/ibus-dconf"])),
            Some("ibus-daemon")
        );
        assert_eq!(
            session_helper_ident(&Process {
                comm: "Xwayland".into(),
                exe: Some("/usr/bin/Xwayland".into()),
                ..Process::default()
            }),
            Some("gnome-shell")
        );
        assert_eq!(
            session_helper_ident(&Process {
                comm: "ssh-agent".into(),
                exe: Some("/usr/bin/ssh-agent".into()),
                cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice/gcr-ssh-agent.service".into(),
                ..Process::default()
            }),
            Some("gcr-ssh-agent")
        );
        assert!(
            session_helper_ident(&p("ssh-agent", &["/usr/bin/ssh-agent"])).is_none(),
            "ssh-agent outside gcr-ssh-agent is not that service"
        );
        assert!(
            session_helper_ident(&p("htop", &["htop", "-u", "1000"])).is_none(),
            "an arbitrary user CLI is not a user service"
        );
        assert_eq!(
            session_helper_ident(&p(
                "abrt-applet",
                &["/usr/bin/abrt-applet", "--gapplication-service"]
            )),
            Some("abrt-applet")
        );
        assert!(session_helper_ident(&p("gnome-abrt", &["gnome-abrt"])).is_none());
        let exe = |path: &str| Process {
            comm: basename(path),
            exe: Some(path.into()),
            ..Process::default()
        };
        for helper in [
            "/home/u/.vscode/extensions/openai.chatgpt-26.5.0-linux-x64/bin/linux-x86_64/codex",
            "/home/u/.vscode/extensions/tailscale.vscode-tailscale-1.0.0/bin/vscode-tailscale",
            "/usr/share/code/bin/code-tunnel",
        ] {
            assert_eq!(bundled_helper_app(&exe(helper)), Some("code"), "{helper}");
        }
        assert_eq!(
            bundled_helper_app(&exe(
                "/home/u/.cursor/extensions/x/bin/language_server_linux_x64"
            )),
            Some("cursor")
        );
        assert_eq!(
            bundled_helper_app(&exe("/usr/share/code-insiders/bin/code-tunnel-insiders")),
            Some("code-insiders"),
            "Insiders is its own row, not a prefix match on /usr/share/code"
        );
        assert_eq!(
            bundled_helper_app(&exe("/home/u/.local/bin/claude")),
            None,
            "a CLI installed outside the editor keeps its own row"
        );
        assert_eq!(
            bundled_helper_app(&exe("/usr/bin/deskflow-core")),
            Some("deskflow")
        );
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
        assert_eq!(
            crash_helper_app(&p(
                "chrome_crashpad_handler",
                &["/opt/cursor/chrome_crashpad_handler"]
            ))
            .as_deref(),
            Some("cursor"),
            "an install path still names the owning app"
        );
        for temp in [
            "/tmp/mount/chrome_crashpad_handler",
            "/tmp/.mount_cursorAb12Cd/chrome_crashpad_handler",
            "/run/user/1000/appimage/chrome_crashpad_handler",
        ] {
            assert_eq!(
                crash_helper_app(&Process {
                    comm: "chrome_crashpad_handler".into(),
                    exe: Some(temp.into()),
                    cmdline: vec![temp.into()],
                    ..Process::default()
                }),
                None,
                "a temp mount directory is not an app identity: {temp}"
            );
        }
        assert_eq!(
            crash_helper_app(&Process {
                comm: "chrome_crashpad".into(),
                exe: Some(
                    "/tmp/.mount_cursorIDenmC/usr/share/cursor/chrome_crashpad_handler".into()
                ),
                cmdline: vec![
                    "/tmp/.mount_cursorIDenmC/usr/share/cursor/chrome_crashpad_handler".into()
                ],
                ppid: 6475,
                ..Process::default()
            })
            .as_deref(),
            Some("cursor"),
            "a nested owner under an AppImage mount still names the app"
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
                "/app/extra/vscode/code",
                "--type=zygote",
            ],
        );
        assert_eq!(launcher_payload_hint(&zypak).as_deref(), Some("code"));
        assert_eq!(
            launcher_payload_hint(&p("bwrap", &["/usr/bin/bwrap", "--", "/usr/bin/flatpak"]))
                .as_deref(),
            None
        );
    }
}
