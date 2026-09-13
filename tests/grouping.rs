use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use heft::containers::{ContainerIndex, Inspect, ListItem};
use heft::group::build_tree;
use heft::rules::{LoadedFile, Rules, Source};
use heft::types::Process;
use heft::{HostHeader, HostTree};

#[derive(serde::Deserialize)]
struct Fixture {
    nproc: u32,
    clk_tck: u64,
    page_size: u64,
    #[serde(default)]
    workdir_uids: HashMap<PathBuf, u32>,
    #[serde(default)]
    containers: Vec<ListItem>,
    #[serde(default)]
    inspects: HashMap<String, Inspect>,
    processes: Vec<ProcFix>,
}

#[derive(serde::Deserialize)]
struct ProcFix {
    pid: u32,
    ppid: u32,
    #[serde(default)]
    pgrp: i32,
    uid: u32,
    #[serde(default)]
    kthread: bool,
    comm: String,
    exe: Option<String>,
    #[serde(default)]
    cmdline: Vec<String>,
    cgroup: String,
    #[serde(default)]
    utime: u64,
    #[serde(default)]
    stime: u64,
}

const GUI: &str = "tests/fixtures/gui/world.json";

fn load(path: &str, rules: &Rules) -> (HashMap<u32, Process>, ContainerIndex, HostHeader) {
    let text = std::fs::read_to_string(path).unwrap();
    let fix: Fixture = serde_json::from_str(&text).unwrap();
    let mut curr = HashMap::new();
    for p in fix.processes {
        curr.insert(
            p.pid,
            Process {
                pid: p.pid,
                ppid: p.ppid,
                pgrp: if p.pgrp == 0 {
                    i32::try_from(p.pid).expect("fixture pid fits i32")
                } else {
                    p.pgrp
                },
                uid: p.uid,
                kthread: p.kthread,
                comm: p.comm,
                exe: p.exe,
                cmdline: p.cmdline,
                cgroup: p.cgroup,
                utime: p.utime,
                stime: p.stime,
                ..Process::default()
            },
        );
    }
    let idx = ContainerIndex::from_list(&fix.containers, &fix.inspects, &fix.workdir_uids, rules);
    let header = HostHeader {
        nproc: fix.nproc,
        clk_tck: fix.clk_tck,
        page_size: fix.page_size,
    };
    (curr, idx, header)
}

fn tree_of(path: &str, rules: &Rules) -> HostTree {
    let (curr, idx, header) = load(path, rules);
    build_tree(
        &curr,
        &curr,
        Duration::from_secs(1),
        &header,
        HostTree::default(),
        &idx,
        rules,
    )
}

/// One user file at the XDG rank plus the built-ins, the set `rules::load`
/// builds when `$XDG_CONFIG_HOME/heft/rules.d/90-mine.json` exists.
fn with_user(text: &str) -> Rules {
    let mut files = vec![LoadedFile {
        source: Source {
            rank: 0,
            label: "xdg".into(),
        },
        name: "90-mine.json".into(),
        text: text.into(),
    }];
    files.extend(heft::rules::builtin_files());
    let r = Rules::from_files(files);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    r
}

fn titles(nodes: &[heft::IdentNode]) -> Vec<String> {
    let mut v: Vec<String> = nodes.iter().map(|n| n.title.clone()).collect();
    v.sort();
    v
}

fn has(nodes: &[heft::IdentNode], name: &str) -> bool {
    nodes.iter().any(|n| n.id == name || n.title == name)
}

fn proc_names(n: &heft::IdentNode) -> Vec<String> {
    fn walk(procs: &[heft::ProcNode], out: &mut Vec<String>) {
        for p in procs {
            out.push(p.name.clone());
            walk(&p.children, out);
        }
    }
    let mut v = Vec::new();
    for inst in &n.instances {
        walk(&inst.processes, &mut v);
    }
    v
}

