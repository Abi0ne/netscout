//! Saved scan profiles and scan-to-profile comparison.
//!
//! A *profile* is a named snapshot of a finished scan (its hosts plus what was
//! scanned and how), stored as one JSON file per profile in a directory the
//! platform chooses (Application Support on macOS, `filesDir` on Android).
//! [`diff_hosts`] compares a scan against a profile's hosts.
//!
//! ```text
//! let store = open_profile_store(dir)?;
//! let saved = store.save("Ufficio", "192.168.1.0/24", ScanProfile::Standard, hosts)?;
//! let baseline = store.load(saved.id)?;
//! let diff = diff_hosts(baseline.hosts, new_hosts);
//! ```

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::ScanError;
use crate::types::{DeviceType, Host, PortState, ScanProfile};

/// Bumped when the on-disk format changes incompatibly.
const FORMAT_VERSION: u32 = 1;

/// A saved scan, with every host as it was when saved.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct SavedProfile {
    pub id: String,
    pub name: String,
    /// Epoch ms when the profile was saved.
    pub created_at: i64,
    /// What was scanned (CIDR, IP, range…), as the user entered it.
    pub target: String,
    /// The scan depth used; comparing scans of different depths reports
    /// ports that were simply not probed as closed.
    pub scan_profile: ScanProfile,
    pub hosts: Vec<Host>,
}

/// A profile without its hosts, for listings.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub target: String,
    pub scan_profile: ScanProfile,
    pub host_count: u32,
}

impl From<&SavedProfile> for ProfileSummary {
    fn from(p: &SavedProfile) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            created_at: p.created_at,
            target: p.target.clone(),
            scan_profile: p.scan_profile,
            host_count: p.hosts.len() as u32,
        }
    }
}

/// On-disk envelope.
#[derive(Serialize, Deserialize)]
struct ProfileFile {
    version: u32,
    profile: SavedProfile,
}

/// A directory of saved profiles. Construct it with [`open_profile_store`].
#[derive(uniffi::Object)]
pub struct ProfileStore {
    dir: PathBuf,
}

/// Open (creating it if needed) the profile directory `dir`.
#[uniffi::export]
pub fn open_profile_store(dir: String) -> Result<Arc<ProfileStore>, ScanError> {
    let dir = PathBuf::from(dir);
    fs::create_dir_all(&dir).map_err(|e| io_error("create profile directory", &dir, e))?;
    Ok(Arc::new(ProfileStore { dir }))
}

#[uniffi::export]
impl ProfileStore {
    /// Every readable profile, newest first. Unreadable files are skipped.
    pub fn list(self: Arc<Self>) -> Result<Vec<ProfileSummary>, ScanError> {
        let entries = fs::read_dir(&self.dir)
            .map_err(|e| io_error("read profile directory", &self.dir, e))?;
        let mut out: Vec<ProfileSummary> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| read_file(&p).ok())
            .map(|p| ProfileSummary::from(&p))
            .collect();
        out.sort_by_key(|p| std::cmp::Reverse(p.created_at));
        Ok(out)
    }

    pub fn load(self: Arc<Self>, id: String) -> Result<SavedProfile, ScanError> {
        read_file(&self.path(&id)?)
    }

    /// Save `hosts` as a new profile called `name`.
    pub fn save(
        self: Arc<Self>,
        name: String,
        target: String,
        scan_profile: ScanProfile,
        hosts: Vec<Host>,
    ) -> Result<ProfileSummary, ScanError> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(ScanError::InvalidConfig("the profile needs a name".into()));
        }
        let created_at = now_ms();
        // Millisecond ids; bump past an existing file in the unlikely clash.
        let mut n = created_at;
        while self.dir.join(format!("{n}.json")).exists() {
            n += 1;
        }
        let profile = SavedProfile {
            id: n.to_string(),
            name,
            created_at,
            target,
            scan_profile,
            hosts,
        };
        self.write(&profile)?;
        Ok(ProfileSummary::from(&profile))
    }

    pub fn rename(self: Arc<Self>, id: String, name: String) -> Result<(), ScanError> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(ScanError::InvalidConfig("the profile needs a name".into()));
        }
        let mut profile = read_file(&self.path(&id)?)?;
        profile.name = name;
        self.write(&profile)
    }

    pub fn delete(self: Arc<Self>, id: String) -> Result<(), ScanError> {
        let path = self.path(&id)?;
        fs::remove_file(&path).map_err(|e| io_error("delete profile", &path, e))
    }
}

