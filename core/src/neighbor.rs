//! The switch at the other end of the Mac's cable, from its LLDP or CDP
//! announcement.
//!
//! Switches announce themselves on every port, about every 30 s (LLDP) or
//! 60 s (CDP, Cisco): name, model, the port the frame left from, management
//! address, VLANs. The frames never cross a switch, so what arrives is
//! always the device directly connected: usually the switch, but it can be
//! an IP phone or a small switch in between.
//!
//! Capturing needs privileges, so the app's root helper writes the frames
//! it hears to a pcap file and [`parse_link_neighbor_pcap`] reads it here.

use serde::{Deserialize, Serialize};

/// What the device at the other end of the cable announced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct LinkNeighbor {
    /// "LLDP" or "CDP".
    pub protocol: String,
    /// Its name ("RACK1P-SW48").
    pub system_name: Option<String>,
    /// What it says it is: LLDP's system description, CDP's platform and
    /// software ("HPE Networking Instant On Switch 48p … 1830 JL814A, …").
    pub description: Option<String>,
    /// Model, when announced separately (LLDP-MED inventory, CDP platform).
    pub model: Option<String>,
    /// The port of the neighbour the Mac is plugged into ("19", "Gi1/0/14").
    pub port: Option<String>,
    /// The port's description, often the name an administrator gave it.
    pub port_description: Option<String>,
    /// The neighbour's identifier, usually its MAC ("14:ab:ec:5d:54:80").
    pub chassis_id: Option<String>,
    /// Where to manage it.
    pub management_ip: Option<String>,
    /// What it is, in plain words: "Switch", "Router", "Telefono",
    /// "Access point", …, the enabled ones only.
    pub capabilities: Vec<String>,
    /// The port's untagged VLAN.
    pub native_vlan: Option<u16>,
    /// The VLANs announced on the port, by tag.
    pub vlans: Vec<NeighborVlan>,
}

/// A VLAN a neighbour announced on the port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct NeighborVlan {
    pub tag: u16,
    pub name: Option<String>,
}

/// The first LLDP or CDP announcement in `pcap` (a capture file, Ethernet
/// link type), if any.
#[uniffi::export]
pub fn parse_link_neighbor_pcap(pcap: Vec<u8>) -> Option<LinkNeighbor> {
    frames(&pcap)?.into_iter().find_map(parse_frame)
}

/// The frames of a classic pcap file, Ethernet only.
fn frames(data: &[u8]) -> Option<Vec<&[u8]>> {
    let magic = data.get(0..4)?;
    let little = match magic {
        [0xd4, 0xc3, 0xb2, 0xa1] | [0x4d, 0x3c, 0xb2, 0xa1] => true,
        [0xa1, 0xb2, 0xc3, 0xd4] | [0xa1, 0xb2, 0x3c, 0x4d] => false,
        _ => return None,
    };
    let u32_at = |b: &[u8], i: usize| -> Option<u32> {
        let bytes: [u8; 4] = b.get(i..i + 4)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
    };
    if u32_at(data, 20)? != 1 {
        return None; // not Ethernet
    }
    let mut out = Vec::new();
    let mut at = 24;
    while at + 16 <= data.len() {
        let len = u32_at(data, at + 8)? as usize;
        let start = at + 16;
        let Some(frame) = data.get(start..start + len) else { break };
        out.push(frame);
        at = start + len;
    }
    Some(out)
}

fn parse_frame(frame: &[u8]) -> Option<LinkNeighbor> {
    let dst = frame.get(0..6)?;
    let mut at = 12;
    let mut kind = u16::from_be_bytes(frame.get(at..at + 2)?.try_into().ok()?);
    if kind == 0x8100 {
        at += 4;
        kind = u16::from_be_bytes(frame.get(at..at + 2)?.try_into().ok()?);
    }
    let payload = frame.get(at + 2..)?;
    if kind == 0x88cc {
        return parse_lldp(payload);
    }
    // CDP: 802.3 length, then LLC/SNAP with Cisco's OUI and protocol 0x2000.
    if dst == [0x01, 0x00, 0x0c, 0xcc, 0xcc, 0xcc]
        && kind <= 1500
        && payload.get(0..8)? == [0xaa, 0xaa, 0x03, 0x00, 0x00, 0x0c, 0x20, 0x00]
    {
        return parse_cdp(payload.get(8..)?);
    }
    None
}

