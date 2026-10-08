//! Names that cross subnets, for the deep profile only.
//!
//! mDNS and NetBIOS (see `names`) are link-local in practice: devices and
//! firewalls drop them from another VLAN. These sources ride ordinary
//! unicast traffic, so they work wherever the port sweep does:
//!
//! * **DNS, asked directly** — a PTR query to name servers other than the
//!   system's: the ones the user configured and the scanned hosts that
//!   answer on port 53 (domain controllers, routers and firewalls that
//!   register their DHCP clients).
//! * **SMB** (445) — the NTLM challenge a Windows or Samba server sends
//!   before any credential, which carries its computer and domain names.
//! * **TLS certificates** — Remote Desktop (3389) presents a certificate
//!   named after the machine; web interfaces (443, 8443, 5001) often carry
//!   the device's DNS name. Only TLS 1.2 is offered, so the certificate
//!   travels in the clear and no TLS library is needed; nothing is verified
//!   and the handshake is dropped once the certificate is read.
//!
//! Every connection is bounded by [`DEADLINE`] and the socket budget.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{timeout, Instant};

use crate::names;

/// Deadline of every query or connection here.
const DEADLINE: Duration = Duration::from_secs(3);
/// Name servers asked at most (configured ones first).
const MAX_SERVERS: usize = 6;

const SMB_PORT: u16 = 445;
const RDP_PORT: u16 = 3389;
/// Web ports whose certificate may name the device.
const HTTPS_PORTS: &[u16] = &[443, 8443, 5001];

/// One host to name: its address and open ports.
pub struct Target {
    pub ip: Ipv4Addr,
    pub open_ports: Vec<u16>,
}

