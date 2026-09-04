use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use std::os::unix::fs::MetadataExt;

use crate::identity::docker_scope_id;
use crate::types::Process;

/// Docker/containerd ids are hex. Byte-slicing `s[..12]` panics when 12 is not a
/// UTF-8 boundary; release is `panic = abort`.
fn hex_id(s: &str) -> Option<&str> {
    (s.len() >= 12 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

fn hex12(s: &str) -> Option<&str> {
    hex_id(s).and_then(|id| id.get(..12))
}

fn path_owner(path: &Path) -> Option<u32> {
    std::fs::symlink_metadata(path).ok().map(|m| m.uid())
}

#[derive(Clone, Debug)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub ident_key: String,
    pub ident_title: String,
    pub member_name: Option<String>,
    pub ips: Vec<String>,
    pub owner_uid: Option<u32>,
    pub running: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ContainerIndex {
    by_id: HashMap<String, ContainerInfo>,
    by_ip: HashMap<String, String>,
    pub engined_uid: Option<u32>,
}

impl ContainerIndex {
    pub fn load() -> Self {
        Self::load_with_engined(None)
    }

    pub fn load_with_engined(engined_uid: Option<u32>) -> Self {
        let mut idx = Self {
            engined_uid,
            ..Self::default()
        };
        let Some(sock) = docker_sock() else {
            return idx;
        };
        let Ok(body) = unix_get(&sock, "/containers/json") else {
            return idx;
        };
        let Ok(list) = serde_json::from_slice::<Vec<ListItem>>(&body) else {
            return idx;
        };
        for item in list {
            if item.state.as_deref() == Some("exited") || item.state.as_deref() == Some("dead") {
                continue;
            }
            let inspect = hex_id(&item.id)
                .and_then(|id| unix_get(&sock, &format!("/containers/{id}/json")).ok())
                .and_then(|b| serde_json::from_slice::<Inspect>(&b).ok());
            idx.insert_item(&item, inspect.as_ref());
        }
        idx
    }

    pub fn from_list(
        items: &[ListItem],
        inspects: &HashMap<String, Inspect>,
        workdir_uids: &HashMap<PathBuf, u32>,
        engined_uid: Option<u32>,
    ) -> Self {
        let mut idx = Self {
            engined_uid,
            ..Self::default()
        };
        for item in items {
            if item.state.as_deref() == Some("exited") || item.state.as_deref() == Some("dead") {
                continue;
            }
            let inspect = inspects.get(&item.id).or_else(|| {
                inspects
                    .iter()
                    .find(|(k, _)| item.id.starts_with(*k) || k.starts_with(&item.id))
                    .map(|(_, v)| v)
            });
            idx.insert_resolved(item, inspect, Some(workdir_uids));
        }
        idx
    }

    fn insert_item(&mut self, item: &ListItem, inspect: Option<&Inspect>) {
        self.insert_resolved(item, inspect, None);
    }

    fn insert_resolved(
        &mut self,
        item: &ListItem,
        inspect: Option<&Inspect>,
        workdir_uids: Option<&HashMap<PathBuf, u32>>,
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
        let (ident_key, ident_title, member_name) = project_identity(&name, &labels);
        let workdir = labels.get("com.supabase.cli.workdir").cloned().or_else(|| {
            labels
                .get("com.docker.compose.project.working_dir")
                .cloned()
        });
        let mut owner = workdir.as_ref().and_then(|p| {
            if let Some(map) = workdir_uids {
                map.get(&PathBuf::from(p)).copied()
            } else {
                path_owner(Path::new(p))
            }
        });
        let engined = name.starts_with("engined-") || labels.contains_key("engined.spec");
        if owner.is_none() && engined {
            owner = self.engined_uid;
        }
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
            name,
            ident_key,
            ident_title,
            member_name,
            ips: ips.clone(),
            owner_uid: owner,
            running,
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

    pub fn get(&self, raw: &str) -> Option<&ContainerInfo> {
        let id = hex_id(raw)?.to_ascii_lowercase();
        self.by_id
            .get(&id)
            .or_else(|| id.get(..12).and_then(|s| self.by_id.get(s)))
            .or_else(|| {
                self.by_id
                    .values()
                    .find(|c| c.id.starts_with(&id) || id.starts_with(&c.id))
            })
    }

    pub fn by_ip(&self, ip: &str) -> Option<&ContainerInfo> {
        self.by_ip.get(ip).and_then(|id| self.get(id))
    }

    pub fn lookup_process(&self, p: &Process) -> Option<&ContainerInfo> {
        if let Some(id) = docker_scope_id(&p.cgroup) {
            return self.get(&id);
        }
        helper_id(p).and_then(|id| self.get(&id)).or_else(|| {
            crate::classify::cmdline_flag_value(&p.cmdline, "-container-ip")
                .and_then(|ip| self.by_ip(ip))
        })
    }

    pub fn apply_engined_uid(&mut self, uid: Option<u32>) {
        self.engined_uid = uid;
        for info in self.by_id.values_mut() {
            if info.owner_uid.is_none()
                && (info.name.starts_with("engined-") || info.ident_key.starts_with("engined-"))
            {
                info.owner_uid = uid;
            }
        }
    }
}

pub fn helper_id(p: &Process) -> Option<String> {
    let comm = p.comm.as_str();
    let name = crate::classify::name_of(p);
    let is_shim = comm.contains("containerd-shim") || name.contains("containerd-shim");
    let is_runtime = matches!(comm, "conmon" | "runc" | "crun")
        || matches!(name.as_str(), "conmon" | "runc" | "crun");
    if is_shim || is_runtime {
        if let Some(id) = crate::classify::cmdline_flag_value(&p.cmdline, "-id") {
            return hex_id(id).map(str::to_ascii_lowercase);
        }
        return p
            .cmdline
            .iter()
            .find_map(|arg| hex_id(arg).map(str::to_ascii_lowercase));
    }
    None
}

pub fn synthetic_from_cgroup(cgroup: &str) -> Option<(String, String)> {
    let id = docker_scope_id(cgroup)?;
    let short: String = id.chars().take(12).collect();
    let title = format!("docker-{short}");
    Some((title.clone(), title))
}

pub fn project_identity(
    name: &str,
    labels: &HashMap<String, String>,
) -> (String, String, Option<String>) {
    if let Some(p) = labels.get("com.supabase.cli.project") {
        return (
            format!("supabase:{p}"),
            format!("supabase:{p}"),
            Some(name.to_string()),
        );
    }
    if let Some(p) = supabase_project_from_name(name) {
        return (
            format!("supabase:{p}"),
            format!("supabase:{p}"),
            Some(name.to_string()),
        );
    }
    if let Some(p) = labels.get("com.docker.compose.project") {
        return (p.clone(), p.clone(), Some(name.to_string()));
    }
    (name.to_string(), name.to_string(), None)
}

pub fn supabase_project_from_name(name: &str) -> Option<&str> {
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
    path.strip_prefix("/containers/")
        .and_then(|rest| rest.strip_suffix("/json"))
        .is_some_and(|id| hex_id(id).is_some())
}

fn http_2xx(head: &[u8]) -> bool {
    head.split(|&b| b == b' ')
        .nth(1)
        .and_then(|c| c.get(..3))
        .is_some_and(|c| c[0] == b'2' && c[1].is_ascii_digit() && c[2].is_ascii_digit())
}

fn unix_get(sock: &Path, path: &str) -> Result<Vec<u8>, crate::types::Error> {
    if !docker_get_path(path) {
        return Err(crate::types::Error("invalid docker path".into()));
    }
    let mut s = UnixStream::connect(sock)?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    s.set_write_timeout(Some(Duration::from_secs(2)))?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    const MAX: usize = 4 * 1024 * 1024;
    let mut buf = Vec::new();
    s.take(MAX as u64 + 1).read_to_end(&mut buf)?;
    if buf.len() > MAX {
        return Err(crate::types::Error("docker response too large".into()));
    }
    let sep = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| crate::types::Error("short docker response".into()))?;
    if !http_2xx(&buf[..sep]) {
        return Err(crate::types::Error("docker HTTP error".into()));
    }
    Ok(buf[sep + 4..].to_vec())
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListItem {
    #[serde(rename = "Id", default)]
    pub id: String,
    #[serde(rename = "Names", default)]
    pub names: Vec<String>,
    #[serde(rename = "Labels")]
    pub labels: Option<HashMap<String, String>>,
    #[serde(rename = "State")]
    pub state: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct Inspect {
    #[serde(rename = "State")]
    pub state: Option<InspectState>,
    #[serde(rename = "NetworkSettings")]
    pub network: Option<NetworkSettings>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct InspectState {
    #[serde(rename = "Running")]
    pub running: Option<bool>,
    #[serde(rename = "Pid")]
    pub pid: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct NetworkSettings {
    #[serde(rename = "IPAddress")]
    pub ip: Option<String>,
    #[serde(rename = "Networks")]
    pub networks: Option<HashMap<String, Value>>,
}

impl Inspect {
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
        assert_eq!(
            supabase_project_from_name("supabase_db_caldera"),
            Some("caldera")
        );
        let labels = HashMap::from([("com.supabase.cli.project".into(), "caldera".into())]);
        let (k, t, m) = project_identity("supabase_db_caldera", &labels);
        assert_eq!(k, "supabase:caldera");
        assert_eq!(t, "supabase:caldera");
        assert_eq!(m.as_deref(), Some("supabase_db_caldera"));
        let (k, _, m) = project_identity("engined-whisper", &HashMap::new());
        assert_eq!(k, "engined-whisper");
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
            name: "x".into(),
            ident_key: "x".into(),
            ident_title: "x".into(),
            member_name: None,
            ips: Vec::new(),
            owner_uid: None,
            running: true,
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
}
