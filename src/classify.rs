use crate::identity;
use crate::rules::{Classes, Rules};
use crate::types::Process;

pub(crate) fn basename(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, b)| b)
}

pub(crate) fn name_of(p: &Process) -> String {
    name_ref(p).to_string()
}

/// The display name, borrowed, so a rules `Facts` does not allocate it once
/// per pid per tick.
pub(crate) fn name_ref(p: &Process) -> &str {
    if let Some(exe) = &p.exe {
        let b = basename(exe);
        // tdeinit runs konsole, kicker or kate as a module in a fork, so `exe`
        // stays tdeinit for all of them and only `comm` (set by prctl) names
        // the program. tdelibs `tdeinit.cpp` `launch()`.
        if b == "tdeinit" && !p.comm.is_empty() {
            return &p.comm;
        }
        if !b.is_empty() && b != "exe" && !b.starts_with('[') {
            return b;
        }
    }
    &p.comm
}

/// `s` without `suffix`, compared ASCII case-insensitively. A cut that lands
/// inside a multibyte character is a miss, not a panic.
fn strip_suffix_ignore_ascii_case<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let (stem, tail) = s.split_at_checked(s.len().checked_sub(suffix.len())?)?;
    tail.eq_ignore_ascii_case(suffix).then_some(stem)
}

/// Firefox/Chromium crash helper whose parent is often user systemd.
/// `classes` is the process's own set; the helper names are the
/// `crash_helper` class, asked of each path basename through `rules`.
///
/// The install directory names the app, unless no process in `procs` is
/// called that while a non-helper whose `exe` sits in the same directory is:
/// an app row is named after its binary (`/opt/vivaldi/vivaldi-bin`), not its
/// directory. Several candidates take the lowest pid, the one started first.
/// A helper whose directory is a per-run `.mount_…` directory has no name to
/// fall back on, so only such a sibling names its app.
/// Candidates are the helper uid's processes outside `system.slice` and any
/// container or machine scope. A candidate is judged by `classes_of`, its full
/// class set, since a `crash_helper` rule may test `exe` or argv, which a bare
/// name lookup never sees.
pub(crate) fn crash_helper_app<'p>(
    p: &Process,
    classes: Classes,
    rules: &Rules,
    classes_of: impl Fn(&Process) -> Classes,
    procs: impl IntoIterator<Item = &'p Process>,
) -> Option<String> {
    if !classes.intersects(Classes::CRASH_HELPER) {
        return None;
    }
    let (owner, dir) = p
        .exe
        .iter()
        .chain(p.cmdline.iter())
        .find_map(|s| app_from_crash_helper_path(s, rules))?;
    let mut sibling: Option<&Process> = None;
    for q in procs.into_iter().filter(|q| in_own_session(p, q)) {
        if owner.as_deref() == Some(name_ref(q)) {
            return owner;
        }
        if q.exe
            .as_deref()
            .and_then(|e| e.rsplit_once('/'))
            .is_some_and(|(d, _)| d == dir)
            && !classes_of(q).intersects(Classes::CRASH_HELPER)
            && sibling.is_none_or(|s| q.pid < s.pid)
        {
            sibling = Some(q);
        }
    }
    sibling.map(name_of).or(owner)
}

/// Whether `q` is one of `p`'s user's session processes, the only ones that can
/// be the app a helper or launcher of `p`'s bills to: a root daemon, a
/// container or another user's app running from the same directory would
/// otherwise take the row by pid order.
pub(crate) fn in_own_session(p: &Process, q: &Process) -> bool {
    q.uid == p.uid
        && (!identity::in_system_slice(&q.cgroup) || identity::in_user_slice(&q.cgroup))
        && identity::docker_scope_id(&q.cgroup).is_none()
        && identity::machine_scope_name(&q.cgroup).is_none()
}