fn parse_lldp(mut data: &[u8]) -> Option<LinkNeighbor> {
    let mut n = LinkNeighbor { protocol: "LLDP".into(), ..Default::default() };
    while data.len() >= 2 {
        let header = u16::from_be_bytes([data[0], data[1]]);
        let (kind, len) = (header >> 9, (header & 0x1ff) as usize);
        let Some(value) = data.get(2..2 + len) else { break };
        data = &data[2 + len..];
        match kind {
            0 => break,
            1 => n.chassis_id = id_value(value),
            2 => n.port = id_value(value),
            4 => n.port_description = text(value),
            5 => n.system_name = text(value),
            6 => n.description = text(value),
            7 if value.len() >= 4 => {
                let enabled = u16::from_be_bytes([value[2], value[3]]);
                n.capabilities = lldp_capabilities(enabled);
            }
            8 => {
                // Address string length (subtype included), subtype, address.
                if n.management_ip.is_none() && value.len() >= 6 && value[0] == 5 && value[1] == 1 {
                    n.management_ip = Some(ipv4(&value[2..6]));
                }
            }
            127 if value.len() >= 4 => {
                let (oui, subtype, rest) = (&value[0..3], value[3], &value[4..]);
                match (oui, subtype) {
                    // IEEE 802.1: port VLAN ID, VLAN name.
                    ([0x00, 0x80, 0xc2], 1) if rest.len() >= 2 => {
                        n.native_vlan = Some(u16::from_be_bytes([rest[0], rest[1]])).filter(|&v| v > 0);
                    }
                    ([0x00, 0x80, 0xc2], 3) if rest.len() >= 3 => {
                        let tag = u16::from_be_bytes([rest[0], rest[1]]);
                        let name_len = rest[2] as usize;
                        let name = rest.get(3..3 + name_len).and_then(text);
                        n.vlans.push(NeighborVlan { tag, name });
                    }
                    // LLDP-MED inventory: model name.
                    ([0x00, 0x12, 0xbb], 10) => n.model = text(rest),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    finish(n)
}

/// Chassis and port ids: a MAC (subtype 3 or 4, depending on the TLV) as
/// "aa:bb:…", a network address as dotted IPv4, the rest as text.
fn id_value(value: &[u8]) -> Option<String> {
    let (&subtype, rest) = value.split_first()?;
    if rest.len() == 6 && matches!(subtype, 3 | 4) {
        return Some(mac(rest));
    }
    if subtype == 5 && rest.len() == 5 && rest[0] == 1 {
        // Chassis subtype 5: network address, IPv4.
        return Some(ipv4(&rest[1..5]));
    }
    text(rest)
}

fn lldp_capabilities(bits: u16) -> Vec<String> {
    [
        (0x0004, "Switch"),
        (0x0010, "Router"),
        (0x0008, "Access point"),
        (0x0020, "Telefono"),
        (0x0002, "Ripetitore"),
        (0x0040, "DOCSIS"),
        (0x0080, "Computer"),
        (0x0001, "Altro"),
    ]
    .into_iter()
    .filter(|(bit, _)| bits & bit != 0)
    .map(|(_, name)| name.to_string())
    .collect()
}

fn parse_cdp(data: &[u8]) -> Option<LinkNeighbor> {
    let mut n = LinkNeighbor { protocol: "CDP".into(), ..Default::default() };
    let mut software = None;
    let mut data = data.get(4..)?; // version, TTL, checksum
    while data.len() >= 4 {
        let kind = u16::from_be_bytes([data[0], data[1]]);
        let len = u16::from_be_bytes([data[2], data[3]]) as usize;
        if len < 4 {
            break;
        }
        let Some(value) = data.get(4..len) else { break };
        data = &data[len..];
        match kind {
            0x0001 => n.system_name = text(value),
            0x0002 | 0x0016 => {
                if n.management_ip.is_none() || kind == 0x0016 {
                    if let Some(ip) = cdp_address(value) {
                        n.management_ip = Some(ip);
                    }
                }
            }
            0x0003 => n.port = text(value),
            0x0004 if value.len() >= 4 => {
                let bits = u32::from_be_bytes(value[0..4].try_into().ok()?);
                n.capabilities = [
                    (0x08, "Switch"),
                    (0x01, "Router"),
                    (0x80, "Telefono"),
                    (0x10, "Computer"),
                    (0x40, "Ripetitore"),
                ]
                .into_iter()
                .filter(|(bit, _)| bits & bit != 0)
                .map(|(_, name)| name.to_string())
                .collect();
            }
            0x0005 => software = text(value),
            0x0006 => n.model = text(value),
            0x000a if value.len() >= 2 => n.native_vlan = Some(u16::from_be_bytes([value[0], value[1]])),
            _ => {}
        }
    }
    n.description = match (&n.model, software) {
        (Some(model), Some(sw)) => Some(format!("{model}, {}", sw.lines().next().unwrap_or_default())),
        (model, sw) => sw.or_else(|| model.clone()),
    };
    finish(n)
}

/// The first IPv4 address of a CDP address list.
fn cdp_address(value: &[u8]) -> Option<String> {
    let count = u32::from_be_bytes(value.get(0..4)?.try_into().ok()?);
    let mut at = 4;
    for _ in 0..count {
        let proto_len = *value.get(at + 1)? as usize;
        let proto = value.get(at + 2..at + 2 + proto_len)?;
        at += 2 + proto_len;
        let addr_len = u16::from_be_bytes(value.get(at..at + 2)?.try_into().ok()?) as usize;
        let addr = value.get(at + 2..at + 2 + addr_len)?;
        at += 2 + addr_len;
        if proto == [0xcc] && addr.len() == 4 {
            return Some(ipv4(addr));
        }
    }
    None
}

/// Nothing announced worth showing: no announcement.
fn finish(n: LinkNeighbor) -> Option<LinkNeighbor> {
    (n.system_name.is_some() || n.chassis_id.is_some() || n.port.is_some()).then_some(n)
}

fn text(value: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(value);
    let s = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    (!s.is_empty()).then(|| s.to_string())
}

fn mac(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(":")
}

fn ipv4(b: &[u8]) -> String {
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tlv(kind: u16, value: &[u8]) -> Vec<u8> {
        let mut v = ((kind << 9) | value.len() as u16).to_be_bytes().to_vec();
        v.extend_from_slice(value);
        v
    }

    /// A one-frame pcap (little endian, Ethernet).
    fn pcap(frame: &[u8]) -> Vec<u8> {
        let mut p = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0];
        p.extend_from_slice(&[0; 8]);
        p.extend_from_slice(&1600u32.to_le_bytes());
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&[0; 8]);
        p.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        p.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        p.extend_from_slice(frame);
        p
    }

    /// The announcement of an HPE Instant On 1830, as captured on its port 19.
    fn instant_on() -> Vec<u8> {
        let mut f = vec![0x01, 0x80, 0xc2, 0x00, 0x00, 0x0e, 0x14, 0xab, 0xec, 0x5d, 0x54, 0x93, 0x88, 0xcc];
        f.extend(tlv(1, &[0x04, 0x14, 0xab, 0xec, 0x5d, 0x54, 0x80]));
        f.extend(tlv(2, &[0x05, b'1', b'9']));
        f.extend(tlv(3, &[0x00, 0x78]));
        f.extend(tlv(127, &[0x00, 0x12, 0x0f, 0x05, 0, 0x11, 0, 0x11, 0, 0x11, 0, 0x11, 0, 0x11]));
        f.extend(tlv(4, b"19"));
        f.extend(tlv(5, b"RACK1P-SW48"));
        f.extend(tlv(6, b"HPE Networking Instant On Switch 48p Gigabit 4p SFP 1830 JL814A, InstantOn_1830_3.4.2.0 (1)"));
        f.extend(tlv(7, &[0x00, 0x04, 0x00, 0x04]));
        f.extend(tlv(8, &[0x05, 0x01, 172, 31, 31, 177, 0x02, 0x00, 0x01, 0x86, 0xa0, 0x00]));
        f.extend(tlv(0, &[]));
        f
    }

    #[test]
    fn hpe_instant_on() {
        let n = parse_link_neighbor_pcap(pcap(&instant_on())).unwrap();
        assert_eq!(n.protocol, "LLDP");
        assert_eq!(n.system_name.as_deref(), Some("RACK1P-SW48"));
        assert_eq!(n.port.as_deref(), Some("19"));
        assert_eq!(n.port_description.as_deref(), Some("19"));
        assert_eq!(n.chassis_id.as_deref(), Some("14:ab:ec:5d:54:80"));
        assert_eq!(n.management_ip.as_deref(), Some("172.31.31.177"));
        assert_eq!(n.capabilities, ["Switch"]);
        assert!(n.description.unwrap().contains("1830 JL814A"));
        assert_eq!(n.native_vlan, None);
        assert!(n.vlans.is_empty());
    }

    #[test]
    fn lldp_vlans() {
        let mut f = instant_on();
        f.truncate(f.len() - 2); // the end TLV
        f.extend(tlv(127, &[0x00, 0x80, 0xc2, 0x01, 0x00, 0x01]));
        let mut name = vec![0x00, 0x80, 0xc2, 0x03, 0x00, 69, 4];
        name.extend_from_slice(b"VoIP");
        f.extend(tlv(127, &name));
        f.extend(tlv(0, &[]));
        let n = parse_link_neighbor_pcap(pcap(&f)).unwrap();
        assert_eq!(n.native_vlan, Some(1));
        assert_eq!(n.vlans, [NeighborVlan { tag: 69, name: Some("VoIP".into()) }]);
    }

    #[test]
    fn cisco_cdp() {
        fn cdp_tlv(kind: u16, value: &[u8]) -> Vec<u8> {
            let mut v = kind.to_be_bytes().to_vec();
            v.extend_from_slice(&((value.len() + 4) as u16).to_be_bytes());
            v.extend_from_slice(value);
            v
        }
        let mut body = vec![0x02, 180, 0, 0];
        body.extend(cdp_tlv(1, b"core-sw1.example"));
        let mut addr = vec![0, 0, 0, 1, 0x01, 0x01, 0xcc, 0x00, 0x04];
        addr.extend_from_slice(&[10, 0, 0, 2]);
        body.extend(cdp_tlv(2, &addr));
        body.extend(cdp_tlv(3, b"GigabitEthernet1/0/14"));
        body.extend(cdp_tlv(4, &[0, 0, 0, 0x28]));
        body.extend(cdp_tlv(5, b"Cisco IOS Software, C2960X\nmore"));
        body.extend(cdp_tlv(6, b"cisco WS-C2960X-48FPD-L"));
        body.extend(cdp_tlv(0x0a, &[0, 10]));
        let mut f = vec![0x01, 0x00, 0x0c, 0xcc, 0xcc, 0xcc, 0, 1, 2, 3, 4, 5];
        f.extend_from_slice(&((body.len() + 8) as u16).to_be_bytes());
        f.extend_from_slice(&[0xaa, 0xaa, 0x03, 0x00, 0x00, 0x0c, 0x20, 0x00]);
        f.extend(body);
        let n = parse_link_neighbor_pcap(pcap(&f)).unwrap();
        assert_eq!(n.protocol, "CDP");
        assert_eq!(n.system_name.as_deref(), Some("core-sw1.example"));
        assert_eq!(n.port.as_deref(), Some("GigabitEthernet1/0/14"));
        assert_eq!(n.management_ip.as_deref(), Some("10.0.0.2"));
        assert_eq!(n.capabilities, ["Switch"]);
        assert_eq!(n.model.as_deref(), Some("cisco WS-C2960X-48FPD-L"));
        assert_eq!(n.native_vlan, Some(10));
        assert_eq!(n.description.as_deref(), Some("cisco WS-C2960X-48FPD-L, Cisco IOS Software, C2960X"));
    }

    #[test]
    fn nothing_heard() {
        assert_eq!(parse_link_neighbor_pcap(vec![]), None);
        assert_eq!(parse_link_neighbor_pcap(pcap(&[])), None);
        let header_only = pcap(&[])[..24].to_vec();
        assert_eq!(parse_link_neighbor_pcap(header_only), None);
    }
}
