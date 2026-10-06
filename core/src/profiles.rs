//! Saved scan profiles and scan-to-profile comparison.
//!
//! A *profile* is a named snapshot of a finished scan (its hosts plus what was
//! scanned and how), stored as one JSON file per profile in a directory the
//! platform chooses (Application Support on macOS, `filesDir` on Android).
//! A profile also remembers devices that were known but off when it was
//! saved (`offline_hosts`), so they can still be woken (Wake-on-LAN) and are
//! not reported as new when they come back. [`diff_hosts`] compares a scan
//! against a profile.
//!
//! ```text
//! let store = open_profile_store(dir)?;
//! let saved = store.save("Ufficio", "192.168.1.0/24", ScanProfile::Standard, hosts)?;
//! let baseline = store.load(saved.id)?;
//! let diff = diff_hosts(baseline.hosts, baseline.offline_hosts, new_hosts);
//! store.update(saved.id, target, profile, new_hosts, now_off)?;
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
    /// Devices up when the profile was saved.
    pub hosts: Vec<Host>,
    /// Devices known from earlier scans but off when the profile was saved.
    #[serde(default)]
    pub offline_hosts: Vec<Host>,
    /// Epoch ms of the last [`ProfileStore::update`], if any.
    #[serde(default)]
    pub updated_at: Option<i64>,
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
    pub offline_count: u32,
    pub updated_at: Option<i64>,
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
            offline_count: p.offline_hosts.len() as u32,
            updated_at: p.updated_at,
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

    /// Save a scan as a new profile called `name`: the `hosts` found up and
    /// the `offline_hosts` known to be off.
    pub fn save(
        self: Arc<Self>,
        name: String,
        target: String,
        scan_profile: ScanProfile,
        hosts: Vec<Host>,
        offline_hosts: Vec<Host>,
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
            offline_hosts,
            updated_at: None,
        };
        self.write(&profile)?;
        Ok(ProfileSummary::from(&profile))
    }

    /// Replace the devices of profile `id` with a newer scan, keeping its
    /// name and creation date.
    pub fn update(
        self: Arc<Self>,
        id: String,
        target: String,
        scan_profile: ScanProfile,
        hosts: Vec<Host>,
        offline_hosts: Vec<Host>,
    ) -> Result<ProfileSummary, ScanError> {
        let mut profile = read_file(&self.path(&id)?)?;
        profile.target = target;
        profile.scan_profile = scan_profile;
        profile.hosts = hosts;
        profile.offline_hosts = offline_hosts;
        profile.updated_at = Some(now_ms());
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
    /// The profile had it as off; it is up again.
    pub came_back: bool,
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
    /// Up in the profile, not found now: off, or gone.
    pub removed: Vec<Host>,
    /// Off in the profile and still not found.
    pub still_offline: Vec<Host>,
    /// Found in both. Includes devices back up (`came_back`) and devices
    /// that moved to another IP (`ip_changed`, matched by MAC).
    pub changed: Vec<HostChange>,
    /// The profile's device at an IP (`before`) and a different device, by
    /// MAC, at that IP now (`after`). The old device is not found elsewhere.
    pub replaced: Vec<HostChange>,
    /// In both, identical.
    pub unchanged: u32,
}

/// Compare a profile (`baseline` up, `baseline_offline` off) with a new scan
/// (`current`).
///
/// Devices are matched by MAC first, so a device that got a new DHCP lease is
/// reported as "IP changed" rather than as one removed and one added device.
/// Hosts left over are matched by IP; when both sides have a MAC and the
/// MACs differ, a different device now uses the address (`replaced`).
#[uniffi::export]
pub fn diff_hosts(
    baseline: Vec<Host>,
    baseline_offline: Vec<Host>,
    current: Vec<Host>,
) -> ScanDiff {
    // (host, was it off in the profile)
    let mut old: Vec<Option<(Host, bool)>> = baseline
        .into_iter()
        .map(|h| Some((h, false)))
        .chain(baseline_offline.into_iter().map(|h| Some((h, true))))
        .collect();
    let mut pairs: Vec<(Host, bool, Host)> = Vec::new();
    let mut unmatched: Vec<Host> = Vec::new();

    let mut by_mac: HashMap<String, usize> = HashMap::new();
    for (i, entry) in old.iter().enumerate() {
        if let Some(mac) = entry.as_ref().and_then(|(h, _)| h.mac.as_deref()) {
            // Up devices first: an off copy of the same MAC never shadows them.
            by_mac.entry(norm_mac(mac)).or_insert(i);
        }
    }
    for host in current {
        let slot = host
            .mac
            .as_deref()
            .map(norm_mac)
            .and_then(|m| by_mac.get(&m));
        match slot.and_then(|&i| old[i].take()) {
            Some((before, off)) => pairs.push((before, off, host)),
            None => unmatched.push(host),
        }
    }

    let mut added = Vec::new();
    let mut replaced = Vec::new();
    for host in unmatched {
        let slot = old
            .iter()
            .position(|o| o.as_ref().is_some_and(|(b, _)| b.ip == host.ip));
        match slot.and_then(|i| old[i].take()) {
            Some((before, off)) => {
                let conflict = matches!((&before.mac, &host.mac),
                    (Some(x), Some(y)) if norm_mac(x) != norm_mac(y));
                if conflict {
                    replaced.push(change(before, false, host));
                } else {
                    pairs.push((before, off, host));
                }
            }
            None => added.push(host),
        }
    }

    let mut changed = Vec::new();
    let mut unchanged = 0;
    for (before, off, after) in pairs {
        let c = change(before, off, after);
        if c.differs() {
            changed.push(c);
        } else {
            unchanged += 1;
        }
    }

    let mut removed = Vec::new();
    let mut still_offline = Vec::new();
    for (host, off) in old.into_iter().flatten() {
        if off {
            still_offline.push(host);
        } else {
            removed.push(host);
        }
    }
    added.sort_by_key(|h| ip_key(&h.ip));
    removed.sort_by_key(|h| ip_key(&h.ip));
    still_offline.sort_by_key(|h| ip_key(&h.ip));
    changed.sort_by_key(|c| ip_key(&c.after.ip));
    replaced.sort_by_key(|c| ip_key(&c.after.ip));
    ScanDiff {
        added,
        removed,
        still_offline,
        changed,
        replaced,
        unchanged,
    }
}

