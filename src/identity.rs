use crate::classify::{self, script_basename};
use crate::types::Process;

pub(crate) fn docker_scope_id(cgroup: &str) -> Option<String> {
    scope_hex(cgroup, "docker-").or_else(|| scope_hex(cgroup, "libpod-"))
}

fn scope_hex(cgroup: &str, prefix: &str) -> Option<String> {
    let mut start = 0;
    while let Some(rel) = cgroup[start..].find(prefix) {
        let i = start + rel + prefix.len();
        let rest = &cgroup[i..];
        let n = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
        if n >= 12 && rest[n..].starts_with(".scope") {
            return Some(rest[..n].to_ascii_lowercase());
        }
        start = i;
    }
    None
}

/// The machine name from `machine.slice/machine-<name>.scope` — a
/// systemd-nspawn container, a `machinectl` machine, or a libvirt VM. These
/// are workloads, but they carry no Docker or Podman API to inspect, so they
/// reach the tree through the cgroup alone.
///
/// Rootful Podman also lives under `machine.slice`, as `libpod-<hex>.scope`;
/// that prefix is not `machine-`, and `container_place` runs first regardless.
pub(crate) fn machine_scope_name(cgroup: &str) -> Option<String> {
    let mut start = 0;
    while let Some(rel) = cgroup[start..].find("machine-") {
        let i = start + rel + "machine-".len();
        let rest = &cgroup[i..];
        if let Some(end) = rest.find(".scope")
            && end > 0
        {
            return Some(machine_display_name(&systemd_unescape(&rest[..end])));
        }
        start = i;
    }
    None
}

/// libvirt names a domain's scope `machine-qemu-<id>-<domain>.scope`, so
/// the unescaped form reads `qemu-3-fedora`. The id is libvirt's own counter
/// and means nothing to whoever is reading the tree; the domain name is what
/// they called the VM.
fn machine_display_name(name: &str) -> String {
    let Some(rest) = name.strip_prefix("qemu-") else {
        return name.to_string();
    };
    match rest.split_once('-') {
        Some((id, domain)) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => {
            domain.to_string()
        }
        _ => name.to_string(),
    }
}

pub(crate) fn in_system_slice(cgroup: &str) -> bool {
    cgroup.contains("/system.slice/") || cgroup.ends_with("/system.slice")
}

pub(crate) fn in_user_slice(cgroup: &str) -> bool {
    cgroup.contains("/user.slice/") || cgroup.contains("user@")
}

/// The one line of `/proc/<pid>/cgroup` a unit name can be read from.
///
/// v2 writes a single `0::/path`. v1 writes a line per controller and ends
/// with an empty `0::/`, so taking the leaf of the whole file read that empty
/// line: every user unit came back `None`, and since the Applications vs User
/// Services split needs a unit name, User Services stayed empty on a v1 host
/// while every user process filed as an application. systemd's own hierarchy
/// is the v1 line that carries unit names. Same shape as `psi::cgroup_path`.
fn unit_line(cgroup: &str) -> &str {
    if let Some(v2) = cgroup.lines().find_map(|l| l.strip_prefix("0::"))
        && v2 != "/"
    {
        return v2;
    }
    cgroup
        .lines()
        .find_map(|l| l.split_once(":name=systemd:").map(|(_, path)| path))
        .unwrap_or(cgroup)
}

pub(crate) fn user_unit(cgroup: &str) -> Option<String> {
    let cgroup = unit_line(cgroup);
    let after = match cgroup.find("user@") {
        Some(i) => {
            let rest = &cgroup[i..];
            &rest[rest.find('/')? + 1..]
        }
        None => cgroup,
    };
    let leaf = after.rsplit('/').next()?;
    if leaf.is_empty() {
        return None;
    }
    Some(systemd_unescape(leaf))
}

/// Decodes into bytes and converts once at the end, never `byte as char`.
/// systemd escapes a unit name one byte at a time, so a UTF-8 character
/// arrives as several `\\xNN` escapes: casting each to a `char` reads them as
/// Latin-1 and turns `\\xc3\\xa9` into `Ã©` instead of `é`. Lossy at the end
/// rather than fallible, because a unit name heft cannot decode is still a row
/// worth drawing.
fn systemd_unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1] == b'x'
            && let Ok(hex) = std::str::from_utf8(&b[i + 2..i + 4])
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn lying_unit(unit: &str) -> bool {
    let u = unit.to_ascii_lowercase();
    u.contains("-transient-")
        || u.contains("org.chromium.chromium")
        || u.starts_with("dbus:")
        || u.starts_with("dbus-:")
        || u.starts_with("run-u")
        || u.starts_with("flatpak-session-helper")
}

pub(crate) fn is_user_service_unit(unit: &str) -> bool {
    let u = unit.to_ascii_lowercase();
    if u == "init.scope" {
        return true;
    }
    let service = u.ends_with(".service");
    service && !u.starts_with("app-")
}

fn unit_stem(unit: &str) -> String {
    let mut s = unit.to_string();
    if let Some(stripped) = s.strip_suffix(".scope") {
        s = stripped.to_string();
    } else if let Some(stripped) = s.strip_suffix(".service") {
        s = stripped.to_string();
    }
    if let Some((name, _)) = s.split_once('@') {
        name.to_string()
    } else {
        s
    }
}

