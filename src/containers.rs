use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use std::os::unix::fs::MetadataExt;

use crate::identity::docker_scope_id;
use crate::rules::Rules;
use crate::types::Process;

/// Docker/containerd ids are hex. Byte-slicing `s[..12]` panics when 12 is not a
/// UTF-8 boundary; release is `panic = abort`.
fn hex_id(s: &str) -> Option<&str> {
    (s.len() >= 12 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

pub(crate) fn hex12(s: &str) -> Option<&str> {
    hex_id(s).and_then(|id| id.get(..12))
}

/// The key every `by_id` insert and lookup uses, so no caller can key an id in
/// a case the others do not.
fn normalized_id(s: &str) -> Option<String> {
    hex_id(s).map(str::to_ascii_lowercase)
}

/// The `by_id` key, on insert and lookup alike: the normalized id cut to 12
/// hex digits, so a full id and a truncated one reach the same row whichever
/// side reported which. The ceiling: two running containers sharing a 12-hex
/// prefix bill to one row, the ambiguity the docker CLI refuses a short id for.
fn id_key(s: &str) -> Option<String> {
    let mut id = normalized_id(s)?;
    id.truncate(12);
    Some(id)
}

/// Both index builders look an inspect up by this normalized id, in one place
/// so neither can key differently from the other: a raw `item.id` misses
/// whenever the daemon reports it in any case but the one the map was built in.
fn inspect_for<'a>(item: &ListItem, inspects: &'a HashMap<String, Inspect>) -> Option<&'a Inspect> {
    normalized_id(&item.id).and_then(|id| inspects.get(&id))
}

/// The title a container with no name is shown under. It is also that row's
/// merge key, so the two builders that need it must spell it the same way or
/// one container becomes two rows.
pub(crate) fn docker_title(id: &str) -> String {
    format!("docker-{}", hex12(id).unwrap_or(id))
}