/// The owning app the directory names, if it names one, and the directory the
/// helper was started from.
fn app_from_crash_helper_path<'s>(s: &'s str, rules: &Rules) -> Option<(Option<String>, &'s str)> {
    let lower = s.to_ascii_lowercase();
    if lower.contains("/firefox/") || lower.ends_with("/firefox") {
        return Some((
            Some("firefox".to_string()),
            s.rsplit_once('/').map_or("", |(d, _)| d),
        ));
    }
    if !rules
        .classes_of_name(basename(s))
        .intersects(Classes::CRASH_HELPER)
    {
        return None;
    }
    let dir = s.rsplit_once('/')?.0;
    let owner = basename(dir);
    if owner.is_empty() || matches!(owner, "bin" | "libexec" | "lib" | "lib64") {
        return None;
    }
    // AppImage mounts are per-run (`/tmp/mount`, `/tmp/.mount_cursorAb12Cd`)
    // and never an identity. A stable directory nested under the mount
    // (`…/usr/share/cursor/chrome_crashpad_handler`) is the same owner
    // `/opt/cursor/…` would name. When the parent dir *is* the mount, a
    // non-helper beside the helper can still name the app; with none, `group`
    // walks PPID.
    if is_temp_unpack_root(&lower) && is_ephemeral_mount_dir(owner) {
        return Some((None, dir));
    }
    Some((Some(owner.to_string()), dir))
}

fn is_temp_unpack_root(lower: &str) -> bool {
    ["/tmp/", "/var/tmp/", "/run/"]
        .iter()
        .any(|root| lower.starts_with(root))
}

fn is_ephemeral_mount_dir(owner: &str) -> bool {
    owner.eq_ignore_ascii_case("mount")
        || owner.eq_ignore_ascii_case("appimage")
        || owner
            .get(..".mount".len())
            .is_some_and(|head| head.eq_ignore_ascii_case(".mount"))
}

/// Interpreters fold into Electron/browser parents, never into a shell or systemd.
///
/// The raw parent process's classes, deliberately not the resolved Place `key`
/// that `group::compute_place` tests: this runs before the parent is placed,
/// so a process whose identity later folds elsewhere is still judged here by
/// what it actually is.
pub(crate) fn absorbs_generic(parent: Classes) -> bool {
    !parent.intersects(
        Classes::GENERIC
            | Classes::LAUNCHER
            | Classes::SHELL
            | Classes::TERMINAL
            | Classes::COMPOSITOR
            | Classes::NO_ABSORB,
    )
}

pub(crate) fn is_interactive_shell(p: &Process, classes: Classes) -> bool {
    if !classes.intersects(Classes::SHELL) {
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

pub(crate) fn launcher_payload_hint(p: &Process, rules: &Rules) -> Option<String> {
    // Case-folded the way the `launchers` class rule folds it: real AppImages ship as
    // `Cursor-x86_64.AppImage`, and `name_ref` does not lowercase. A launcher
    // that yields no hint falls through to `user_place` and takes the
    // top-level row launchers are not supposed to have. The stem keeps its
    // own casing because it becomes the displayed identity.
    if let Some(stem) = strip_suffix_ignore_ascii_case(name_ref(p), ".appimage")
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
            // "child" is a zypak-helper subcommand, not a payload.
            if b.is_empty()
                || rules.classes_of_name(b).intersects(Classes::LAUNCHER)
                || b.eq_ignore_ascii_case("child")
            {
                continue;
            }
            return Some(b.to_string());
        }
    }
    None
}

/// The directory name prefix an `AppImage` runtime mounts its payload under:
/// `.mount_` and the first six bytes of the file name, followed by six random
/// characters, in `$TMPDIR` or `/tmp` (`grok_bot.appimage` mounts
/// `/tmp/.mount_grok_bHb2bit`). A payload that reparented to user systemd is
/// tied to its launcher by nothing else.
///
/// The random half is not visible from the launcher, so two `AppImage`s whose
/// file names share their first six bytes, run by one user at once, both bill
/// to the lower pid's app. A file name shorter than six bytes names no prefix.
pub(crate) fn appimage_mount_prefix(launcher: &Process) -> Option<String> {
    let name = name_ref(launcher);
    strip_suffix_ignore_ascii_case(name, ".appimage")?;
    Some(format!(".mount_{}", name.get(..6)?))
}

