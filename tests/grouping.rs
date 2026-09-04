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
                pgrp: if p.pgrp == 0 { p.pid as i32 } else { p.pgrp },
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
        cpu_pct: 0.0,
        mem_used_bytes: 0,
        mem_total_bytes: 0,
        vram_used_bytes: None,
        vram_total_bytes: None,
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
        !has(&user.applications, "nautilus"),
        "minecraft must not become nautilus"
    );
    assert!(!has(&user.applications, "cursor.appimage"));

    assert!(has(&user.user_services, "earshotd"));
    assert!(has(&user.user_services, "engined"));
    assert!(has(&user.user_services, "gnome-shell"));
    assert!(!has(&user.user_services, "ghostty"));

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