/// A `--fixture` dump attached to a report has to load here unedited, or the
/// report cannot become a test.
#[test]
fn a_fixture_dump_loads_as_a_fixture() {
    let path = std::env::temp_dir().join(format!("heft-fixture-{}.json", std::process::id()));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_heft"))
        .arg("--fixture")
        .output()
        .expect("run heft --fixture");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::write(&path, &out.stdout).unwrap();
    let tree = tree_of(path.to_str().unwrap(), &Rules::builtin());
    std::fs::remove_file(&path).unwrap();
    assert!(
        !tree.users.is_empty(),
        "this test's own process is a user process, so a loaded dump has a User"
    );
}

#[test]
fn gui_and_docker_fixture() {
    let tree = tree_of(GUI, &Rules::builtin());
    let user = tree.users.iter().find(|u| u.uid == 1000).expect("uid 1000");

    for name in [
        "ghostty",
        "chrome",
        "claude",
        "code",
        "cursor",
        "easyeffects",
        "soffice.bin",
        "spotify",
        "vivaldi-bin",
    ] {
        assert!(
            has(&user.applications, name),
            "Applications missing {name}: {:?}",
            titles(&user.applications)
        );
    }
    assert!(
        !has(&user.applications, "bash"),
        "interactive bash bills to ghostty or to the app it launched: {:?}",
        titles(&user.applications)
    );
    assert!(
        !has(&user.applications, "bunx"),
        "bunx must bill to payload"
    );
    assert!(
        !has(&user.applications, "bwrap"),
        "bwrap must bill to payload"
    );
    assert!(
        !has(&user.applications, "zypak-wrapper"),
        "the flatpak zypak wrapper script is a launcher: {:?}",
        titles(&user.applications)
    );
    assert!(
        !has(&user.applications, "cat"),
        "cat session noise must not be an Applications row: {:?}",
        titles(&user.applications)
    );
    assert!(
        !has(&user.applications, "child"),
        "zypak-helper child is not an identity: {:?}",
        titles(&user.applications)
    );
    assert!(
        !has(&user.applications, "nautilus"),
        "an app in another app's dbus scope keeps its own name"
    );
    assert!(
        !has(&user.applications, "java"),
        "a JVM child bills to the app that spawned it: {:?}",
        titles(&user.applications)
    );
    assert!(!has(&user.applications, "cursor.appimage"));
    assert!(
        !has(&user.applications, "context7-mcp"),
        "MCP servers fold into the launching agent: {:?}",
        titles(&user.applications)
    );
    assert!(
        !has(&user.applications, "npm"),
        "npm exec under claude must not be an Applications row: {:?}",
        titles(&user.applications)
    );
    assert!(
        !has(&user.applications, "shadcn"),
        "node MCP under cursor folds into cursor: {:?}",
        titles(&user.applications)
    );
    for name in [
        "gdm-wayland-session",
        "gnome-session-init-worker",
        "gnome-shell-calendar-server",
        "gnome-keyring-daemon",
        "gnome-calendar",
        "gnome-clocks",
        "dbus-broker",
        "dbus-broker-launch",
        "gjs-console",
        "gjs",
        "ibus-portal",
        "at-spi2-registryd",
        "goa-identity-service",
        "goa-daemon",
        "p11-kit-server",
        "p11-kit-remote",
        "p11-kit",
        "gsd-disk-utility-notify",
        "gsd-color",
        "gsd-power",
        "gvfsd",
        "gvfsd-trash",
        "gvfs-goa-volume-monitor",
        "wsdd",
        "abrt-applet",
        "crashhelper",
        "flatpak-session-helper",
        "flatpak-portal",
        "xdg-desktop-portal",
        "xdg-desktop-portal-gnome",
        "xdg-document-portal",
        "evolution-addressbook-factory",
        "pipewire",
        "wireplumber",
        "ibus-dconf",
        "ibus-x11",
        "Xwayland",
        "gcr-ssh-agent",
        "ssh-agent",
    ] {
        assert!(
            !has(&user.applications, name),
            "{name} must not be Applications: {:?}",
            titles(&user.applications)
        );
    }

    let claude = user
        .applications
        .iter()
        .find(|n| n.id == "claude" || n.title == "claude")
        .expect("claude");
    let claude_procs = proc_names(claude);
    assert!(
        claude_procs
            .iter()
            .any(|n| n == "node-24" || n == "MainThread" || n == "node"),
        "context7-mcp process must remain visible under claude: {claude_procs:?}"
    );
    assert!(
        claude_procs.iter().any(|n| n == "bash"),
        "the bash that launched claude bills to claude, not ghostty: {claude_procs:?}"
    );

    assert!(has(&user.user_services, "node-red"));
    assert!(has(&user.user_services, "homebridge"));
    assert!(has(&user.user_services, "gnome-shell"));
    assert!(has(&user.user_services, "dbus-broker"));
    assert!(has(&user.user_services, "ibus-daemon"));
    assert!(has(&user.user_services, "at-spi-bus-launcher"));
    assert!(has(&user.user_services, "goa-daemon"));
    assert!(has(&user.user_services, "p11-kit"));
    assert!(has(&user.user_services, "gsd-disk-utility-notify"));
    assert!(has(&user.user_services, "gnome-settings-daemon"));
    assert!(has(&user.user_services, "gvfs"));
    assert!(has(&user.user_services, "flatpak"));
    assert!(has(&user.user_services, "xdg-desktop-portal"));
    assert!(has(&user.user_services, "evolution-data-server"));
    assert!(has(&user.user_services, "pipewire"));
    assert!(has(&user.user_services, "wireplumber"));
    assert!(has(&user.user_services, "gcr-ssh-agent"));
    assert!(has(&user.user_services, "abrt-applet"));
    assert!(!has(&user.user_services, "gsd-color"));
    assert!(!has(&user.user_services, "gsd-power"));
    assert!(!has(&user.user_services, "gvfsd"));
    assert!(!has(&user.user_services, "gvfs-goa-volume-monitor"));
    assert!(!has(&user.user_services, "flatpak-session-helper"));
    assert!(!has(&user.user_services, "flatpak-portal"));
    assert!(!has(&user.user_services, "xdg-desktop-portal-gnome"));
    assert!(!has(&user.user_services, "evolution-addressbook-factory"));
    assert!(!has(&user.user_services, "pipewire-pulse"));
    assert!(!has(&user.user_services, "ibus-dconf"));
    assert!(!has(&user.user_services, "ibus-x11"));
    assert!(!has(&user.user_services, "ssh-agent"));
    assert!(!has(&user.user_services, "gnome-calendar"));
    assert!(!has(&user.user_services, "gnome-clocks"));
    assert!(!has(&user.user_services, "ghostty"));
    assert!(!has(&user.user_services, "gnome-abrt"));
    assert!(!has(&user.user_services, "ibus-portal"));
    assert!(!has(&user.user_services, "at-spi2-registryd"));
    assert!(!has(&user.user_services, "goa-identity-service"));
    assert!(!has(&user.user_services, "p11-kit-server"));
    assert!(!has(&user.user_services, "gjs-console"));
    assert!(
        !has(&user.user_services, "cursor"),
        "cursor in a helper service must stay Applications: {:?}",
        titles(&user.user_services)
    );
    assert!(
        !has(&user.user_services, "firefox"),
        "firefox must stay Applications: {:?}",
        titles(&user.user_services)
    );
    assert!(has(&user.applications, "firefox"));
    assert!(
        !has(&user.user_services, "vivaldi-bin")
            && !has(&user.user_services, "claude")
            && !has(&user.user_services, "code"),
        "independent apps must not fold into User Services: {:?}",
        titles(&user.user_services)
    );

    let gnome = user
        .user_services
        .iter()
        .find(|n| n.id == "gnome-shell" || n.title == "gnome-shell")
        .expect("gnome-shell");
    let gnome_procs = proc_names(gnome);
    assert!(
        gnome_procs.iter().any(|n| n == "gjs-console" || n == "gjs"),
        "gjs-console gnome-shell backends must remain visible under gnome-shell: {gnome_procs:?}"
    );
    assert!(
        gnome_procs.iter().any(|n| n == "Xwayland"),
        "Xwayland must remain visible under gnome-shell: {gnome_procs:?}"
    );

    let ibus = user
        .user_services
        .iter()
        .find(|n| n.id == "ibus-daemon")
        .expect("ibus-daemon");
    let ibus_procs = proc_names(ibus);
    assert!(
        ibus_procs.iter().any(|n| n == "ibus-portal")
            && ibus_procs.iter().any(|n| n == "ibus-dconf")
            && ibus_procs.iter().any(|n| n == "ibus-x11"),
        "ibus helpers including ibus-x11 (lying GSD unit) fold into ibus-daemon: {ibus_procs:?}"
    );

    let atspi = user
        .user_services
        .iter()
        .find(|n| n.id == "at-spi-bus-launcher")
        .expect("at-spi-bus-launcher");
    assert!(
        proc_names(atspi).iter().any(|n| n == "at-spi2-registryd"),
        "at-spi2-registryd must remain visible under at-spi-bus-launcher: {:?}",
        proc_names(atspi)
    );

    let goa = user
        .user_services
        .iter()
        .find(|n| n.id == "goa-daemon")
        .expect("goa-daemon");
    let goa_procs = proc_names(goa);
    assert!(
        goa_procs.iter().any(|n| n == "goa-identity-service"),
        "goa-identity-service must remain visible under goa-daemon: {goa_procs:?}"
    );
    assert!(
        !goa_procs.iter().any(|n| n.contains("gvfs")),
        "gvfs-goa-volume-monitor must not bill to goa-daemon: {goa_procs:?}"
    );

    let p11 = user
        .user_services
        .iter()
        .find(|n| n.id == "p11-kit")
        .expect("p11-kit");
    let p11_procs = proc_names(p11);
    assert!(
        p11_procs.iter().any(|n| n == "p11-kit-server")
            && p11_procs.iter().any(|n| n == "p11-kit-remote"),
        "p11-kit server and remote must share one identity: {p11_procs:?}"
    );
    let gsd = user
        .user_services
        .iter()
        .find(|n| n.id == "gnome-settings-daemon")
        .expect("gnome-settings-daemon");
    let gsd_procs = proc_names(gsd);
    assert!(
        gsd_procs.iter().any(|n| n == "gsd-color") && gsd_procs.iter().any(|n| n == "gsd-power"),
        "gsd plugins must remain visible under gnome-settings-daemon: {gsd_procs:?}"
    );
    assert!(
        !gsd_procs.iter().any(|n| n.contains("disk-utility")),
        "disk-utility-notify is not gnome-settings-daemon: {gsd_procs:?}"
    );
    let gvfs = user
        .user_services
        .iter()
        .find(|n| n.id == "gvfs")
        .expect("gvfs");
    let gvfs_procs = proc_names(gvfs);
    assert!(
        gvfs_procs.iter().any(|n| n == "gvfsd")
            && gvfs_procs.iter().any(|n| n == "gvfsd-trash")
            && gvfs_procs.iter().any(|n| n == "gvfs-goa-volume-monitor")
            && gvfs_procs.iter().any(|n| n == "wsdd" || n == "python3"),
        "gvfs stack must remain visible under gvfs: {gvfs_procs:?}"
    );
    let flatpak = user
        .user_services
        .iter()
        .find(|n| n.id == "flatpak")
        .expect("flatpak");
    let flatpak_procs = proc_names(flatpak);
    assert!(
        flatpak_procs.iter().any(|n| n == "flatpak-session-helper")
            && flatpak_procs.iter().any(|n| n == "flatpak-portal")
            && flatpak_procs.iter().any(|n| n == "xdg-dbus-proxy"),
        "Flatpak session infra must remain visible under flatpak: {flatpak_procs:?}"
    );
    assert!(
        !flatpak_procs
            .iter()
            .any(|n| n == "cursor" || n.starts_with("p11-kit")),
        "Cursor/p11-kit must not bill to flatpak: {flatpak_procs:?}"
    );
    let portal = user
        .user_services
        .iter()
        .find(|n| n.id == "xdg-desktop-portal")
        .expect("xdg-desktop-portal");
    let portal_procs = proc_names(portal);
    assert!(
        portal_procs.iter().any(|n| n == "xdg-desktop-portal")
            && portal_procs.iter().any(|n| n == "xdg-desktop-portal-gnome")
            && portal_procs.iter().any(|n| n == "xdg-document-portal"),
        "portal backends must remain visible under xdg-desktop-portal: {portal_procs:?}"
    );
    let eds = user
        .user_services
        .iter()
        .find(|n| n.id == "evolution-data-server")
        .expect("evolution-data-server");
    let eds_procs = proc_names(eds);
    assert!(
        eds_procs
            .iter()
            .any(|n| n == "evolution-addressbook-factory")
            && eds_procs.iter().any(|n| n == "evolution-calendar-factory"),
        "EDS factories must remain visible: {eds_procs:?}"
    );
    let pw = user
        .user_services
        .iter()
        .find(|n| n.id == "pipewire")
        .expect("pipewire");
    assert!(
        proc_names(pw).iter().any(|n| n == "pipewire"),
        "pipewire-pulse bills to pipewire (exe basename): {:?}",
        proc_names(pw)
    );
    assert!(has(&user.applications, "htop"));
    assert!(!has(&user.user_services, "htop"));
    let code = user
        .applications
        .iter()
        .find(|n| n.id == "code" || n.title == "code")
        .expect("code");
    assert!(
        proc_names(code).iter().any(|n| n == "xdg-dbus-proxy"),
        "app-bound xdg-dbus-proxy bills to the flatpak app: {:?}",
        proc_names(code)
    );
    assert!(
        proc_names(code).iter().filter(|n| *n == "cat").count() == 2,
        "pipe helpers under the flatpak launcher bill to the payload: {:?}",
        proc_names(code)
    );
    let cursor = user
        .applications
        .iter()
        .find(|n| n.id == "cursor" || n.title == "cursor")
        .expect("cursor");
    assert!(
        !proc_names(cursor).iter().any(|n| n.starts_with("p11-kit")),
        "p11-kit must not bill to Cursor: {:?}",
        proc_names(cursor)
    );
    assert_eq!(
        proc_names(cursor)
            .iter()
            .filter(|n| *n == "chrome_crashpad_handler")
            .count(),
        2,
        "both AppImage crash helpers bill to Cursor, including the one reparented to user systemd: {:?}",
        proc_names(cursor)
    );
    assert!(
        !has(&user.applications, "chrome_crashpad_handler"),
        "a crash helper is never its own application row: {:?}",
        titles(&user.applications)
    );
    assert!(
        !has(&user.applications, "mount"),
        "a temp mount directory is never an app identity: {:?}",
        titles(&user.applications)
    );

    let firefox = user
        .applications
        .iter()
        .find(|n| n.id == "firefox" || n.title == "firefox")
        .expect("firefox");
    assert!(
        proc_names(firefox).iter().any(|n| n == "crashhelper"),
        "crashhelper must remain visible under firefox: {:?}",
        proc_names(firefox)
    );
    assert!(
        !has(&user.user_services, "cat"),
        "cat under an app must not leak into User Services: {:?}",
        titles(&user.user_services)
    );

    assert!(has(&user.containers, "supabase:demo"));
    assert!(has(&user.containers, "acme-encoder"));
    assert!(has(&user.containers, "acme-indexer"));
    assert!(has(&user.containers, "acme-thumbnailer"));
    assert!(
        has(&user.containers, "spec-runner-7"),
        "a container named nothing like its siblings bills to its bind-mount owner: {:?}",
        titles(&user.containers)
    );
    assert!(
        !has(&tree.containers, "spec-runner-7"),
        "an attributed container must not also sit on Host: {:?}",
        titles(&tree.containers)
    );
    assert!(!has(&user.containers, "acme"));
    assert!(!has(&user.containers, "dockerd"));
    assert!(!has(&user.containers, "containerd"));

    let demo = user
        .containers
        .iter()
        .find(|c| c.id == "supabase:demo")
        .unwrap();
    assert!(
        demo.containers.len() >= 2,
        "project should expand to member containers"
    );

    // Kernel threads sit on cgroup `0::/`, in neither slice, so System is
    // reachable for them only through the PF_KTHREAD flag.
    assert!(
        has(&tree.system, "kernel"),
        "kthreadd and kworker must bucket to System: {:?}",
        titles(&tree.system)
    );
    assert!(has(&tree.system, "dockerd"));
    assert!(has(&tree.system, "containerd"));
    assert!(!has(&tree.system, "acme-encoder"));
    assert!(!has(&tree.system, "supabase:demo"));
    assert!(!tree.system.iter().any(|n| n.title.starts_with("docker-")));
}