/// Whether `exe` runs from a temp mount named by `appimage_mount_prefix`.
pub(crate) fn in_appimage_mount(exe: &str, prefix: &str) -> bool {
    exe.split('/')
        .any(|c| c.len() == prefix.len() + 6 && c.starts_with(prefix))
        && is_temp_unpack_root(&exe.to_ascii_lowercase())
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
                return Some(b.to_string());
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

    /// A process's class set from every fact, the lookup `group` passes.
    fn full(rules: &Rules) -> impl Fn(&Process) -> Classes + '_ {
        |q| rules.classes(&crate::group::facts_of(q, None))
    }

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
        assert_eq!(
            launcher_payload_hint(&cursor, &Rules::builtin()).as_deref(),
            Some("Cursor-x86_64"),
            "a launcher with no hint takes a top-level row of its own"
        );
        let named = |name: &str| Process {
            comm: name.into(),
            exe: Some(format!("/opt/{name}")),
            ..Process::default()
        };
        assert_eq!(
            launcher_payload_hint(&named("Curseé.AppImage"), &Rules::builtin()).as_deref(),
            Some("Curseé")
        );
        assert_eq!(
            launcher_payload_hint(&named("éAppImage"), &Rules::builtin()),
            None,
            "a suffix cut inside a multibyte character is a miss"
        );
    }

    #[test]
    fn interactive_shells() {
        let shell = Classes::SHELL;
        assert!(is_interactive_shell(&p("bash", &["-bash"]), shell));
        assert!(
            is_interactive_shell(&p("bash", &["/bin/bash", "--posix"]), shell),
            "ghostty's login bash is interactive"
        );
        let wrapper = p(
            "bash",
            &["bash", "/app/bin/zypak-wrapper", "/app/extra/vscode/code"],
        );
        assert!(!is_interactive_shell(&wrapper, shell));
        assert!(!is_interactive_shell(
            &p("bashful", &["bashful"]),
            Classes::default()
        ));
    }

    #[test]
    fn a_crash_helper_takes_the_binary_beside_it_when_its_directory_names_no_process() {
        let rules = Rules::builtin();
        let helper = Classes::CRASH_HELPER | Classes::WORKER;
        let at = |pid, exe: &str| Process {
            pid,
            comm: basename(exe).into(),
            exe: Some(exe.into()),
            ..Process::default()
        };
        let handler = at(20, "/opt/foo/chrome_crashpad_handler");
        let mut procs = vec![
            handler.clone(),
            at(9, "/opt/foo/foo-bin"),
            at(3, "/opt/foo/foo-helper"),
            at(1, "/opt/bar/bar"),
        ];
        assert_eq!(
            crash_helper_app(&handler, helper, &rules, full(&rules), &procs).as_deref(),
            Some("foo-helper"),
            "no process is called foo, so the lowest pid beside the handler names it"
        );
        procs.push(at(30, "/usr/local/bin/foo"));
        assert_eq!(
            crash_helper_app(&handler, helper, &rules, full(&rules), &procs).as_deref(),
            Some("foo"),
            "a process named after the directory keeps the directory's name"
        );
    }

    #[test]
    fn a_crash_helper_in_the_mount_dir_takes_its_sibling_app() {
        let rules = Rules::builtin();
        let helper = Classes::CRASH_HELPER | Classes::WORKER;
        let at = |pid, exe: &str| Process {
            pid,
            comm: basename(exe).into(),
            exe: Some(exe.into()),
            ..Process::default()
        };
        let handler = at(320, "/tmp/.mount_grok_bHb2bit/chrome_crashpad_handler");
        let procs = [
            handler.clone(),
            at(310, "/tmp/.mount_grok_bHb2bit/grok-bot"),
        ];
        assert_eq!(
            crash_helper_app(&handler, helper, &rules, full(&rules), &procs).as_deref(),
            Some("grok-bot")
        );
        assert_eq!(
            crash_helper_app(&handler, helper, &rules, full(&rules), &procs[..1]),
            None,
            "with no sibling the mount names nothing, and group walks PPID"
        );
    }

    #[test]
    fn a_sibling_whose_exe_is_a_crash_helper_does_not_name_the_app() {
        use crate::rules::{LoadedFile, Source};

        let mut files = vec![LoadedFile {
            source: Source {
                rank: 0,
                label: "xdg".into(),
            },
            name: "90-mine.json".into(),
            text: r#"{"stage": "class", "rules": [{"id": "dumper", "match": {"exe_suffix": "/foo-dump"}, "classes": ["crash_helper"]}]}"#.into(),
        }];
        files.extend(crate::rules::builtin_files());
        let rules = Rules::from_files(&files);
        assert!(rules.problems.is_empty(), "{:?}", rules.problems);
        let at = |pid, exe: &str| Process {
            pid,
            comm: basename(exe).into(),
            exe: Some(exe.into()),
            ..Process::default()
        };
        let handler = at(20, "/opt/foo/chrome_crashpad_handler");
        let procs = [
            handler.clone(),
            at(3, "/opt/foo/foo-dump"),
            at(9, "/opt/foo/foo-bin"),
        ];
        assert_eq!(
            crash_helper_app(
                &handler,
                Classes::CRASH_HELPER,
                &rules,
                full(&rules),
                &procs
            )
            .as_deref(),
            Some("foo-bin"),
            "the lower pid is a crash helper by its exe, so it cannot be the app"
        );
    }

    #[test]
    fn crash_helpers_name_their_app() {
        const NONE: [&Process; 0] = [];
        let rules = Rules::builtin();
        let helper = Classes::CRASH_HELPER | Classes::WORKER;
        assert_eq!(
            crash_helper_app(
                &Process {
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
                },
                helper,
                &rules,
                full(&rules),
                NONE
            )
            .as_deref(),
            Some("firefox")
        );
        assert_eq!(
            crash_helper_app(
                &p(
                    "chrome_crashpad_handler",
                    &["/opt/cursor/chrome_crashpad_handler"]
                ),
                helper,
                &rules,
                full(&rules),
                NONE
            )
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
                crash_helper_app(
                    &Process {
                        comm: "chrome_crashpad_handler".into(),
                        exe: Some(temp.into()),
                        cmdline: vec![temp.into()],
                        ..Process::default()
                    },
                    helper,
                    &rules,
                    full(&rules),
                    NONE
                ),
                None,
                "a temp mount directory is not an app identity: {temp}"
            );
        }
        let multibyte = "/tmp/mounté/chrome_crashpad_handler";
        assert_eq!(
            crash_helper_app(
                &Process {
                    comm: "chrome_crashpad_handler".into(),
                    exe: Some(multibyte.into()),
                    cmdline: vec![multibyte.into()],
                    ..Process::default()
                },
                helper,
                &rules,
                full(&rules),
                NONE
            )
            .as_deref(),
            Some("mounté"),
            "a prefix cut inside a multibyte character is a miss"
        );
        assert_eq!(
            crash_helper_app(
                &Process {
                    comm: "chrome_crashpad".into(),
                    exe: Some(
                        "/tmp/.mount_cursorIDenmC/usr/share/cursor/chrome_crashpad_handler".into()
                    ),
                    cmdline: vec![
                        "/tmp/.mount_cursorIDenmC/usr/share/cursor/chrome_crashpad_handler".into()
                    ],
                    ppid: 6475,
                    ..Process::default()
                },
                helper,
                &rules,
                full(&rules),
                NONE
            )
            .as_deref(),
            Some("cursor"),
            "a nested owner under an AppImage mount still names the app"
        );
    }

    #[test]
    fn launcher_payload_hints_through_bwrap() {
        let rules = Rules::builtin();
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
        assert_eq!(
            launcher_payload_hint(&zypak, &rules).as_deref(),
            Some("code")
        );
        assert_eq!(
            launcher_payload_hint(
                &p("bwrap", &["/usr/bin/bwrap", "--", "/usr/bin/flatpak"]),
                &rules
            )
            .as_deref(),
            None
        );
    }
}