/// Names found per host, best first (SMB, DNS, Remote Desktop, web).
pub async fn resolve(
    targets: &[Target],
    configured_servers: &[Ipv4Addr],
    sockets: Arc<Semaphore>,
) -> HashMap<Ipv4Addr, Vec<String>> {
    // Hosts serving DNS on the scanned networks are likely to know their
    // neighbours' names.
    let mut servers: Vec<Ipv4Addr> = configured_servers.to_vec();
    for t in targets {
        if t.open_ports.contains(&53) && !servers.contains(&t.ip) {
            servers.push(t.ip);
        }
    }
    servers.truncate(MAX_SERVERS);

    let ips: Vec<Ipv4Addr> = targets.iter().map(|t| t.ip).collect();
    let dns = async { dns_names(&ips, &servers, DEADLINE).await };

    let mut set = JoinSet::new();
    for t in targets {
        for &port in &t.open_ports {
            let kind = match port {
                SMB_PORT => Probe::Smb,
                RDP_PORT => Probe::Rdp,
                p if HTTPS_PORTS.contains(&p) => Probe::Https,
                _ => continue,
            };
            let (ip, sockets) = (t.ip, Arc::clone(&sockets));
            set.spawn(async move {
                let Ok(_permit) = sockets.acquire_owned().await else {
                    return (ip, kind, None);
                };
                let name = timeout(DEADLINE, probe(ip, port, kind))
                    .await
                    .ok()
                    .flatten();
                (ip, kind, name)
            });
        }
    }
    let connections = async {
        let mut found: Vec<(Ipv4Addr, Probe, String)> = Vec::new();
        while let Some(done) = set.join_next().await {
            if let Ok((ip, kind, Some(name))) = done {
                found.push((ip, kind, name));
            }
        }
        found
    };
    let (dns, mut found) = tokio::join!(dns, connections);

    found.sort_by_key(|(_, kind, _)| *kind as u8);
    let mut out: HashMap<Ipv4Addr, Vec<String>> = HashMap::new();
    let mut add = |ip: Ipv4Addr, name: String| {
        let names = out.entry(ip).or_default();
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    };
    // SMB first (the machine's own name), then DNS, then certificates.
    for (ip, kind, name) in &found {
        if *kind == Probe::Smb {
            add(*ip, name.clone());
        }
    }
    for (ip, name) in dns {
        add(ip, name);
    }
    for (ip, kind, name) in found {
        if kind != Probe::Smb {
            add(ip, name);
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Probe {
    Smb = 0,
    Rdp = 1,
    Https = 2,
}

async fn probe(ip: Ipv4Addr, port: u16, kind: Probe) -> Option<String> {
    let mut stream = TcpStream::connect((ip, port)).await.ok()?;
    match kind {
        Probe::Smb => smb_name(&mut stream).await,
        Probe::Rdp => {
            rdp_start_tls(&mut stream).await?;
            let names = tls_certificate_names(&mut stream).await?;
            names.into_iter().find_map(|n| host_name(&n, false))
        }
        Probe::Https => {
            let names = tls_certificate_names(&mut stream).await?;
            // A web certificate named without a domain is usually the
            // vendor's ("UniFi", "Fortinet"), not the device's.
            names.into_iter().find_map(|n| host_name(&n, true))
        }
    }
}

/// A usable host name from a certificate or protocol field: not an address,
/// not a wildcard or "localhost", no spaces; with a dot if `need_domain`.
fn host_name(name: &str, need_domain: bool) -> Option<String> {
    let name = name.trim().trim_end_matches('.');
    let valid = !name.is_empty()
        && name.parse::<Ipv4Addr>().is_err()
        && !name.contains('*')
        && !name.contains(char::is_whitespace)
        && !name.eq_ignore_ascii_case("localhost")
        && !name.to_ascii_lowercase().starts_with("localhost.")
        && name.chars().any(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._".contains(c))
        && (!need_domain || name.contains('.'));
    valid.then(|| name.to_string())
}

// ---------------------------------------------------------------------------
// DNS, asked directly
// ---------------------------------------------------------------------------

/// PTR names of `hosts` from `servers`: every server gets every query from
/// one socket; the first answer per host wins.
async fn dns_names(
    hosts: &[Ipv4Addr],
    servers: &[Ipv4Addr],
    wait: Duration,
) -> HashMap<Ipv4Addr, String> {
    let mut found = HashMap::new();
    if hosts.is_empty() || servers.is_empty() {
        return found;
    }
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else {
        return found;
    };
    // Transaction id → host; ids are unique across servers.
    let mut ids: HashMap<(Ipv4Addr, u16), Ipv4Addr> = HashMap::new();
    let mut id: u16 = 0x2b00;
    for &server in servers {
        for &host in hosts {
            id = id.wrapping_add(1);
            if socket
                .send_to(&names::dns_ptr_query(host, id, true), (server, 53))
                .await
                .is_ok()
            {
                ids.insert((server, id), host);
            }
        }
    }
    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 1500];
    while found.len() < hosts.len() {
        let Ok(Ok((n, from))) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await
        else {
            break;
        };
        let SocketAddr::V4(from) = from else { continue };
        let reply = &buf[..n];
        if reply.len() < 2 {
            continue;
        }
        let id = u16::from_be_bytes([reply[0], reply[1]]);
        let Some(&host) = ids.get(&(*from.ip(), id)) else {
            continue;
        };
        if found.contains_key(&host) {
            continue;
        }
        if let Some(name) = names::parse_ptr_answer(reply).and_then(|n| host_name(&n, false)) {
            found.insert(host, name);
        }
    }
    found
}

// ---------------------------------------------------------------------------
// SMB: the NTLM challenge
// ---------------------------------------------------------------------------

/// The server's DNS computer name (else its NetBIOS name) from the NTLM
/// challenge of an anonymous SMB2 session setup.
async fn smb_name(stream: &mut TcpStream) -> Option<String> {
    send_netbios(stream, &smb2_negotiate()).await?;
    read_netbios(stream).await?;
    send_netbios(stream, &smb2_session_setup()).await?;
    let reply = read_netbios(stream).await?;
    ntlm_challenge_name(&reply)
}

async fn send_netbios(stream: &mut TcpStream, msg: &[u8]) -> Option<()> {
    let len = msg.len() as u32;
    let mut frame = vec![0, (len >> 16) as u8, (len >> 8) as u8, len as u8];
    frame.extend(msg);
    stream.write_all(&frame).await.ok()
}

async fn read_netbios(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await.ok()?;
    let len = u32::from_be_bytes([0, head[1], head[2], head[3]]) as usize;
    if len > 1 << 16 {
        return None;
    }
    let mut msg = vec![0u8; len];
    stream.read_exact(&mut msg).await.ok()?;
    Some(msg)
}

fn smb2_header(command: u16, message_id: u64) -> Vec<u8> {
    let mut h = Vec::with_capacity(64);
    h.extend(b"\xfeSMB");
    h.extend(64u16.to_le_bytes()); // structure size
    h.extend(0u16.to_le_bytes()); // credit charge
    h.extend(0u32.to_le_bytes()); // status
    h.extend(command.to_le_bytes());
    h.extend(1u16.to_le_bytes()); // credits requested
    h.extend(0u32.to_le_bytes()); // flags
    h.extend(0u32.to_le_bytes()); // next command
    h.extend(message_id.to_le_bytes());
    h.extend(0u32.to_le_bytes()); // process id
    h.extend(0u32.to_le_bytes()); // tree id
    h.extend(0u64.to_le_bytes()); // session id
    h.extend([0u8; 16]); // signature
    h
}

/// SMB2 NEGOTIATE offering dialects 2.0.2 and 2.1 (no negotiate contexts).
fn smb2_negotiate() -> Vec<u8> {
    let mut m = smb2_header(0, 0);
    m.extend(36u16.to_le_bytes()); // structure size
    m.extend(2u16.to_le_bytes()); // dialect count
    m.extend(1u16.to_le_bytes()); // security mode: signing enabled
    m.extend(0u16.to_le_bytes());
    m.extend(0u32.to_le_bytes()); // capabilities
    m.extend(*b"NetScout-namesrq"); // client GUID
    m.extend(0u64.to_le_bytes()); // client start time
    m.extend(0x0202u16.to_le_bytes());
    m.extend(0x0210u16.to_le_bytes());
    m
}

/// SMB2 SESSION_SETUP carrying an NTLM NEGOTIATE wrapped in SPNEGO.
fn smb2_session_setup() -> Vec<u8> {
    let token = spnego_init(&ntlm_negotiate());
    let mut m = smb2_header(1, 1);
    m.extend(25u16.to_le_bytes()); // structure size
    m.push(0); // flags
    m.push(1); // security mode
    m.extend(0u32.to_le_bytes()); // capabilities
    m.extend(0u32.to_le_bytes()); // channel
    m.extend((64u16 + 24).to_le_bytes()); // security buffer offset
    m.extend((token.len() as u16).to_le_bytes());
    m.extend(0u64.to_le_bytes()); // previous session id
    m.extend(token);
    m
}

/// NTLM NEGOTIATE_MESSAGE: Unicode, NTLM, extended session security,
/// always sign, 128/56-bit, key exchange; asks for the target info.
fn ntlm_negotiate() -> Vec<u8> {
    let mut m = Vec::with_capacity(32);
    m.extend(b"NTLMSSP\0");
    m.extend(1u32.to_le_bytes());
    m.extend(0xe008_8205u32.to_le_bytes());
    m.extend([0u8; 16]); // domain and workstation: empty
    m
}

/// DER element `tag` with `content` (definite length).
fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else if len < 0x100 {
        out.extend([0x81, len as u8]);
    } else {
        out.extend([0x82, (len >> 8) as u8, len as u8]);
    }
    out.extend(content);
    out
}

/// GSS-API SPNEGO negTokenInit offering NTLMSSP with `mech_token`.
fn spnego_init(mech_token: &[u8]) -> Vec<u8> {
    const SPNEGO: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x02];
    const NTLMSSP: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x02, 0x0a];
    let mech_types = der(0xa0, &der(0x30, &der(0x06, NTLMSSP)));
    let token = der(0xa2, &der(0x04, mech_token));
    let init = der(0xa0, &der(0x30, &[mech_types, token].concat()));
    der(0x60, &[der(0x06, SPNEGO), init].concat())
}