#[test]
fn idle_interactive_bash_under_ghostty_bills_to_ghostty() {
    let tree = tree_of(GUI, &Rules::builtin());
    let user = user_of(&tree, 1000);
    assert!(
        !has(&user.applications, "bash"),
        "idle --posix bash must not be an Applications row: {:?}",
        titles(&user.applications)
    );
    let ghostty = user
        .applications
        .iter()
        .find(|n| n.id == "ghostty")
        .expect("ghostty");
    assert!(
        proc_names(ghostty).iter().any(|n| n == "bash"),
        "idle bash must remain visible under ghostty: {:?}",
        proc_names(ghostty)
    );
}

#[test]
fn claude_under_bash_under_ghostty_owns_the_shell() {
    let tree = tree_of(GUI, &Rules::builtin());
    let user = user_of(&tree, 1000);
    let claude = user
        .applications
        .iter()
        .find(|n| n.id == "claude")
        .expect("claude");
    let claude_procs = proc_names(claude);
    assert!(
        claude_procs.iter().any(|n| n == "bash")
            && claude_procs.iter().any(|n| n == "bun" || n == "bunx"),
        "launching bash and bunx bill to claude, not only ghostty: {claude_procs:?}"
    );
    assert!(
        has(&user.applications, "claude") && has(&user.applications, "ghostty"),
        "claude stays its own Applications row beside ghostty: {:?}",
        titles(&user.applications)
    );
}