/// The differences between the two sides of a matched device.
fn change(before: Host, came_back: bool, after: Host) -> HostChange {
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

    HostChange {
        came_back,
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
    }
}

impl HostChange {
    fn differs(&self) -> bool {
        self.came_back
            || self.ip_changed
            || self.mac_changed
            || self.vendor_changed
            || self.device_type_changed
            || !self.opened_ports.is_empty()
            || !self.closed_ports.is_empty()
            || !self.added_hostnames.is_empty()
            || !self.removed_hostnames.is_empty()
    }
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
        let d = diff_hosts(a, vec![], b);
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
        let d = diff_hosts(old, vec![], new);
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
        let d = diff_hosts(old, vec![], new);
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert!(d.changed[0].ip_changed);
    }

    #[test]
    fn different_mac_on_same_ip_is_a_replacement() {
        let old = vec![host("10.0.0.5", Some("aa:00:00:00:00:05"), &[])];
        let new = vec![host("10.0.0.5", Some("bb:00:00:00:00:05"), &[])];
        let d = diff_hosts(old, vec![], new);
        assert_eq!((d.added.len(), d.removed.len(), d.changed.len()), (0, 0, 0));
        assert_eq!(d.replaced.len(), 1);
        assert_eq!(
            d.replaced[0].before.mac.as_deref(),
            Some("aa:00:00:00:00:05")
        );
        assert_eq!(
            d.replaced[0].after.mac.as_deref(),
            Some("bb:00:00:00:00:05")
        );
    }

    #[test]
    fn moved_device_frees_its_old_ip_for_another() {
        // A moved to .9; B took A's old .5: one IP change, one new device.
        let old = vec![host("10.0.0.5", Some("aa:00:00:00:00:0a"), &[])];
        let new = vec![
            host("10.0.0.9", Some("aa:00:00:00:00:0a"), &[]),
            host("10.0.0.5", Some("bb:00:00:00:00:0b"), &[]),
        ];
        let d = diff_hosts(old, vec![], new);
        assert!(d.replaced.is_empty() && d.removed.is_empty());
        assert_eq!(d.added.len(), 1);
        assert!(d.changed[0].ip_changed);
    }

    #[test]
    fn offline_devices_stay_offline_or_come_back() {
        let up = vec![host("10.0.0.1", Some("aa:00:00:00:00:01"), &[])];
        let off = vec![
            host("10.0.0.2", Some("aa:00:00:00:00:02"), &[]),
            host("10.0.0.3", Some("aa:00:00:00:00:03"), &[]),
        ];
        let new = vec![
            host("10.0.0.1", Some("aa:00:00:00:00:01"), &[]),
            host("10.0.0.3", Some("aa:00:00:00:00:03"), &[]),
        ];
        let d = diff_hosts(up, off, new);
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert_eq!(d.still_offline.len(), 1);
        assert_eq!(d.still_offline[0].ip, "10.0.0.2");
        assert_eq!(d.changed.len(), 1);
        assert!(d.changed[0].came_back);
        assert_eq!(d.unchanged, 1);
    }

    #[test]
    fn missing_mac_now_is_not_a_change() {
        let old = vec![host("10.0.0.5", Some("aa:00:00:00:00:05"), &[])];
        let new = vec![host("10.0.0.5", None, &[])];
        let d = diff_hosts(old, vec![], new);
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
                vec![],
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

        let updated = store
            .clone()
            .update(
                saved.id.clone(),
                "10.0.0.0/24".into(),
                ScanProfile::Deep,
                vec![],
                vec![host("10.0.0.1", Some("aa:00:00:00:00:01"), &[80])],
            )
            .unwrap();
        assert_eq!(updated.name, "Ufficio");
        assert_eq!((updated.host_count, updated.offline_count), (0, 1));
        assert!(updated.updated_at.is_some());
        assert_eq!(updated.created_at, saved.created_at);

        assert!(store.clone().load("../x".into()).is_err());
        store.clone().delete(saved.id).unwrap();
        assert!(store.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}