/// The DNS computer name, else the NetBIOS one, from the target info of an
/// NTLM CHALLENGE_MESSAGE found anywhere in `msg`.
fn ntlm_challenge_name(msg: &[u8]) -> Option<String> {
    let start = msg.windows(12).position(|w| w == b"NTLMSSP\0\x02\0\0\0")?;
    let ntlm = &msg[start..];
    let info_len = u16::from_le_bytes([*ntlm.get(40)?, *ntlm.get(41)?]) as usize;
    let info_off = u32::from_le_bytes(ntlm.get(44..48)?.try_into().ok()?) as usize;
    let info = ntlm.get(info_off..info_off + info_len)?;
    let (mut netbios, mut dns) = (None, None);
    let mut off = 0;
    while let Some(head) = info.get(off..off + 4) {
        let id = u16::from_le_bytes([head[0], head[1]]);
        let len = u16::from_le_bytes([head[2], head[3]]) as usize;
        let value = info.get(off + 4..off + 4 + len)?;
        let text = || {
            let units: Vec<u16> = value
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        };
        match id {
            0 => break,
            1 => netbios = Some(text()),
            3 => dns = Some(text()),
            _ => {}
        }
        off += 4 + len;
    }
    dns.and_then(|n| host_name(&n, false))
        .or_else(|| netbios.and_then(|n| host_name(&n, false)))
}

