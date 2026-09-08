use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use std::os::unix::fs::MetadataExt;

use crate::config::Overrides;
use crate::identity::docker_scope_id;
use crate::types::Process;

/// Docker/containerd ids are hex. Byte-slicing `s[..12]` panics when 12 is not a
/// UTF-8 boundary; release is `panic = abort`.
fn hex_id(s: &str) -> Option<&str> {
    (s.len() >= 12 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

pub(crate) fn hex12(s: &str) -> Option<&str> {
    hex_id(s).and_then(|id| id.get(..12))
}

/// Both index builders look an inspect up by this normalized id, in one place
/// so neither can key differently from the other: a raw `item.id` misses
/// whenever the daemon reports it in any case but the one the map was built in.
fn inspect_for<'a>(item: &ListItem, inspects: &'a HashMap<String, Inspect>) -> Option<&'a Inspect> {
    hex_id(&item.id).and_then(|id| inspects.get(&id.to_ascii_lowercase()))
}

fn live_hex_ids(list: &[ListItem]) -> Vec<String> {
    let mut ids: Vec<String> = list
        .iter()
        .filter(|item| !list_skip(item))
        .filter_map(|item| hex_id(&item.id).map(str::to_ascii_lowercase))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn list_skip(item: &ListItem) -> bool {
    matches!(item.state.as_deref(), Some("exited" | "dead"))
}

fn path_owner(path: &Path) -> Option<u32> {
    std::fs::symlink_metadata(path).ok().map(|m| m.uid())
}

#[derive(Clone, Debug)]
pub(crate) struct ContainerInfo {
    pub(crate) id: String,
    pub(crate) ident_key: String,
    pub(crate) member_name: Option<String>,
    pub(crate) owner_uid: Option<u32>,
    /// Whether this container has a network namespace of its own that heft may
    /// read. See `Inspect::owns_netns`.
    pub(crate) own_netns: bool,
}

#[derive(Default)]
pub(crate) struct InspectCache {
    ids: Vec<String>,
    inspects: HashMap<String, Inspect>,
}

impl InspectCache {
    fn refresh(&mut self, ids: Vec<String>, mut fetch: impl FnMut(&str) -> Option<Inspect>) {
        if self.ids == ids {
            return;
        }
        self.inspects.clear();
        for id in &ids {
            if let Some(insp) = fetch(id) {
                self.inspects.insert(id.clone(), insp);
            }
        }
        self.ids = ids;
    }
}

#[derive(Clone, Debug, Default)]
pub struct ContainerIndex {
    by_id: HashMap<String, ContainerInfo>,
    by_ip: HashMap<String, String>,
}

impl ContainerIndex {
    pub(crate) fn load(cache: &mut InspectCache, ov: &Overrides) -> Self {
        let mut idx = Self::default();
        let Some(sock) = docker_sock() else {
            cache.refresh(Vec::new(), |_| None);
            return idx;
        };
        let Ok(body) = unix_get(&sock, "/containers/json") else {
            return idx;
        };
        let Ok(list) = serde_json::from_slice::<Vec<ListItem>>(&body) else {
            return idx;
        };
        // Inspect is IPs/running; names/labels come from the list GET. Replace
        // the map when the id set changes so vanished ids cannot linger.
        cache.refresh(live_hex_ids(&list), |id| {
            unix_get(&sock, &format!("/containers/{id}/json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
        });
        for item in &list {
            if list_skip(item) {
                continue;
            }
            idx.insert_resolved(item, inspect_for(item, &cache.inspects), None, ov);
        }
        idx
    }
    #[must_use]
    pub fn from_list(
        items: &[ListItem],
        inspects: &HashMap<String, Inspect>,
        workdir_uids: &HashMap<PathBuf, u32>,
        ov: &Overrides,
    ) -> Self {
        let mut idx = Self::default();
        for item in items {
            if list_skip(item) {
                continue;
            }
            idx.insert_resolved(item, inspect_for(item, inspects), Some(workdir_uids), ov);
        }
        idx
    }

    fn insert_resolved(
        &mut self,
        item: &ListItem,
        inspect: Option<&Inspect>,
        workdir_uids: Option<&HashMap<PathBuf, u32>>,
        ov: &Overrides,
    ) {
        let Some(id) = hex_id(&item.id).map(str::to_ascii_lowercase) else {
            return;
        };
        let name = item
            .names
            .first()
            .map(|n| n.trim_start_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("docker-{}", hex12(&id).unwrap_or(id.as_str())));
        let labels = item.labels.clone().unwrap_or_default();
        let (ident_key, member_name) = project_identity(&name, &labels);
        let workdir = labels.get("com.supabase.cli.workdir").cloned().or_else(|| {
            labels
                .get("com.docker.compose.project.working_dir")
                .cloned()
        });
        let owner_of = |p: &str| match workdir_uids {
            Some(map) => map.get(Path::new(p)).copied(),
            None => path_owner(Path::new(p)),
        };
        // A named volume lives under /var/lib/docker/volumes and is root-owned,
        // so only a bind source carries ownership, and uid 0 is no information
        // rather than an owner: Host -> Containers stays right for a container
        // that mounts nothing of a user's.
        // A user pin comes first: it is the escape hatch for a container that
        // mounts nothing of theirs, so inference must not be able to beat it.
        let owner = ov
            .container_owner(&name)
            .or_else(|| workdir.as_deref().and_then(owner_of))
            .or_else(|| {
                inspect
                    .map(|i| i.mounts.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter(|m| m.kind.as_deref() == Some("bind"))
                    .filter_map(|m| owner_of(m.source.as_deref()?))
                    .find(|&uid| uid != 0)
            });
        let ips = inspect.map(Inspect::ips).unwrap_or_default();
        let running = inspect
            .and_then(|i| i.state.as_ref())
            .and_then(|s| s.running)
            .unwrap_or(true);
        if !running {
            return;
        }
        let info = ContainerInfo {
            id,
            ident_key,
            member_name,
            owner_uid: owner,
            own_netns: inspect.is_some_and(Inspect::owns_netns),
        };
        self.index_ids(&info);
        for ip in ips {
            self.by_ip.insert(ip, info.id.clone());
        }
    }

    fn index_ids(&mut self, info: &ContainerInfo) {
        let Some(id) = hex_id(&info.id) else {
            return;
        };
        self.by_id.insert(id.to_ascii_lowercase(), info.clone());
        if let Some(short) = hex12(id) {
            self.by_id.insert(short.to_ascii_lowercase(), info.clone());
        }
    }
    pub(crate) fn get(&self, raw: &str) -> Option<&ContainerInfo> {
        let id = hex_id(raw)?.to_ascii_lowercase();
        self.by_id
            .get(&id)
            .or_else(|| hex12(&id).and_then(|s| self.by_id.get(s)))
    }
    pub(crate) fn by_ip(&self, ip: &str) -> Option<&ContainerInfo> {
        self.by_ip.get(ip).and_then(|id| self.get(id))
    }
    pub(crate) fn lookup_process(&self, p: &Process) -> Option<&ContainerInfo> {
        if let Some(id) = docker_scope_id(&p.cgroup) {
            return self.get(&id);
        }
        helper_id(p).and_then(|id| self.get(&id)).or_else(|| {
            crate::classify::cmdline_flag_value(&p.cmdline, "-container-ip")
                .and_then(|ip| self.by_ip(ip))
        })
    }
}

pub(crate) fn helper_id(p: &Process) -> Option<String> {
    let names = crate::classify::names_of(p);
    let runtime = crate::classify::names_match(&names, |n| {
        n.contains("containerd-shim") || matches!(n, "conmon" | "runc" | "crun")
    });
    if !runtime {
        return None;
    }
    if let Some(id) = crate::classify::cmdline_flag_value(&p.cmdline, "-id") {
        return hex_id(id).map(str::to_ascii_lowercase);
    }
    p.cmdline
        .iter()
        .find_map(|arg| hex_id(arg).map(str::to_ascii_lowercase))
}
pub(crate) fn project_identity(
    name: &str,
    labels: &HashMap<String, String>,
) -> (String, Option<String>) {
    if let Some(p) = labels.get("com.supabase.cli.project") {
        return (format!("supabase:{p}"), Some(name.to_string()));
    }
    if let Some(p) = supabase_project_from_name(name) {
        return (format!("supabase:{p}"), Some(name.to_string()));
    }
    if let Some(p) = labels.get("com.docker.compose.project") {
        return (p.clone(), Some(name.to_string()));
    }
    (name.to_string(), None)
}
pub(crate) fn supabase_project_from_name(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("supabase_")?;
    let (_, proj) = rest.rsplit_once('_')?;
    if proj.is_empty() { None } else { Some(proj) }
}

fn docker_sock() -> Option<PathBuf> {
    if let Ok(host) = std::env::var("DOCKER_HOST")
        && let Some(path) = host.strip_prefix("unix://")
    {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    let default = PathBuf::from("/var/run/docker.sock");
    if default.exists() {
        return Some(default);
    }
    let uid = crate::cpu::euid();
    let podman = PathBuf::from(format!("/run/user/{uid}/podman/podman.sock"));
    if podman.exists() { Some(podman) } else { None }
}

fn docker_get_path(path: &str) -> bool {
    if path
        .bytes()
        .any(|b| matches!(b, b'\r' | b'\n' | b'\0' | b' '))
    {
        return false;
    }
    if path == "/containers/json" {
        return true;
    }
    path.strip_circumfix("/containers/", "/json")
        .is_some_and(|id| hex_id(id).is_some())
}

fn http_2xx(head: &[u8]) -> bool {
    head.split(|&b| b == b' ')
        .nth(1)
        .and_then(|c| c.get(..3))
        .is_some_and(|c| c[0] == b'2' && c[1].is_ascii_digit() && c[2].is_ascii_digit())
}

fn unix_get(sock: &Path, path: &str) -> Result<Vec<u8>, crate::types::Error> {
    const MAX: usize = 4 * 1024 * 1024;
    if !docker_get_path(path) {
        return Err("invalid docker path".into());
    }
    let mut s = UnixStream::connect(sock)?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    s.set_write_timeout(Some(Duration::from_secs(2)))?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut buf = Vec::new();
    s.take(MAX as u64 + 1).read_to_end(&mut buf)?;
    if buf.len() > MAX {
        return Err("docker response too large".into());
    }
    let sep = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("short docker response")?;
    if !http_2xx(&buf[..sep]) {
        return Err("docker HTTP error".into());
    }
    Ok(buf[sep + 4..].to_vec())
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListItem {
    #[serde(rename = "Id", default)]
    pub(crate) id: String,
    #[serde(rename = "Names", default)]
    pub(crate) names: Vec<String>,
    #[serde(rename = "Labels")]
    pub(crate) labels: Option<HashMap<String, String>>,
    #[serde(rename = "State")]
    pub(crate) state: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct Inspect {
    #[serde(rename = "State")]
    pub(crate) state: Option<InspectState>,
    #[serde(rename = "NetworkSettings")]
    pub(crate) network: Option<NetworkSettings>,
    #[serde(rename = "Mounts", default)]
    pub(crate) mounts: Vec<Mount>,
    #[serde(rename = "HostConfig")]
    pub(crate) host_config: Option<HostConfig>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub(crate) struct HostConfig {
    #[serde(rename = "NetworkMode")]
    pub(crate) network_mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub(crate) struct Mount {
    #[serde(rename = "Type")]
    pub(crate) kind: Option<String>,
    #[serde(rename = "Source")]
    pub(crate) source: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub(crate) struct InspectState {
    #[serde(rename = "Running")]
    pub(crate) running: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub(crate) struct NetworkSettings {
    #[serde(rename = "IPAddress")]
    pub(crate) ip: Option<String>,
    #[serde(rename = "Networks")]
    pub(crate) networks: Option<HashMap<String, Value>>,
}

impl Inspect {
    /// `--network=host` shares the root network namespace, so that container's
    /// `/proc/<pid>/net/dev` is the machine-wide file: measured, 988.5 GB in
    /// and 430.6 GB out, none of it the container's. It gets no rate, and an
    /// absent `NetworkMode` fails closed for the same reason.
    fn owns_netns(&self) -> bool {
        self.host_config
            .as_ref()
            .and_then(|h| h.network_mode.as_deref())
            .is_some_and(|mode| mode != "host")
    }

    fn ips(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(ip) = self.network.as_ref().and_then(|n| n.ip.clone())
            && !ip.is_empty()
        {
            out.push(ip);
        }
        if let Some(nets) = self.network.as_ref().and_then(|n| n.networks.as_ref()) {
            for v in nets.values() {
                if let Some(ip) = v.get("IPAddress").and_then(Value::as_str)
                    && !ip.is_empty()
                {
                    out.push(ip.to_string());
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supabase_project() {
        assert_eq!(supabase_project_from_name("supabase_db_demo"), Some("demo"));
        let labels = HashMap::from([("com.supabase.cli.project".into(), "demo".into())]);
        let (k, m) = project_identity("supabase_db_demo", &labels);
        assert_eq!(k, "supabase:demo");
        assert_eq!(m.as_deref(), Some("supabase_db_demo"));
        let (k, m) = project_identity("spec-runner-7", &HashMap::new());
        assert_eq!(k, "spec-runner-7");
        assert!(m.is_none());
    }

    fn runc(id: &str) -> Process {
        Process {
            comm: "runc".into(),
            cmdline: vec!["runc".into(), "-id".into(), id.into()],
            ..Process::default()
        }
    }

    fn info(id: &str) -> ContainerInfo {
        ContainerInfo {
            id: id.into(),
            ident_key: "x".into(),
            member_name: None,
            owner_uid: None,
            own_netns: false,
        }
    }

    #[test]
    fn untrusted_helper_id_does_not_panic_or_index() {
        // 11 ASCII + U+00E9 (2 bytes) = 13 bytes; byte 12 is mid-character.
        let mid = "01234567890é";
        assert_eq!(mid.len(), 13);
        assert!(mid.get(..12).is_none());
        assert!(helper_id(&runc(mid)).is_none());
        assert!(helper_id(&runc("zzzzzzzzzzzzz")).is_none());

        let mut idx = ContainerIndex::default();
        idx.index_ids(&info(mid));
        idx.index_ids(&info("zzzzzzzzzzzzz"));
        assert!(idx.by_id.is_empty());
        assert!(idx.get(mid).is_none());
        assert!(idx.get("zzzzzzzzzzzzz").is_none());

        let hex = "0123456789abcdef";
        idx.index_ids(&info(hex));
        assert!(idx.get(hex).is_some());
        assert!(idx.get("0123456789ab").is_some());
        assert!(idx.get(mid).is_none());
        assert_eq!(helper_id(&runc(hex)).as_deref(), Some(hex));

        assert!(!docker_get_path("/containers/0123456789ab\r\nHost: x/json"));
        assert!(!docker_get_path(&format!("/containers/{mid}/json")));
        assert!(docker_get_path("/containers/json"));
        assert!(docker_get_path("/containers/0123456789ab/json"));
    }

    #[test]
    fn runtime_helpers_match_regardless_of_case() {
        let hex = "0123456789abcdef";
        for comm in ["RunC", "Conmon", "CRun", "Containerd-Shim-Runc-V2"] {
            let p = Process {
                comm: comm.into(),
                cmdline: vec![comm.into(), "-id".into(), hex.into()],
                ..Process::default()
            };
            assert_eq!(helper_id(&p).as_deref(), Some(hex), "comm {comm}");
        }
        let by_exe = Process {
            comm: "n/a".into(),
            exe: Some("/usr/bin/Conmon".into()),
            cmdline: vec!["conmon".into(), "-id".into(), hex.into()],
            ..Process::default()
        };
        assert_eq!(helper_id(&by_exe).as_deref(), Some(hex));
    }

    fn item(id: &str, state: Option<&str>) -> ListItem {
        ListItem {
            id: id.into(),
            names: vec![],
            labels: None,
            state: state.map(str::to_string),
        }
    }

    #[test]
    fn live_ids_skip_exited_sort_and_lower() {
        let items = vec![
            item("BBBBBBBBBBBB", Some("running")),
            item("AAAAAAAAAAAA", Some("exited")),
            item("cccccccccccccccc", Some("dead")),
            item("aaaaaaaaaaaa", None),
        ];
        assert_eq!(
            live_hex_ids(&items),
            vec!["aaaaaaaaaaaa".to_string(), "bbbbbbbbbbbb".to_string()]
        );
    }

    #[test]
    fn inspect_cache_reuses_until_id_set_changes() {
        let mut cache = InspectCache::default();
        let mut fetches = 0;
        let a = "0123456789ab".to_string();
        let b = "0123456789cd".to_string();
        cache.refresh(vec![a.clone(), b.clone()], |_| {
            fetches += 1;
            Some(Inspect::default())
        });
        assert_eq!(fetches, 2);
        cache.refresh(vec![a.clone(), b.clone()], |_| {
            fetches += 1;
            Some(Inspect::default())
        });
        assert_eq!(fetches, 2);
        cache.refresh(vec![b.clone()], |_| {
            fetches += 1;
            Some(Inspect::default())
        });
        assert_eq!(fetches, 3);
        assert!(!cache.inspects.contains_key(&a));
        assert!(cache.inspects.contains_key(&b));
    }
}
