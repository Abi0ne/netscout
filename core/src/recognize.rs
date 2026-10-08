//! Recognizing the network being scanned among the saved profiles.
//!
//! Only identifiers that are stable and unique to one device count:
//!
//! * the **gateway's MAC** — the router is the network's anchor (it does not
//!   roam to other networks), so finding it in a profile is enough;
//! * **globally unique MACs** (universally administered) of other devices.
//!   Locally administered MACs are skipped: phones and tablets make up a
//!   random one per network and rotate it;
//! * the **UPnP UUID** (SSDP `USN`) of a device whose MAC is unknown.
//!
//! IPs, hostnames and vendors are never used: private address ranges are the
//! same on every network, and names like "iPhone" are shared by many devices.
//!
//! Without the gateway, a profile qualifies only when at least
//! [`MIN_DEVICES`] of its devices are found *and* they are at least half of
//! the devices it could be recognized by, so a laptop or two that roam
//! between home and office never tie a scan to the wrong network. When two
//! profiles qualify equally and they are not the same network (same router),
//! nothing is suggested.

use std::collections::HashSet;

use crate::profiles::SavedProfile;
use crate::types::Host;

/// Devices other than the gateway that must be found to recognize a network.
const MIN_DEVICES: u32 = 3;

/// The saved profile the scanned network appears to be.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NetworkMatch {
    pub profile_id: String,
    pub profile_name: String,
    /// The scan's gateway (by MAC) is one of the profile's devices.
    pub gateway_matched: bool,
    /// Devices of the scan found in the profile by a stable identifier,
    /// the gateway included.
    pub matched_devices: u32,
    /// Devices of the profile that have a stable identifier.
    pub profile_devices: u32,
}

/// The profile the network of `current` belongs to, if one is recognized
/// reliably. `gateway_ip` is the default gateway of the scanned network,
/// when known.
#[uniffi::export]
pub fn recognize_network(
    current: &[Host],
    gateway_ip: Option<String>,
    profiles: &[SavedProfile],
) -> Option<NetworkMatch> {
    let gateway_mac = gateway_ip
        .as_deref()
        .and_then(|ip| current.iter().find(|h| h.ip == ip))
        .and_then(|h| h.mac.as_deref())
        .and_then(unicast_mac);
    let scan: Vec<String> = current.iter().filter_map(stable_id).collect();

    let mut candidates: Vec<(&SavedProfile, NetworkMatch)> = Vec::new();
    for profile in profiles {
        let devices: Vec<&Host> = profile.hosts.iter().chain(&profile.offline_hosts).collect();
        let macs: HashSet<String> = devices
            .iter()
            .filter_map(|h| h.mac.as_deref().and_then(unicast_mac))
            .collect();
        let ids: HashSet<String> = devices.iter().filter_map(|h| stable_id(h)).collect();
        let gateway_matched = gateway_mac.as_ref().is_some_and(|m| macs.contains(m));
        let mut matched: HashSet<&str> = scan
            .iter()
            .filter(|id| ids.contains(*id))
            .map(String::as_str)
            .collect();
        if let (true, Some(mac)) = (gateway_matched, &gateway_mac) {
            matched.insert(mac);
        }
        let matched = matched.len() as u32;
        let known = ids.len() as u32;
        let qualifies = gateway_matched || (matched >= MIN_DEVICES && matched * 2 >= known);
        if qualifies {
            candidates.push((
                profile,
                NetworkMatch {
                    profile_id: profile.id.clone(),
                    profile_name: profile.name.clone(),
                    gateway_matched,
                    matched_devices: matched,
                    profile_devices: known,
                },
            ));
        }
    }

    let rank = |m: &NetworkMatch| (m.gateway_matched, m.matched_devices);
    let best = candidates.iter().map(|(_, m)| rank(m)).max()?;
    let tied: Vec<&(&SavedProfile, NetworkMatch)> =
        candidates.iter().filter(|(_, m)| rank(m) == best).collect();
    if tied.len() > 1 && !best.0 {
        // Equal evidence for different networks: ambiguous.
        return None;
    }
    // Several profiles with the same router are the same network: suggest
    // the most recent one.
    tied.into_iter()
        .max_by_key(|(p, _)| p.updated_at.unwrap_or(p.created_at))
        .map(|(_, m)| m.clone())
}