impl ProfileStore {
    /// The file of profile `id`. Ids are generated digits; anything else is
    /// rejected so an id can never reach outside the directory.
    fn path(&self, id: &str) -> Result<PathBuf, ScanError> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ScanError::InvalidConfig(format!(
                "invalid profile id {id:?}"
            )));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    /// Write through a temporary file and rename, so a crash never leaves a
    /// half-written profile.
    fn write(&self, profile: &SavedProfile) -> Result<(), ScanError> {
        let path = self.path(&profile.id)?;
        let tmp = path.with_extension("json.tmp");
        let file = ProfileFile {
            version: FORMAT_VERSION,
            profile: profile.clone(),
        };
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|e| ScanError::Internal(format!("encode profile: {e}")))?;
        fs::write(&tmp, json).map_err(|e| io_error("write profile", &tmp, e))?;
        fs::rename(&tmp, &path).map_err(|e| io_error("write profile", &path, e))
    }
}

fn read_file(path: &Path) -> Result<SavedProfile, ScanError> {
    let bytes = fs::read(path).map_err(|e| io_error("read profile", path, e))?;
    let file: ProfileFile = serde_json::from_slice(&bytes)
        .map_err(|e| ScanError::Internal(format!("{}: invalid profile: {e}", path.display())))?;
    if file.version > FORMAT_VERSION {
        return Err(ScanError::Unsupported(format!(
            "{}: profile format {} is newer than this app",
            path.display(),
            file.version
        )));
    }
    Ok(file.profile)
}

fn io_error(what: &str, path: &Path, e: std::io::Error) -> ScanError {
    ScanError::Internal(format!("{what} {}: {e}", path.display()))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// How one device differs between the profile and the new scan. Only the
/// fields that describe the device are compared; latency, timestamps and
/// mDNS/SSDP details change from scan to scan and are ignored.
#[derive(Debug, Clone, uniffi::Record)]
pub struct HostChange {
    /// The device as saved in the profile.
    pub before: Host,
    /// The device as found now.
    pub after: Host,
    pub ip_changed: bool,
    pub mac_changed: bool,
    pub vendor_changed: bool,
    pub device_type_changed: bool,
    /// Open now, not in the profile.
    pub opened_ports: Vec<u16>,
    /// Open in the profile, not now.
    pub closed_ports: Vec<u16>,
    pub added_hostnames: Vec<String>,
    pub removed_hostnames: Vec<String>,
}

/// Result of comparing a scan against a saved profile.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScanDiff {
    /// In the new scan only.
    pub added: Vec<Host>,
    /// In the profile only.
    pub removed: Vec<Host>,
    /// In both, with differences.
    pub changed: Vec<HostChange>,
    /// In both, identical.
    pub unchanged: u32,
}