/// `PF_KTHREAD` is the kernel's own answer, so nothing here re-derives it from
/// uid/ppid/comm. Those two agreed on all 864 live pids of this machine, but
/// only the flag survives a kthread reparented away from `kthreadd`.
pub(crate) const fn is_kernel(p: &Process) -> bool {
    p.kthread
}

pub(crate) fn generic_fallback(p: &Process, unit: Option<&str>) -> String {
    if let Some(u) = unit
        && !lying_unit(u)
    {
        let stem = unit_stem(u);
        if !stem.is_empty() && stem != "app" {
            return stem;
        }
    }
    if let Some(script) = script_basename(&p.cmdline) {
        let b = classify::basename(&script);
        if !matches!(
            b.as_str(),
            "main.js" | "main" | "index.js" | "app.js" | "server.js" | "run.js"
        ) {
            return script;
        }
    }
    classify::name_of(p)
}

pub(crate) fn instance_key(p: &Process, container_id: Option<&str>) -> String {
    if let Some(id) = container_id {
        return format!("ctr:{id}");
    }
    if let Some(unit) = user_unit(&p.cgroup)
        && !lying_unit(&unit)
    {
        return unit;
    }
    format!("pgid:{}", p.pgrp)
}

#[cfg(test)]
mod tests {
    /// systemd escapes a unit name byte by byte, so one non-ASCII character
    /// arrives as several escapes and has to be reassembled before it becomes
    /// text. Reading each escape as its own char produced Latin-1 mojibake in
    /// every unit or scope name that was not pure ASCII.
    #[test]
    fn unescaping_reassembles_a_multibyte_character() {
        use super::systemd_unescape;
        assert_eq!(systemd_unescape(r"caf\xc3\xa9.service"), "café.service");
        assert_eq!(
            systemd_unescape(r"app-\xe6\x97\xa5\xe6\x9c\xac.scope"),
            "app-日本.scope"
        );
        // ASCII escapes and untouched text still behave as they always did.
        assert_eq!(systemd_unescape(r"dev\x2ddisk.mount"), "dev-disk.mount");
        assert_eq!(systemd_unescape("plain.service"), "plain.service");
        // A trailing partial escape is data, not a decode: it must not panic.
        assert_eq!(systemd_unescape(r"trail\xc"), r"trail\xc");
    }

    use super::*;

    /// A VM or nspawn container is a workload with no API to inspect, so the
    /// cgroup name is the only thing that can title the row. libvirt's scope
    /// carries an internal counter that means nothing to a reader; rootful
    /// Podman shares the same slice and must not be mistaken for one.
    #[test]
    fn a_machine_scope_names_the_machine() {
        let name = |cg| machine_scope_name(cg);
        assert_eq!(
            name(r"0::/machine.slice/machine-qemu-3-fedora.scope/libvirt/emulator"),
            Some("fedora".to_string())
        );
        assert_eq!(
            name(r"0::/machine.slice/machine-my-builder.scope"),
            Some("my-builder".to_string())
        );
        // Not a libvirt counter, so the name stays whole.
        assert_eq!(
            name("0::/machine.slice/machine-qemu-tools.scope"),
            Some("qemu-tools".to_string())
        );
        // Rootful Podman lives in the same slice under a different prefix.
        assert_eq!(name("0::/machine.slice/libpod-abc123def456.scope"), None);
        assert_eq!(name("0::/system.slice/sshd.service"), None);
    }

    /// A v1 host writes a line per controller and ends with an empty `0::/`.
    /// Reading the leaf of the whole file found that empty line, so every user
    /// unit came back `None` and User Services could never fill.
    #[test]
    fn a_unit_name_survives_a_v1_cgroup_file() {
        let leaf = "/user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox.scope";
        let v1 = format!("12:pids:{leaf}\n4:memory:{leaf}\n1:name=systemd:{leaf}\n0::/");
        assert_eq!(user_unit(&v1).as_deref(), Some("app-firefox.scope"));
        // v2 is one line and must be unchanged by the fallback.
        assert_eq!(
            user_unit(&format!("0::{leaf}")).as_deref(),
            Some("app-firefox.scope")
        );
        // Pure v1, no unified hierarchy line at all.
        assert_eq!(
            user_unit(&format!("1:name=systemd:{leaf}")).as_deref(),
            Some("app-firefox.scope")
        );
        // A v2 root cgroup still has no unit to name.
        assert_eq!(user_unit("0::/"), None);
    }

    #[test]
    fn docker_and_unescape() {
        let cg = "0::/system.slice/docker-31c4735b8670a5b31ca1894f141c5cff4a2f5ed814214c9ccd4dc2a3ec0cc94c.scope";
        assert_eq!(docker_scope_id(cg).unwrap().len(), 64);
        assert_eq!(
            systemd_unescape(r"app-gnome-vivaldi\x2dstable-1.scope"),
            "app-gnome-vivaldi-stable-1.scope"
        );
        assert!(lying_unit("app-ghostty-surface-transient-1.scope"));
        assert!(lying_unit("app-org.chromium.Chromium-1743723.scope"));
        assert!(lying_unit("flatpak-session-helper.service"));
        assert!(lying_unit("dbus-:1.2-org.gnome.Nautilus@250.service"));
        assert!(!lying_unit("dbus-broker.service"));
        assert!(is_user_service_unit("syncthing.service"));
        assert!(!is_user_service_unit("app-com.mitchellh.ghostty.service"));
        assert!(is_user_service_unit("org.gnome.Shell@user.service"));
    }
}