fn live_hex_ids(list: &[ListItem]) -> Vec<String> {
    let mut ids: Vec<String> = list
        .iter()
        .filter(|item| !list_skip(item))
        .filter_map(|item| normalized_id(&item.id))
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
    pub(crate) fn load(cache: &mut InspectCache, rules: &Rules) -> Self {
        let Some(sock) = docker_sock() else {
            cache.refresh(Vec::new(), |_| None);
            return Self::default();
        };
        let Ok(body) = unix_get(&sock, "/containers/json") else {
            return Self::default();
        };
        let Ok(list) = serde_json::from_slice::<Vec<ListItem>>(&body) else {
            return Self::default();
        };
        // Inspect is IPs/running; names/labels come from the list GET. Replace
        // the map when the id set changes so vanished ids cannot linger.
        cache.refresh(live_hex_ids(&list), |id| {
            unix_get(&sock, &format!("/containers/{id}/json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
        });
        Self::from_list(&list, &cache.inspects, path_owner, rules)
    }

    /// The index over a container list. `owner_of` is a path's owning uid:
    /// `path_owner` on a live host, a recorded map for a fixture from another
    /// machine, whose paths do not exist here.
    #[must_use]
    pub fn from_list(
        items: &[ListItem],
        inspects: &HashMap<String, Inspect>,
        owner_of: impl Fn(&Path) -> Option<u32>,
        rules: &Rules,
    ) -> Self {
        let mut idx = Self::default();
        for item in items.iter().filter(|item| !list_skip(item)) {
            idx.insert_resolved(item, inspect_for(item, inspects), &owner_of, rules);
        }
        idx
    }

    fn insert_resolved(
        &mut self,
        item: &ListItem,
        inspect: Option<&Inspect>,
        owner_of: &impl Fn(&Path) -> Option<u32>,
        rules: &Rules,
    ) {
        let Some(id) = normalized_id(&item.id) else {
            return;
        };
        let name = item
            .names
            .first()
            .map(|n| n.trim_start_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| docker_title(&id));
        let labels = item.labels.clone().unwrap_or_default();
        let (ident_key, member_name) = project_identity(&name, &labels);
        let workdir = labels.get("com.supabase.cli.workdir").cloned().or_else(|| {
            labels
                .get("com.docker.compose.project.working_dir")
                .cloned()
        });
        // A named volume lives under /var/lib/docker/volumes and is root-owned,
        // so only a bind source carries ownership, and uid 0 is no information
        // rather than an owner: Host -> Containers stays right for a container
        // that mounts nothing of a user's.
        // A user pin comes first: it is the escape hatch for a container that
        // mounts nothing of theirs, so inference must not be able to beat it.
        let owner = rules
            .container_owner(&name)
            .or_else(|| workdir.as_deref().and_then(|p| owner_of(Path::new(p))))
            .or_else(|| {
                inspect
                    .map(|i| i.mounts.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter(|m| m.kind.as_deref() == Some("bind"))
                    .filter_map(|m| owner_of(Path::new(m.source.as_deref()?)))
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
        for ip in ips {
            self.by_ip.insert(ip, info.id.clone());
        }
        if let Some(key) = id_key(&info.id) {
            self.by_id.insert(key, info);
        }
    }

    pub(crate) fn get(&self, raw: &str) -> Option<&ContainerInfo> {
        self.by_id.get(&id_key(raw)?)
    }
    pub(crate) fn by_ip(&self, ip: &str) -> Option<&ContainerInfo> {
        self.by_ip.get(ip).and_then(|id| self.get(id))
    }
    /// `runtime` is the `container_runtime` class the caller already holds
    /// for this pid, so the name test is not run again here.
    pub(crate) fn lookup_process(&self, p: &Process, runtime: bool) -> Option<&ContainerInfo> {
        if let Some(id) = docker_scope_id(&p.cgroup) {
            return self.get(&id);
        }
        helper_id(p, runtime)
            .and_then(|id| self.get(&id))
            .or_else(|| {
                crate::classify::cmdline_flag_value(&p.cmdline, "-container-ip")
                    .and_then(|ip| self.by_ip(ip))
            })
    }
}

/// The container id a runtime helper (`containerd-shim-runc-v2 -id`, `conmon`,
/// `runc`, `crun`) names in its argv; `runtime` is that class, decided by the
/// `container-runtimes` rule in `rules.d/10-classes.json`.
pub(crate) fn helper_id(p: &Process, runtime: bool) -> Option<String> {
    if !runtime {
        return None;
    }
    if let Some(id) = crate::classify::cmdline_flag_value(&p.cmdline, "-id") {
        return normalized_id(id);
    }
    p.cmdline.iter().find_map(|arg| normalized_id(arg))
}
fn project_identity(name: &str, labels: &HashMap<String, String>) -> (String, Option<String>) {
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
fn supabase_project_from_name(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("supabase_")?;
    let (_, proj) = rest.rsplit_once('_')?;
    (!proj.is_empty()).then_some(proj)
}

fn docker_sock() -> Option<PathBuf> {
    let host = std::env::var("DOCKER_HOST").ok();
    host.as_deref()
        .and_then(|h| h.strip_prefix("unix://"))
        .map(PathBuf::from)
        .into_iter()
        .chain([
            PathBuf::from("/var/run/docker.sock"),
            PathBuf::from(format!(
                "/run/user/{}/podman/podman.sock",
                crate::cpu::euid()
            )),
            // Rootful Podman, the default on RHEL and Fedora servers. Without it
            // every container on such a host rendered `docker-<12hex>` with no
            // owner and a blank NETNS, indistinguishable from having no socket
            // at all. Last, because a rootless socket belongs to the user heft
            // is running as and a rootful one may not be readable.
            PathBuf::from("/run/podman/podman.sock"),
        ])
        .find(|p| p.exists())
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
    pub(crate) networks: Option<HashMap<String, Network>>,
}

/// Every field optional and unknown keys ignored, so a runtime's schema
/// difference costs an address, never the container.
#[derive(Clone, Debug, Deserialize, Default)]
pub(crate) struct Network {
    #[serde(rename = "IPAddress")]
    pub(crate) ip: Option<String>,
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
        let Some(n) = &self.network else {
            return Vec::new();
        };
        let mut out: Vec<String> = n
            .networks
            .iter()
            .flat_map(HashMap::values)
            .filter_map(|net| net.ip.as_ref())
            .chain(&n.ip)
            .filter(|ip| !ip.is_empty())
            .cloned()
            .collect();
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
        assert!(helper_id(&runc(mid), true).is_none());
        assert!(helper_id(&runc("zzzzzzzzzzzzz"), true).is_none());

        assert!(id_key(mid).is_none());
        assert!(id_key("zzzzzzzzzzzzz").is_none());

        let hex = "0123456789abcdef";
        let mut idx = ContainerIndex::default();
        idx.by_id.insert(id_key(hex).unwrap(), info(hex));
        assert!(idx.get(hex).is_some());
        assert!(
            idx.get("0123456789AB").is_some(),
            "truncated, in another case"
        );
        assert!(idx.get(mid).is_none());
        let mut short = ContainerIndex::default();
        short
            .by_id
            .insert(id_key("0123456789ab").unwrap(), info("0123456789ab"));
        assert!(
            short.get(hex).is_some(),
            "a full id finds a row a truncated id keyed"
        );
        assert_eq!(helper_id(&runc(hex), true).as_deref(), Some(hex));
        assert!(helper_id(&runc(hex), false).is_none());

        assert!(!docker_get_path("/containers/0123456789ab\r\nHost: x/json"));
        assert!(!docker_get_path(&format!("/containers/{mid}/json")));
        assert!(docker_get_path("/containers/json"));
        assert!(docker_get_path("/containers/0123456789ab/json"));
    }

    #[test]
    fn a_minimal_inspect_parses_and_yields_its_ips() {
        let bare: Inspect = serde_json::from_str("{}").unwrap();
        assert!(bare.ips().is_empty());
        let i: Inspect = serde_json::from_str(
            r#"{"Unknown":1,"NetworkSettings":{"IPAddress":"","Networks":{
                "bridge":{"IPAddress":"172.17.0.2","Gateway":"172.17.0.1"},"none":{}}}}"#,
        )
        .unwrap();
        assert_eq!(i.ips(), ["172.17.0.2"]);
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