// ---------------------------------------------------------------------------
// Remote Desktop and TLS certificates
// ---------------------------------------------------------------------------

/// X.224 connection request asking for TLS (or CredSSP over TLS); `Some` if
/// the server agreed, after which TLS starts on the same stream.
async fn rdp_start_tls(stream: &mut TcpStream) -> Option<()> {
    const REQUEST: &[u8] = &[
        0x03, 0x00, 0x00, 0x13, // TPKT, length 19
        0x0e, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, // X.224 CR
        0x01, 0x00, 0x08, 0x00, 0x03, 0x00, 0x00, 0x00, // RDP_NEG_REQ: SSL | HYBRID
    ];
    stream.write_all(REQUEST).await.ok()?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await.ok()?;
    let len = u16::from_be_bytes([head[2], head[3]]) as usize;
    if !(4..=512).contains(&len) {
        return None;
    }
    let mut rest = vec![0u8; len - 4];
    stream.read_exact(&mut rest).await.ok()?;
    // X.224 CC (7 bytes), then RDP_NEG_RSP (type 2) or RDP_NEG_FAILURE (3).
    let neg = rest.get(7..)?;
    (neg.first() == Some(&0x02)).then_some(())
}

/// TLS 1.2 ClientHello with common ECDHE/RSA suites and no SNI.
fn client_hello() -> Vec<u8> {
    const SUITES: &[u16] = &[
        0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8, 0xc009, 0xc013, 0xc00a, 0xc014, 0x009c,
        0x009d, 0x002f, 0x0035, 0x000a,
    ];
    let mut body = Vec::new();
    body.extend([0x03, 0x03]); // TLS 1.2
    body.extend(*b"NetScout reads certificate names"); // random (32 bytes)
    body.push(0); // no session id
    body.extend(((SUITES.len() * 2) as u16).to_be_bytes());
    for s in SUITES {
        body.extend(s.to_be_bytes());
    }
    body.extend([1, 0]); // compression: null
    let mut ext = Vec::new();
    // supported_groups: x25519, secp256r1, secp384r1
    ext.extend([
        0x00, 0x0a, 0x00, 0x08, 0x00, 0x06, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x18,
    ]);
    // ec_point_formats: uncompressed
    ext.extend([0x00, 0x0b, 0x00, 0x02, 0x01, 0x00]);
    // signature_algorithms
    let algs: &[u16] = &[
        0x0403, 0x0503, 0x0603, 0x0804, 0x0805, 0x0806, 0x0401, 0x0501, 0x0601, 0x0201, 0x0203,
    ];
    ext.extend([0x00, 0x0d]);
    ext.extend(((algs.len() * 2 + 2) as u16).to_be_bytes());
    ext.extend(((algs.len() * 2) as u16).to_be_bytes());
    for a in algs {
        ext.extend(a.to_be_bytes());
    }
    // renegotiation_info (empty)
    ext.extend([0xff, 0x01, 0x00, 0x01, 0x00]);
    body.extend((ext.len() as u16).to_be_bytes());
    body.extend(ext);

    let mut hs = vec![1]; // client_hello
    hs.extend(&(body.len() as u32).to_be_bytes()[1..]);
    hs.extend(body);
    let mut record = vec![0x16, 0x03, 0x01];
    record.extend((hs.len() as u16).to_be_bytes());
    record.extend(hs);
    record
}