/// A device's stable identifier: its globally unique MAC, or, when the MAC
/// is unknown, its UPnP UUID.
fn stable_id(host: &Host) -> Option<String> {
    match host.mac.as_deref() {
        Some(mac) => unicast_mac(mac).filter(|m| is_universal(m)),
        None => host
            .ssdp_info
            .as_ref()
            .and_then(|s| upnp_uuid(&s.usn))
            .map(|u| format!("uuid:{u}")),
    }
}

/// `mac` normalized, if it is a valid unicast address (not zero, not
/// multicast or broadcast).
fn unicast_mac(mac: &str) -> Option<String> {
    let bytes: Vec<u8> = mac
        .split([':', '-'])
        .map(|b| u8::from_str_radix(b, 16).ok())
        .collect::<Option<_>>()?;
    if bytes.len() != 6 || bytes.iter().all(|&b| b == 0) || bytes[0] & 0x01 != 0 {
        return None;
    }
    Some(
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

/// Universally administered (assigned by the maker), not made up locally.
fn is_universal(mac: &str) -> bool {
    u8::from_str_radix(&mac[..2], 16).is_ok_and(|b| b & 0x02 == 0)
}

/// The UUID of a USN like `uuid:1234-…::urn:schemas-upnp-org:device:…`.
fn upnp_uuid(usn: &str) -> Option<String> {
    let rest = usn.trim().strip_prefix("uuid:")?;
    let uuid = rest.split("::").next()?.trim().to_lowercase();
    // Too short to be unique (some firmwares send placeholders).
    (uuid.chars().filter(char::is_ascii_hexdigit).count() >= 16).then_some(uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DeviceType, ScanProfile, SsdpInfo};
    use std::collections::HashMap;

    fn host(ip: &str, mac: Option<&str>) -> Host {
        Host {
            ip: ip.into(),
            mac: mac.map(Into::into),
            vendor: None,
            hostnames: vec![],
            device_type: DeviceType::Computer,
            open_ports: vec![],
            rtt_ms: None,
            mdns_services: vec![],
            ssdp_info: None,
            first_seen: 0,
            last_seen: 0,
        }
    }

    fn profile(id: &str, hosts: Vec<Host>) -> SavedProfile {
        SavedProfile {
            id: id.into(),
            name: format!("P{id}"),
            created_at: id.parse().unwrap(),
            target: "192.168.1.0/24".into(),
            scan_profile: ScanProfile::Standard,
            hosts,
            offline_hosts: vec![],
            updated_at: None,
            notes: HashMap::new(),
        }
    }

    const GW: &str = "00:11:22:00:00:01";

    #[test]
    fn gateway_mac_recognizes_the_network() {
        let p = profile("1", vec![host("192.168.1.1", Some(GW))]);
        let scan = vec![host("192.168.1.1", Some("00:11:22:00:00:01"))];
        let m = recognize_network(&scan, Some("192.168.1.1".into()), &[p]).unwrap();
        assert!(m.gateway_matched);
        assert_eq!(m.profile_id, "1");
    }

    #[test]
    fn same_ip_different_router_is_not_recognized() {
        let p = profile("1", vec![host("192.168.1.1", Some(GW))]);
        let scan = vec![host("192.168.1.1", Some("00:99:99:00:00:01"))];
        assert!(recognize_network(&scan, Some("192.168.1.1".into()), &[p]).is_none());
    }

    #[test]
    fn a_roaming_laptop_alone_is_not_enough() {
        let laptop = "00:aa:00:00:00:01";
        let office = profile(
            "1",
            vec![
                host("10.0.0.5", Some(laptop)),
                host("10.0.0.6", Some("00:aa:00:00:00:02")),
                host("10.0.0.7", Some("00:aa:00:00:00:03")),
            ],
        );
        let scan = vec![host("192.168.1.20", Some(laptop))];
        assert!(recognize_network(&scan, None, &[office]).is_none());
    }

    #[test]
    fn enough_devices_recognize_without_gateway() {
        let devices: Vec<Host> = (1..=4)
            .map(|i| {
                host(
                    &format!("10.0.0.{i}"),
                    Some(&format!("00:aa:00:00:00:0{i}")),
                )
            })
            .collect();
        let p = profile("1", devices.clone());
        let m = recognize_network(&devices[..3], None, &[p]).unwrap();
        assert_eq!((m.matched_devices, m.profile_devices), (3, 4));
        assert!(!m.gateway_matched);
    }

    #[test]
    fn three_of_many_is_too_weak() {
        let devices: Vec<Host> = (1..=10)
            .map(|i| {
                host(
                    &format!("10.0.0.{i}"),
                    Some(&format!("00:aa:00:00:00:{i:02x}")),
                )
            })
            .collect();
        let p = profile("1", devices.clone());
        assert!(recognize_network(&devices[..3], None, &[p]).is_none());
    }

    #[test]
    fn private_macs_do_not_count() {
        // 0x02 bit set: locally administered.
        let devices: Vec<Host> = (1..=4)
            .map(|i| {
                host(
                    &format!("10.0.0.{i}"),
                    Some(&format!("da:aa:00:00:00:0{i}")),
                )
            })
            .collect();
        let p = profile("1", devices.clone());
        assert!(recognize_network(&devices, None, &[p]).is_none());
    }

    #[test]
    fn upnp_uuid_stands_in_for_an_unknown_mac() {
        let with_uuid = |ip: &str, n: u8| {
            let mut h = host(ip, None);
            h.ssdp_info = Some(SsdpInfo {
                usn: format!("uuid:4d696e69-444c-164e-9d41-0000000000{n:02x}::upnp:rootdevice"),
                location: String::new(),
                server: None,
                friendly_name: None,
            });
            h
        };
        let devices: Vec<Host> = (1..=3)
            .map(|i| with_uuid(&format!("10.0.0.{i}"), i))
            .collect();
        let p = profile("1", devices.clone());
        assert_eq!(
            recognize_network(&devices, None, &[p]).map(|m| m.matched_devices),
            Some(3)
        );
    }

    #[test]
    fn equal_evidence_for_two_networks_is_ambiguous() {
        let devices: Vec<Host> = (1..=3)
            .map(|i| {
                host(
                    &format!("10.0.0.{i}"),
                    Some(&format!("00:aa:00:00:00:0{i}")),
                )
            })
            .collect();
        let a = profile("1", devices.clone());
        let b = profile("2", devices.clone());
        assert!(recognize_network(&devices, None, &[a, b]).is_none());
    }

    #[test]
    fn snapshots_of_the_same_router_pick_the_most_recent() {
        let gw = host("192.168.1.1", Some(GW));
        let old = profile("1", vec![gw.clone()]);
        let new = profile("2", vec![gw.clone()]);
        let m = recognize_network(&[gw], Some("192.168.1.1".into()), &[old, new]).unwrap();
        assert_eq!(m.profile_id, "2");
    }

    #[test]
    fn more_evidence_wins() {
        let gw = host("192.168.1.1", Some(GW));
        let nas = host("192.168.1.2", Some("00:aa:00:00:00:02"));
        let small = profile("1", vec![gw.clone()]);
        let full = profile("2", vec![gw.clone(), nas.clone()]);
        let m = recognize_network(&[gw, nas], Some("192.168.1.1".into()), &[full, small]).unwrap();
        assert_eq!((m.profile_id.as_str(), m.matched_devices), ("2", 2));
    }
}