/// Compare a profile's hosts (`baseline`) with a new scan (`current`).
///
/// Devices are matched by MAC first, so a device that got a new DHCP lease is
/// reported as "IP changed" rather than as one removed and one added device.
/// Hosts left over are matched by IP, unless both sides have a MAC and the
/// MACs differ: that is a different device on a reused address.
#[uniffi::export]
pub fn diff_hosts(baseline: Vec<Host>, current: Vec<Host>) -> ScanDiff {
    let mut old: Vec<Option<Host>> = baseline.into_iter().map(Some).collect();
    let mut pairs: Vec<(Host, Host)> = Vec::new();
    let mut unmatched: Vec<Host> = Vec::new();

    let by_mac: HashMap<String, usize> = old
        .iter()
        .enumerate()
        .filter_map(|(i, h)| Some((norm_mac(h.as_ref()?.mac.as_deref()?), i)))
        .collect();
    for host in current {
        let slot = host
            .mac
            .as_deref()
            .map(norm_mac)
            .and_then(|m| by_mac.get(&m));
        match slot.and_then(|&i| old[i].take()) {
            Some(before) => pairs.push((before, host)),
            None => unmatched.push(host),
        }
    }

    let mut added = Vec::new();
    for host in unmatched {
        let slot = old.iter().position(|o| {
            o.as_ref().is_some_and(|b| {
                b.ip == host.ip
                    && !matches!((&b.mac, &host.mac), (Some(x), Some(y)) if norm_mac(x) != norm_mac(y))
            })
        });
        match slot.and_then(|i| old[i].take()) {
            Some(before) => pairs.push((before, host)),
            None => added.push(host),
        }
    }

    let mut changed = Vec::new();
    let mut unchanged = 0;
    for (before, after) in pairs {
        match compare(before, after) {
            Some(change) => changed.push(change),
            None => unchanged += 1,
        }
    }

    let mut removed: Vec<Host> = old.into_iter().flatten().collect();
    added.sort_by_key(|h| ip_key(&h.ip));
    removed.sort_by_key(|h| ip_key(&h.ip));
    changed.sort_by_key(|c| ip_key(&c.after.ip));
    ScanDiff {
        added,
        removed,
        changed,
        unchanged,
    }
}

/// `Some` when the two sides of a matched device differ.
fn compare(before: Host, after: Host) -> Option<HostChange> {
    let ports = |h: &Host| -> BTreeSet<u16> {
        h.open_ports
            .iter()
            .filter(|p| p.state == PortState::Open)
            .map(|p| p.number)
            .collect()
    };
    let names =
        |h: &Host| -> BTreeSet<String> { h.hostnames.iter().map(|n| n.to_lowercase()).collect() };
    let (old_ports, new_ports) = (ports(&before), ports(&after));
    let (old_names, new_names) = (names(&before), names(&after));
    // A missing value on the new side is "not learned this time", not a
    // change (MAC/vendor need ARP, the type may still be unknown).
    let learned_differently = |a: &Option<String>, b: &Option<String>| match (a, b) {
        (Some(x), Some(y)) => x.to_lowercase() != y.to_lowercase(),
        (None, Some(_)) => true,
        _ => false,
    };

    let change = HostChange {
        ip_changed: before.ip != after.ip,
        mac_changed: learned_differently(&before.mac, &after.mac),
        vendor_changed: learned_differently(&before.vendor, &after.vendor),
        device_type_changed: before.device_type != after.device_type
            && after.device_type != DeviceType::Unknown,
        opened_ports: new_ports.difference(&old_ports).copied().collect(),
        closed_ports: old_ports.difference(&new_ports).copied().collect(),
        added_hostnames: new_names.difference(&old_names).cloned().collect(),
        removed_hostnames: old_names.difference(&new_names).cloned().collect(),
        before,
        after,
    };
    let differs = change.ip_changed
        || change.mac_changed
        || change.vendor_changed
        || change.device_type_changed
        || !change.opened_ports.is_empty()
        || !change.closed_ports.is_empty()
        || !change.added_hostnames.is_empty()
        || !change.removed_hostnames.is_empty();
    differs.then_some(change)
}

fn norm_mac(mac: &str) -> String {
    mac.to_ascii_lowercase()
}

