use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use heft::HostHeader;
use heft::containers::{ContainerIndex, Inspect, ListItem};
use heft::group::build_tree;
use heft::types::Process;

#[derive(serde::Deserialize)]
struct Fixture {
    nproc: u32,
    clk_tck: u64,
    page_size: u64,
    engined_uid: Option<u32>,
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
    #[serde(default)]
    sid: i32,
    uid: u32,
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

fn load(path: &str) -> (HashMap<u32, Process>, ContainerIndex, HostHeader) {
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
                sid: p.sid,
                uid: p.uid,
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
    let idx = ContainerIndex::from_list(
        &fix.containers,
        &fix.inspects,
        &fix.workdir_uids,
        fix.engined_uid,
    );
    let header = HostHeader {
        nproc: fix.nproc,
        clk_tck: fix.clk_tck,
        page_size: fix.page_size,
        ..HostHeader::default()
    };
    (curr, idx, header)
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

#[test]
fn gui_and_docker_fixture() {
    let (curr, idx, header) = load("tests/fixtures/gui/world.json");
    let tree = build_tree(&curr, &curr, Duration::from_secs(1), &header, &idx);
    let user = tree.users.iter().find(|u| u.uid == 1000).expect("uid 1000");

    for name in [
        "ghostty",
        "bash",
        "claude",
        "cursor",
        "easyeffects",
        "minecraft-launcher",
        "signal-desktop",
        "spotify",
        "vesktop.bin",
        "vivaldi-bin",
    ] {
        assert!(
            has(&user.applications, name),
            "Applications missing {name}: {:?}",
            titles(&user.applications)
        );
    }
    assert!(
        !has(&user.applications, "bunx"),
        "bunx must bill to payload"
    );
    assert!(
        !has(&user.applications, "bwrap"),
        "bwrap must bill to payload"
    );
    assert!(
        !has(&user.applications, "startvesktop"),
        "startvesktop is a launcher: {:?}",
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
        "minecraft must not become nautilus"
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

    assert!(has(&user.user_services, "earshotd"));
    assert!(has(&user.user_services, "engined"));
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
            && !has(&user.user_services, "vesktop.bin"),
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
    assert!(has(&user.applications, "majordomo"));
    assert!(!has(&user.user_services, "majordomo"));
    let vesktop = user
        .applications
        .iter()
        .find(|n| n.id == "vesktop.bin" || n.title == "vesktop.bin")
        .expect("vesktop.bin");
    assert!(
        proc_names(vesktop).iter().any(|n| n == "xdg-dbus-proxy"),
        "app-bound xdg-dbus-proxy bills to vesktop: {:?}",
        proc_names(vesktop)
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

    assert!(has(&user.containers, "supabase:caldera"));
    assert!(has(&user.containers, "engined-whisper"));
    assert!(has(&user.containers, "engined-llama"));
    assert!(has(&user.containers, "engined-kokoro"));
    assert!(!has(&user.containers, "engined"));
    assert!(!has(&user.containers, "dockerd"));
    assert!(!has(&user.containers, "containerd"));

    let caldera = user
        .containers
        .iter()
        .find(|c| c.id == "supabase:caldera")
        .unwrap();
    assert!(
        caldera.containers.len() >= 2,
        "project should expand to member containers"
    );

    assert!(has(&tree.system, "dockerd"));
    assert!(has(&tree.system, "containerd"));
    assert!(!has(&tree.system, "engined-whisper"));
    assert!(!has(&tree.system, "supabase:caldera"));
    assert!(!tree.system.iter().any(|n| n.title.starts_with("docker-")));
}