/// The names in the server's certificate: subject alternative DNS names,
/// then the subject common name.
async fn tls_certificate_names(stream: &mut TcpStream) -> Option<Vec<String>> {
    stream.write_all(&client_hello()).await.ok()?;
    // Handshake messages, reassembled across records.
    let mut handshake: Vec<u8> = Vec::new();
    loop {
        let mut head = [0u8; 5];
        stream.read_exact(&mut head).await.ok()?;
        let len = u16::from_be_bytes([head[3], head[4]]) as usize;
        if len > 1 << 15 {
            return None;
        }
        let mut record = vec![0u8; len];
        stream.read_exact(&mut record).await.ok()?;
        match head[0] {
            0x16 => handshake.extend(record),
            _ => return None, // alert or anything else: no certificate
        }
        // Walk the complete messages: ServerHello (2), Certificate (11).
        let mut off = 0;
        while let Some(h) = handshake.get(off..off + 4) {
            let (kind, mlen) = (h[0], u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize);
            let Some(msg) = handshake.get(off + 4..off + 4 + mlen) else {
                break;
            };
            match kind {
                11 => return certificate_names(msg),
                14 => return None, // server hello done without a certificate
                _ => off += 4 + mlen,
            }
        }
        if handshake.len() > 1 << 17 {
            return None;
        }
    }
}

/// Names from the first certificate of a TLS Certificate message.
fn certificate_names(msg: &[u8]) -> Option<Vec<String>> {
    // u24 list length, u24 first certificate length, the DER certificate.
    let len = u32::from_be_bytes([0, *msg.get(3)?, *msg.get(4)?, *msg.get(5)?]) as usize;
    let cert = msg.get(6..6 + len)?;
    x509_names(cert)
}

/// One DER element at the start of `data`: (tag, content, rest).
fn der_next(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *data.first()?;
    let first = *data.get(1)?;
    let (len, head) = if first < 0x80 {
        (first as usize, 2)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 3 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = len << 8 | *data.get(2 + i)? as usize;
        }
        (len, 2 + n)
    };
    let content = data.get(head..head + len)?;
    Some((tag, content, &data[head + len..]))
}

/// Subject alternative DNS names, then the subject CN, of a certificate.
fn x509_names(cert: &[u8]) -> Option<Vec<String>> {
    let (_, cert, _) = der_next(cert)?; // Certificate
    let (_, tbs, _) = der_next(cert)?; // TBSCertificate
    let mut rest = tbs;
    let mut fields: Vec<(u8, &[u8])> = Vec::new();
    while let Some((tag, content, next)) = der_next(rest) {
        fields.push((tag, content));
        rest = next;
    }
    // [0] version is optional; then serial, signature, issuer, validity,
    // subject, key; extensions in [3].
    let base = usize::from(fields.first()?.0 == 0xa0);
    let subject = fields.get(base + 4)?.1;
    let mut names = Vec::new();
    if let Some(&(_, ext)) = fields.iter().find(|(tag, _)| *tag == 0xa3) {
        names.extend(san_dns_names(ext));
    }
    if let Some(cn) = subject_cn(subject) {
        names.push(cn);
    }
    Some(names)
}

/// dNSName entries of the subjectAltName extension in `[3] Extensions`.
fn san_dns_names(ext: &[u8]) -> Vec<String> {
    const SAN: &[u8] = &[0x55, 0x1d, 0x11];
    let mut out = Vec::new();
    let Some((_, mut list, _)) = der_next(ext) else {
        return out;
    };
    while let Some((_, extension, next)) = der_next(list) {
        list = next;
        let Some((0x06, oid, mut rest)) = der_next(extension) else {
            continue;
        };
        if oid != SAN {
            continue;
        }
        // Optional critical BOOLEAN, then the OCTET STRING.
        if let Some((0x01, _, after)) = der_next(rest) {
            rest = after;
        }
        let Some((0x04, value, _)) = der_next(rest) else {
            continue;
        };
        let Some((_, mut general, _)) = der_next(value) else {
            continue;
        };
        while let Some((tag, name, next)) = der_next(general) {
            general = next;
            if tag == 0x82 {
                out.push(String::from_utf8_lossy(name).into_owned());
            }
        }
    }
    out
}