fn user_of(tree: &HostTree, uid: u32) -> &heft::UserNode {
    tree.users
        .iter()
        .find(|u| u.uid == uid)
        .unwrap_or_else(|| panic!("uid {uid}"))
}

#[test]
fn a_lone_crash_helper_under_a_launcher_bills_to_its_app() {
    // The launcher's only child is a crash helper with no children of its own,
    // so the zygote fallback in `unique_descendant_ident` decides the identity.
    // The helper's own path names the app, which is what every other placement
    // site would use.
    let tree = tree_of("tests/fixtures/zygote/world.json", &Rules::builtin());
    let user = user_of(&tree, 1000);
    assert!(
        has(&user.applications, "firefox"),
        "expected the helper to bill to firefox, got: {:?}",
        titles(&user.applications)
    );
    assert!(
        !has(&user.applications, "crashhelper"),
        "a crash helper is never its own application row: {:?}",
        titles(&user.applications)
    );
}

#[test]
fn one_override_moves_one_row_and_leaves_the_rest_alone() {
    let base = tree_of(GUI, &Rules::builtin());
    let ov = with_user(
        r#"{"stage":"placement","rules":[{"id":"pin-htop","match":{"identity":"htop"},"folder":"user_services"}]}"#,
    );
    let pinned = tree_of(GUI, &ov);

    let (b, p) = (user_of(&base, 1000), user_of(&pinned, 1000));
    assert!(has(&b.applications, "htop") && !has(&b.user_services, "htop"));
    assert!(has(&p.user_services, "htop") && !has(&p.applications, "htop"));

    let drop_htop =
        |v: Vec<String>| -> Vec<String> { v.into_iter().filter(|t| t != "htop").collect() };
    assert_eq!(drop_htop(titles(&b.applications)), titles(&p.applications));
    assert_eq!(
        titles(&b.user_services),
        drop_htop(titles(&p.user_services))
    );
    assert_eq!(titles(&b.containers), titles(&p.containers));
    assert_eq!(titles(&base.containers), titles(&pinned.containers));
    assert_eq!(titles(&base.system), titles(&pinned.system));
}