fn ip_key(ip: &str) -> u32 {
    ip.parse::<std::net::Ipv4Addr>()
        .map(u32::from)
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Port, Transport};

    fn host(ip: &str, mac: Option<&str>, ports: &[u16]) -> Host {
        Host {
            ip: ip.into(),
            mac: mac.map(Into::into),
            vendor: None,
            hostnames: vec![],
            device_type: DeviceType::Computer,
            open_ports: ports
                .iter()
                .map(|&number| Port {
                    number,
                    transport: Transport::Tcp,
                    state: PortState::Open,
                    service: None,
                    version: None,
                })
                .collect(),
            rtt_ms: Some(1.0),
            mdns_services: vec![],
            ssdp_info: None,
            first_seen: 0,
            last_seen: 0,
        }
    }

    #[test]
    fn identical_scans_have_no_differences() {
        let a = vec![host("10.0.0.1", Some("aa:00:00:00:00:01"), &[80])];
        let mut b = a.clone();
        b[0].rtt_ms = Some(9.0);
        b[0].last_seen = 42;
        let d = diff_hosts(a, b);
        assert!(d.added.is_empty() && d.removed.is_empty() && d.changed.is_empty());
        assert_eq!(d.unchanged, 1);
    }

    #[test]
    fn added_removed_and_port_changes() {
        let old = vec![
            host("10.0.0.1", Some("aa:00:00:00:00:01"), &[22, 80]),
            host("10.0.0.2", None, &[]),
        ];
        let new = vec![
            host("10.0.0.1", Some("AA:00:00:00:00:01"), &[80, 443]),
            host("10.0.0.3", None, &[]),
        ];
        let d = diff_hosts(old, new);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].ip, "10.0.0.3");
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].ip, "10.0.0.2");
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].opened_ports, vec![443]);
        assert_eq!(d.changed[0].closed_ports, vec![22]);
        assert!(!d.changed[0].ip_changed);
    }

    #[test]
    fn same_mac_on_new_ip_is_an_ip_change() {
        let old = vec![host("10.0.0.5", Some("aa:00:00:00:00:05"), &[])];
        let new = vec![host("10.0.0.9", Some("aa:00:00:00:00:05"), &[])];
        let d = diff_hosts(old, new);
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert!(d.changed[0].ip_changed);
    }

    #[test]
    fn different_mac_on_same_ip_is_a_different_device() {
        let old = vec![host("10.0.0.5", Some("aa:00:00:00:00:05"), &[])];
        let new = vec![host("10.0.0.5", Some("bb:00:00:00:00:05"), &[])];
        let d = diff_hosts(old, new);
        assert_eq!((d.added.len(), d.removed.len(), d.changed.len()), (1, 1, 0));
    }

    #[test]
    fn missing_mac_now_is_not_a_change() {
        let old = vec![host("10.0.0.5", Some("aa:00:00:00:00:05"), &[])];
        let new = vec![host("10.0.0.5", None, &[])];
        let d = diff_hosts(old, new);
        assert_eq!(d.unchanged, 1);
    }

    #[test]
    fn store_round_trip() {
        let dir = std::env::temp_dir().join(format!("netscout-profiles-{}", now_ms()));
        let store = open_profile_store(dir.to_string_lossy().into()).unwrap();
        let saved = store
            .clone()
            .save(
                " Casa ".into(),
                "10.0.0.0/24".into(),
                ScanProfile::Standard,
                vec![host("10.0.0.1", Some("aa:00:00:00:00:01"), &[80])],
            )
            .unwrap();
        assert_eq!(saved.name, "Casa");
        assert_eq!(saved.host_count, 1);

        let list = store.clone().list().unwrap();
        assert_eq!(list.len(), 1);
        let loaded = store.clone().load(saved.id.clone()).unwrap();
        assert_eq!(loaded.hosts[0].open_ports[0].number, 80);

        store
            .clone()
            .rename(saved.id.clone(), "Ufficio".into())
            .unwrap();
        assert_eq!(store.clone().list().unwrap()[0].name, "Ufficio");

        assert!(store.clone().load("../x".into()).is_err());
        store.clone().delete(saved.id).unwrap();
        assert!(store.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}