/// The commonName of an X.501 Name.
fn subject_cn(name: &[u8]) -> Option<String> {
    const CN: &[u8] = &[0x55, 0x04, 0x03];
    let mut sets = name;
    while let Some((_, set, next)) = der_next(sets) {
        sets = next;
        let mut attrs = set;
        while let Some((_, attr, next)) = der_next(attrs) {
            attrs = next;
            if let Some((0x06, oid, rest)) = der_next(attr) {
                if oid == CN {
                    let (_, value, _) = der_next(rest)?;
                    return Some(String::from_utf8_lossy(value).into_owned());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_are_filtered() {
        assert_eq!(
            host_name("srv-w11.corp.local.", false).as_deref(),
            Some("srv-w11.corp.local")
        );
        assert_eq!(host_name("SRV-W11", false).as_deref(), Some("SRV-W11"));
        assert_eq!(host_name("SRV-W11", true), None);
        assert_eq!(host_name("*.corp.local", false), None);
        assert_eq!(host_name("localhost", false), None);
        assert_eq!(host_name("10.0.0.5", false), None);
        assert_eq!(host_name("Sophos Firewall", false), None);
    }

    #[test]
    fn spnego_wraps_ntlm() {
        let token = spnego_init(&ntlm_negotiate());
        assert_eq!(token[0], 0x60);
        assert_eq!(token[1] as usize, token.len() - 2);
        assert!(token.windows(8).any(|w| w == b"NTLMSSP\0"));
    }

    /// An NTLM challenge whose target info names "SRV01" / "srv01.corp.local".
    fn challenge(with_dns: bool) -> Vec<u8> {
        let utf16 = |s: &str| {
            s.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        let mut info = Vec::new();
        let mut pair = |id: u16, value: Vec<u8>| {
            info.extend(id.to_le_bytes());
            info.extend((value.len() as u16).to_le_bytes());
            info.extend(value);
        };
        pair(2, utf16("CORP"));
        pair(1, utf16("SRV01"));
        if with_dns {
            pair(3, utf16("srv01.corp.local"));
        }
        pair(0, Vec::new());
        let mut m = b"junk-before".to_vec(); // SPNEGO wrapping, SMB header…
        let start = m.len();
        m.extend(b"NTLMSSP\0");
        m.extend(2u32.to_le_bytes());
        m.resize(start + 48, 0);
        m[start + 40..start + 42].copy_from_slice(&(info.len() as u16).to_le_bytes());
        m[start + 44..start + 48].copy_from_slice(&48u32.to_le_bytes());
        m.extend(info);
        m
    }

    #[test]
    fn ntlm_challenge_gives_the_computer_name() {
        assert_eq!(
            ntlm_challenge_name(&challenge(true)).as_deref(),
            Some("srv01.corp.local")
        );
        assert_eq!(
            ntlm_challenge_name(&challenge(false)).as_deref(),
            Some("SRV01")
        );
        assert_eq!(ntlm_challenge_name(b"no challenge here"), None);
    }

    /// A minimal certificate: version, serial, algorithm, issuer, validity,
    /// subject CN, key, and a subjectAltName with two DNS names.
    fn certificate() -> Vec<u8> {
        let cn = |s: &str| {
            der(
                0x30,
                &der(
                    0x31,
                    &der(
                        0x30,
                        &[der(0x06, &[0x55, 0x04, 0x03]), der(0x0c, s.as_bytes())].concat(),
                    ),
                ),
            )
        };
        let general = [der(0x82, b"rdp.corp.local"), der(0x82, b"rdp")].concat();
        let san = der(
            0x30,
            &[
                der(0x06, &[0x55, 0x1d, 0x11]),
                der(0x04, &der(0x30, &general)),
            ]
            .concat(),
        );
        let tbs = der(
            0x30,
            &[
                der(0xa0, &der(0x02, &[2])),
                der(0x02, &[1]),
                der(0x30, &[]),
                cn("Issuer CA"),
                der(0x30, &[]),
                cn("RDP-HOST"),
                der(0x30, &[]),
                der(0xa3, &der(0x30, &san)),
            ]
            .concat(),
        );
        der(0x30, &[tbs, der(0x30, &[]), der(0x03, &[0])].concat())
    }

    #[test]
    fn certificate_names_are_read() {
        assert_eq!(
            x509_names(&certificate()).unwrap(),
            vec![
                "rdp.corp.local".to_string(),
                "rdp".into(),
                "RDP-HOST".into()
            ]
        );
    }

    #[test]
    fn client_hello_is_one_tls_record() {
        let hello = client_hello();
        assert_eq!(&hello[..3], &[0x16, 0x03, 0x01]);
        assert_eq!(
            u16::from_be_bytes([hello[3], hello[4]]) as usize,
            hello.len() - 5
        );
        assert_eq!(hello[5], 1);
    }
}