#[test]
fn fold_bills_a_named_process_to_another_identity() {
    let ov = with_user(
        r#"{"stage":"placement","rules":[{"id":"fold-spotify","match":{"identity":"spotify"},"fold_to":"media"}]}"#,
    );
    let tree = tree_of(GUI, &ov);
    let user = &user_of(&tree, 1000).applications;
    assert!(!has(user, "spotify"), "{:?}", titles(user));
    let media = user.iter().find(|n| n.id == "media").expect("media");
    assert!(
        proc_names(media).iter().any(|n| n == "spotify"),
        "{:?}",
        proc_names(media)
    );
}

#[test]
fn a_malformed_override_file_is_rejected_rather_than_obeyed() {
    // The loader turns each of these into a stderr warning and built-in
    // behaviour; parsing is where the file is judged.
    let bad = |text: &str| {
        let r = Rules::from_files(vec![LoadedFile {
            source: Source {
                rank: 0,
                label: "xdg".into(),
            },
            name: "90-mine.json".into(),
            text: text.into(),
        }]);
        r.problems.len() == 1
    };
    assert!(bad("{ not json }"));
    assert!(
        bad(r#"{"stage":"placement","ruls":[]}"#),
        "a typoed key must not be silently dropped"
    );
    assert!(bad(r#"{"stage":"placement","rules":"htop"}"#));
    assert!(!bad(r#"{"stage":"placement"}"#));
    // Built-in behaviour is what the default stands in for.
    assert!(has(
        &user_of(&tree_of(GUI, &Rules::builtin()), 1000).applications,
        "htop"
    ));
}

#[test]
fn an_override_cannot_break_a_structural_invariant() {
    let ov = with_user(
        r#"{"stage":"placement","rules":[
            {"id":"fold-kernel","match":{"identity":"kernel"},"fold_to":"myapp","folder":"applications"},
            {"id":"fold-acme","match":{"identity":"acme-indexer"},"fold_to":"myapp","folder":"applications"}]}"#,
    );
    let tree = tree_of(GUI, &ov);
    let user = user_of(&tree, 1000);
    assert!(has(&tree.system, "kernel"), "{:?}", titles(&tree.system));
    assert!(
        has(&user.containers, "acme-indexer"),
        "{:?}",
        titles(&user.containers)
    );
    for bucket in [&user.applications, &user.user_services, &user.containers] {
        assert!(
            !has(bucket, "myapp") && !has(bucket, "kthreadd"),
            "{:?}",
            titles(bucket)
        );
    }
}

#[test]
fn a_container_owner_override_beats_bind_mount_inference() {
    let ov = with_user(
        r#"{"stage":"placement","rules":[{"id":"own","match":{"container":"acme-encoder"},"owner_uid":1001}]}"#,
    );
    let tree = tree_of(GUI, &ov);
    assert!(has(&user_of(&tree, 1001).containers, "acme-encoder"));
    assert!(!has(&user_of(&tree, 1000).containers, "acme-encoder"));
    assert!(!has(&tree.containers, "acme-encoder"));
}

/// R44 budget harness: `build_tree` mean over the gui fixture and any
/// `HEFT_BENCH_FIXTURE`. Run with
/// `cargo test --release --test grouping -- --ignored --nocapture build_tree_timing`.
#[test]
#[ignore]
fn build_tree_timing() {
    let iters: u32 = std::env::var("HEFT_BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    let mut paths = vec![(GUI.to_string(), iters)];
    if let Ok(p) = std::env::var("HEFT_BENCH_FIXTURE") {
        paths.push((p, iters / 10));
    }
    let rules = Rules::builtin();
    for (path, iters) in paths {
        let (curr, idx, header) = load(&path, &rules);
        let t = std::time::Instant::now();
        for _ in 0..iters {
            let tree = build_tree(
                &curr,
                &curr,
                Duration::from_secs(1),
                &header,
                HostTree::default(),
                &idx,
                &rules,
            );
            std::hint::black_box(tree);
        }
        let per = t.elapsed().as_secs_f64() * 1e6 / f64::from(iters);
        println!(
            "build_tree_timing {path}: {} processes, {iters} iters, mean {per:.1} us",
            curr.len()
        );
    }
}

/// R44: nanoseconds per process for each stage, once, over the gui fixture
/// and any `HEFT_BENCH_FIXTURE`; same variables as `build_tree_timing`.
#[test]
#[ignore]
#[expect(
    clippy::cast_precision_loss,
    reason = "a process count and a nanosecond total both fit in 52 bits"
)]
fn rules_timing() {
    use heft::rules::Facts;
    use std::fmt::Write;
    let iters: u32 = std::env::var("HEFT_BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    let mut paths = vec![GUI.to_string()];
    if let Ok(p) = std::env::var("HEFT_BENCH_FIXTURE") {
        paths.push(p);
    }
    let rules = Rules::builtin();
    for path in paths {
        let (curr, _, _) = load(&path, &rules);
        let procs: Vec<(&Process, Option<String>)> = curr
            .values()
            .map(|p| (p, heft::identity_user_unit(&p.cgroup)))
            .collect();
        fn facts<'a>((p, unit): &'a (&'a Process, Option<String>)) -> Facts<'a> {
            Facts {
                comm: &p.comm,
                name: &p.comm,
                exe: p.exe.as_deref(),
                argv: &p.cmdline,
                cgroup: &p.cgroup,
                unit: unit.as_deref(),
                script: None,
                identity: None,
                container: None,
            }
        }
        let n = procs.len() as f64;
        let mut acc = 0u64;
        let mut line = format!("rules_timing {path}: {} processes", procs.len());
        for (label, which) in [
            ("classes", 0),
            ("session", 1),
            ("app", 2),
            ("unit_flags", 3),
        ] {
            let t = std::time::Instant::now();
            for _ in 0..iters {
                for pr in &procs {
                    let f = facts(pr);
                    acc += match which {
                        0 => u64::from(rules.classes(&f).0),
                        1 => rules.session(&f).map_or(0, |(i, _)| i.len() as u64),
                        2 => rules.app(&f).map_or(0, |i| i.len() as u64),
                        _ => f.unit.map_or(0, |u| u64::from(rules.unit_flags(u).0)),
                    };
                }
            }
            let ns = t.elapsed().as_nanos() as f64 / f64::from(iters) / n;
            let _ = write!(line, ", {label} {ns:.0} ns");
        }
        std::hint::black_box(acc);
        println!("{line}");
    }
}
