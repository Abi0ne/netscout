//! Where a device of a scan with a tagged VLAN was found: the network in
//! access (untagged), the VLAN, or neither (link-level discovery only), like
//! the macOS app's `ScanSegment`.

use std::cmp::Ordering;
use std::net::Ipv4Addr;

use netscout_core::profiles::ProfileVlan;
use netscout_core::Host;

/// An IPv4 address with its prefix length ("172.20.20.250/24").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4Cidr {
    pub address: u32,
    pub prefix: u8,
}

impl Ipv4Cidr {
    pub fn parse(text: &str) -> Option<Self> {
        let (ip, prefix) = text.trim().split_once('/')?;
        let ip: Ipv4Addr = ip.trim().parse().ok()?;
        let prefix: u8 = prefix.trim().parse().ok()?;
        (prefix <= 32).then_some(Self {
            address: u32::from(ip),
            prefix,
        })
    }

    pub fn mask(&self) -> u32 {
        if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix)
        }
    }

    pub fn network(&self) -> Self {
        Self {
            address: self.address & self.mask(),
            prefix: self.prefix,
        }
    }

    /// `ip` (dotted) is in this network.
    pub fn contains(&self, ip: &str) -> bool {
        ip.parse::<Ipv4Addr>()
            .is_ok_and(|ip| u32::from(ip) & self.mask() == self.address & self.mask())
    }

    /// The two networks share addresses.
    pub fn overlaps(&self, other: &Self) -> bool {
        let prefix = self.prefix.min(other.prefix);
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        self.address & mask == other.address & mask
    }

    pub fn ip(&self) -> String {
        Ipv4Addr::from(self.address).to_string()
    }

    pub fn netmask(&self) -> String {
        Ipv4Addr::from(self.mask()).to_string()
    }
}

impl std::fmt::Display for Ipv4Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.ip(), self.prefix)
    }
}

/// The tagged VLAN a scan took in: its tag and its network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScannedVlan {
    pub tag: u16,
    pub network: Ipv4Cidr,
}

impl ScannedVlan {
    /// One a saved profile recorded.
    pub fn from_profile(saved: &ProfileVlan) -> Option<Self> {
        Some(Self {
            tag: saved.tag,
            network: Ipv4Cidr::parse(&saved.network)?,
        })
    }

    pub fn to_profile(self) -> ProfileVlan {
        ProfileVlan {
            tag: self.tag,
            network: self.network.to_string(),
        }
    }
}

/// The VLANs of a saved profile.
pub fn profile_vlans(vlans: &[ProfileVlan]) -> Vec<ScannedVlan> {
    vlans.iter().filter_map(ScannedVlan::from_profile).collect()
}

/// `host` belongs to one of `vlans`.
pub fn is_in(vlans: &[ScannedVlan], host: &Host) -> bool {
    !host.off_subnet && vlans.iter().any(|v| v.network.contains(&host.ip))
}

