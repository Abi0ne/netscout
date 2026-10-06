//! Wake-on-LAN: the "magic packet" (6 × 0xFF, then the target MAC 16 times)
//! sent as a UDP broadcast on ports 9 and 7. A sleeping NIC with WoL enabled
//! powers its machine up when it sees its own MAC repeated like this.
//!
//! The packet goes to the directed broadcast of the network the device was
//! last seen on (so it leaves through the right interface) and to the limited
//! broadcast 255.255.255.255. UDP broadcast needs no privileges.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

use ipnet::Ipv4Net;

use crate::error::ScanError;
use crate::types::NetworkInfo;

/// Ports magic packets are conventionally sent to (discard, echo).
const PORTS: [u16; 2] = [9, 7];
/// Each packet is sent this many times: UDP may drop one.
const REPEAT: usize = 3;

/// "aa:bb:cc:dd:ee:ff" or "aa-bb-cc-dd-ee-ff", any case.
pub fn parse_mac(mac: &str) -> Result<[u8; 6], ScanError> {
    let invalid = || ScanError::InvalidConfig(format!("'{mac}' is not a MAC address"));
    let parts: Vec<&str> = mac.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return Err(invalid());
    }
    let mut out = [0u8; 6];
    for (byte, part) in out.iter_mut().zip(parts) {
        if part.is_empty() || part.len() > 2 {
            return Err(invalid());
        }
        *byte = u8::from_str_radix(part, 16).map_err(|_| invalid())?;
    }
    Ok(out)
}

pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0xFF; 102];
    for chunk in packet[6..].as_chunks_mut::<6>().0 {
        chunk.copy_from_slice(&mac);
    }
    packet
}

/// Where to send: the broadcast of the local network containing `ip` (or of
/// every local network when `ip` is unknown or on none of them), then
/// 255.255.255.255.
pub fn broadcasts(ip: Option<Ipv4Addr>, networks: &[NetworkInfo]) -> Vec<Ipv4Addr> {
    let nets: Vec<Ipv4Net> = networks
        .iter()
        .filter_map(|n| n.cidr.parse::<Ipv4Net>().ok())
        .collect();
    let matching: Vec<&Ipv4Net> = nets
        .iter()
        .filter(|n| ip.is_some_and(|ip| n.contains(&ip)))
        .collect();
    let chosen: Vec<&Ipv4Net> = if matching.is_empty() {
        nets.iter().collect()
    } else {
        matching
    };
    let mut out: Vec<Ipv4Addr> = chosen.iter().map(|n| n.broadcast()).collect();
    out.push(Ipv4Addr::BROADCAST);
    out.dedup();
    out
}

/// Send the magic packet for `mac` to every address in `targets`. Succeeds
/// if at least one send went out.
pub fn send(mac: [u8; 6], targets: &[Ipv4Addr]) -> Result<(), ScanError> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| ScanError::Network(format!("wake-on-lan socket: {e}")))?;
    socket
        .set_broadcast(true)
        .map_err(|e| ScanError::Network(format!("wake-on-lan broadcast: {e}")))?;
    let packet = magic_packet(mac);
    let mut sent = false;
    let mut last_error = None;
    for _ in 0..REPEAT {
        for &addr in targets {
            for port in PORTS {
                match socket.send_to(&packet, SocketAddrV4::new(addr, port)) {
                    Ok(_) => sent = true,
                    Err(e) => last_error = Some(format!("{addr}:{port}: {e}")),
                }
            }
        }
    }
    if sent {
        Ok(())
    } else {
        Err(ScanError::Network(format!(
            "wake-on-lan not sent ({})",
            last_error.unwrap_or_else(|| "no destination".into())
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(cidr: &str) -> NetworkInfo {
        NetworkInfo {
            interface: "en0".into(),
            ipv4: "0.0.0.0".into(),
            cidr: cidr.into(),
            gateway: None,
            dns: vec![],
        }
    }

    #[test]
    fn parses_macs() {
        assert_eq!(
            parse_mac("AA:bb:0c:dd:ee:0F").unwrap(),
            [0xaa, 0xbb, 0x0c, 0xdd, 0xee, 0x0f]
        );
        assert_eq!(
            parse_mac("aa-bb-cc-dd-ee-ff").unwrap(),
            [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]
        );
        assert!(parse_mac("aa:bb:cc:dd:ee").is_err());
        assert!(parse_mac("aa:bb:cc:dd:ee:gg").is_err());
        assert!(parse_mac("aaa:bb:cc:dd:ee:ff").is_err());
    }

    #[test]
    fn magic_packet_layout() {
        let mac = [1, 2, 3, 4, 5, 6];
        let p = magic_packet(mac);
        assert_eq!(&p[..6], &[0xFF; 6]);
        assert!(p[6..].chunks(6).all(|c| c == mac));
        assert_eq!(p[6..].chunks(6).count(), 16);
    }

    #[test]
    fn broadcast_of_the_matching_network() {
        let nets = [net("192.168.1.0/24"), net("10.0.0.0/16")];
        assert_eq!(
            broadcasts(Some("10.0.3.4".parse().unwrap()), &nets),
            vec![Ipv4Addr::new(10, 0, 255, 255), Ipv4Addr::BROADCAST]
        );
        assert_eq!(
            broadcasts(Some("172.16.0.1".parse().unwrap()), &nets),
            vec![
                Ipv4Addr::new(192, 168, 1, 255),
                Ipv4Addr::new(10, 0, 255, 255),
                Ipv4Addr::BROADCAST
            ]
        );
        assert_eq!(broadcasts(None, &[]), vec![Ipv4Addr::BROADCAST]);
    }
}