/// The target as written, without the VLAN networks after it.
pub fn native_target(target: &str) -> String {
    target.split(", ").next().unwrap_or(target).to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Segment {
    Native,
    Vlan(u16),
    OffSubnet,
}

impl Segment {
    /// Where `host` was found, for scans that took in `vlans`; `None`
    /// without any.
    pub fn of(host: &Host, vlans: &[ScannedVlan]) -> Option<Segment> {
        if vlans.is_empty() {
            return None;
        }
        if host.off_subnet {
            return Some(Segment::OffSubnet);
        }
        Some(
            vlans
                .iter()
                .find(|v| v.network.contains(&host.ip))
                .map_or(Segment::Native, |v| Segment::Vlan(v.tag)),
        )
    }

    /// `hosts` by network, in order; empty without a VLAN.
    pub fn groups(hosts: &[Host], vlans: &[ScannedVlan]) -> Vec<(Segment, Vec<Host>)> {
        if vlans.is_empty() {
            return Vec::new();
        }
        let mut groups: Vec<(Segment, Vec<Host>)> = Vec::new();
        for host in hosts {
            let segment = Segment::of(host, vlans).unwrap_or(Segment::Native);
            match groups.iter_mut().find(|(s, _)| *s == segment) {
                Some((_, list)) => list.push(host.clone()),
                None => groups.push((segment, vec![host.clone()])),
            }
        }
        groups.sort_by_key(|g| g.0);
        groups
    }

    pub fn label(&self) -> String {
        match self {
            Segment::Native => "Nativa".into(),
            Segment::Vlan(tag) => format!("VLAN {tag}"),
            Segment::OffSubnet => "Fuori rete".into(),
        }
    }

    /// For the table's "VLAN" column: the tag alone.
    pub fn short_label(&self) -> String {
        match self {
            Segment::Vlan(tag) => tag.to_string(),
            _ => self.label(),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Segment::Native => "network-wired-symbolic",
            Segment::Vlan(_) => "netscout-tag-symbolic",
            Segment::OffSubnet => "network-wired-disconnected-symbolic",
        }
    }

    /// A VLAN in the accent colour, off-network devices in the purple of
    /// their "Fuori rete" badge.
    pub fn css_class(&self) -> &'static str {
        match self {
            Segment::Native => "dim-label",
            Segment::Vlan(_) => "accent",
            Segment::OffSubnet => "off-subnet",
        }
    }

    /// The query ("vlan 69") names this network.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        !query.is_empty() && self.label().to_lowercase().contains(&query)
    }

    fn rank(&self) -> (u8, u16) {
        match self {
            Segment::Native => (0, 0),
            Segment::Vlan(tag) => (1, *tag),
            Segment::OffSubnet => (2, 0),
        }
    }
}

impl PartialOrd for Segment {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Segment {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscout_core::DeviceType;

    fn host(ip: &str, off: bool) -> Host {
        Host {
            ip: ip.into(),
            mac: None,
            vendor: None,
            hostnames: vec![],
            device_type: DeviceType::Unknown,
            open_ports: vec![],
            rtt_ms: None,
            mdns_services: vec![],
            ssdp_info: None,
            first_seen: 0,
            last_seen: 0,
            off_subnet: off,
            model: None,
        }
    }

    #[test]
    fn segments_by_network() {
        let vlans = [ScannedVlan {
            tag: 69,
            network: Ipv4Cidr::parse("172.20.20.0/24").unwrap(),
        }];
        assert_eq!(Segment::of(&host("172.20.20.5", false), &[]), None);
        assert_eq!(
            Segment::of(&host("172.20.20.5", false), &vlans),
            Some(Segment::Vlan(69))
        );
        assert_eq!(
            Segment::of(&host("192.168.1.5", false), &vlans),
            Some(Segment::Native)
        );
        assert_eq!(
            Segment::of(&host("172.20.20.5", true), &vlans),
            Some(Segment::OffSubnet)
        );
        let groups = Segment::groups(
            &[
                host("192.168.1.1", true),
                host("172.20.20.9", false),
                host("192.168.1.2", false),
            ],
            &vlans,
        );
        let order: Vec<Segment> = groups.iter().map(|g| g.0).collect();
        assert_eq!(
            order,
            [Segment::Native, Segment::Vlan(69), Segment::OffSubnet]
        );
        assert!(Segment::Vlan(69).matches("vlan 69"));
        assert!(!Segment::Vlan(69).matches("vlan 7"));
    }

    #[test]
    fn cidr() {
        let c = Ipv4Cidr::parse("172.20.20.250/24").unwrap();
        assert_eq!(c.network().to_string(), "172.20.20.0/24");
        assert_eq!(c.netmask(), "255.255.255.0");
        assert!(c.overlaps(&Ipv4Cidr::parse("172.20.0.0/16").unwrap()));
        assert!(!c.overlaps(&Ipv4Cidr::parse("172.20.21.0/24").unwrap()));
        assert_eq!(native_target("10.0.0.0/24, 172.20.20.0/24"), "10.0.0.0/24");
    }
}
