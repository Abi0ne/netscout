//! CSV import of saved profiles: the reverse of [`crate::export`].
//!
//! Rows are grouped back into profiles by name, scanned network and save
//! date; each profile gets its devices (up and off), notes and tagged VLANs
//! again. Columns are found by their header, so a file reordered in a
//! spreadsheet, or exported before a column existed, still reads. What the
//! CSV does not carry (port banners, mDNS TXT records, UPnP identifiers) is
//! left empty.
//!
//! The separator is read from the header line (`;` as exported, `,` or tab
//! if a spreadsheet saved it again), and dates as exported ("2026-10-07
//! 14:30") or in the Italian form a spreadsheet may turn them into
//! ("07/10/2026 14:30").

use std::collections::HashMap;

use crate::error::ScanError;
use crate::export::{ip_key, scan_label, type_label, OFF_SUBNET};
use crate::profiles::{device_key, ProfileVlan, SavedProfile};
use crate::types::{
    DeviceType, Host, Port, PortState, ScanProfile, ServiceInfo, SsdpInfo, Transport,
};

/// The profiles in `text`, a CSV exported by NetScout, without ids (the
/// store assigns them). Fails when the file is not such a CSV.
pub(crate) fn parse_profiles_csv(text: &str) -> Result<Vec<SavedProfile>, ScanError> {
    let text = text.trim_start_matches('\u{feff}');
    let first_line = text.lines().next().unwrap_or_default();
    let separator = [';', ',', '\t']
        .into_iter()
        .max_by_key(|c| first_line.matches(*c).count())
        .unwrap_or(';');
    let mut rows = records(text, separator).into_iter();
    let header: HashMap<String, usize> = rows
        .next()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, name)| (name.trim().to_lowercase(), i))
        .collect();
    if !["profilo", "ip"].iter().all(|c| header.contains_key(*c)) {
        return Err(ScanError::InvalidConfig(
            "not a CSV exported by NetScout (no \"Profilo\" and \"IP\" columns)".into(),
        ));
    }
    let column = |row: &[String], name: &str| -> String {
        header
            .get(&name.to_lowercase())
            .and_then(|&i| row.get(i))
            .map(|f| unescape(f.trim()))
            .unwrap_or_default()
    };

    let mut profiles: Vec<SavedProfile> = Vec::new();
    let mut index: HashMap<(String, String, String), usize> = HashMap::new();
    for row in rows {
        if row.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        let name = column(&row, "Profilo");
        if name.is_empty() {
            continue;
        }
        let target = column(&row, "Rete scansionata");
        let saved = column(&row, "Salvato il");
        let key = (name.clone(), target.clone(), saved.clone());
        let i = *index.entry(key).or_insert_with(|| {
            let created_at = parse_time(&saved).unwrap_or_else(crate::profiles::now_ms);
            profiles.push(SavedProfile {
                id: String::new(),
                name,
                created_at,
                target,
                scan_profile: parse_scan(&column(&row, "Tipo di scansione")),
                hosts: Vec::new(),
                offline_hosts: Vec::new(),
                updated_at: parse_time(&column(&row, "Aggiornato il")),
                notes: HashMap::new(),
                vlans: parse_vlans(&column(&row, "VLAN del profilo")),
            });
            profiles.len() - 1
        });
        let profile = &mut profiles[i];

        let ip = column(&row, "IP");
        if ip.is_empty() {
            continue; // a profile without devices
        }
        let off = column(&row, "Stato").eq_ignore_ascii_case("Spento");
        let seen = profile.updated_at.unwrap_or(profile.created_at);
        let names: Vec<String> = std::iter::once(column(&row, "Nome"))
            .chain(list(&column(&row, "Altri nomi")))
            .filter(|n| !n.is_empty())
            .collect();
        let host = Host {
            ip: ip.clone(),
            mac: some(column(&row, "MAC")),
            vendor: some(column(&row, "Produttore")),
            hostnames: names,
            device_type: parse_type(&column(&row, "Tipo")),
            open_ports: list(&column(&row, "Porte aperte"))
                .iter()
                .filter_map(|p| parse_port(p))
                .collect(),
            rtt_ms: if off {
                None
            } else {
                column(&row, "Latenza (ms)").replace(',', ".").parse().ok()
            },
            mdns_services: list(&column(&row, "Servizi mDNS"))
                .iter()
                .map(|s| parse_service(s, &ip))
                .collect(),
            ssdp_info: some(column(&row, "Dispositivo UPnP")).map(|name| SsdpInfo {
                usn: String::new(),
                location: String::new(),
                server: None,
                friendly_name: Some(name),
            }),
            first_seen: seen,
            last_seen: seen,
            off_subnet: column(&row, "VLAN").eq_ignore_ascii_case(OFF_SUBNET),
            model: some(column(&row, "Modello")),
        };
        let note = column(&row, "Note");
        if !note.is_empty() {
            profile.notes.insert(device_key(&host), note);
        }
        if off {
            profile.offline_hosts.push(host);
        } else {
            profile.hosts.push(host);
        }
    }
    for p in &mut profiles {
        p.hosts.sort_by_key(|h| ip_key(&h.ip));
        p.offline_hosts.sort_by_key(|h| ip_key(&h.ip));
    }
    Ok(profiles)
}

/// The fields of each line, per RFC 4180 (quotes, doubled quotes, line
/// breaks inside quotes).
fn records(text: &str, separator: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            quoted = true;
        } else if c == separator {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' {
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else if c != '\r' {
            field.push(c);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Undo the export's `'` before text that reads like a formula.
fn unescape(field: &str) -> String {
    match field.strip_prefix('\'') {
        Some(rest) if rest.starts_with(['=', '+', '-', '@', '\t', '\r']) => rest.to_string(),
        _ => field.to_string(),
    }
}

fn some(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// "a, b, c" as its items.
fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// "69 172.20.20.0/24, 70 10.0.70.0/24", sorted by tag, one each.
fn parse_vlans(s: &str) -> Vec<ProfileVlan> {
    let mut vlans: Vec<ProfileVlan> = list(s)
        .iter()
        .filter_map(|v| {
            let mut parts = v.split_whitespace();
            let tag: u16 = parts.next()?.parse().ok()?;
            let network = parts.next()?.to_string();
            ((1..=4094).contains(&tag) && network.contains('/'))
                .then_some(ProfileVlan { tag, network })
        })
        .collect();
    vlans.sort_by_key(|v| v.tag);
    vlans.dedup_by_key(|v| v.tag);
    vlans
}

/// "445 smb", "53/udp dns" or "8080".
fn parse_port(s: &str) -> Option<Port> {
    let (number, service) = match s.split_once(' ') {
        Some((n, svc)) => (n, some(svc.trim().to_string())),
        None => (s, None),
    };
    let (number, transport) = match number.strip_suffix("/udp") {
        Some(n) => (n, Transport::Udp),
        None => (number.strip_suffix("/tcp").unwrap_or(number), Transport::Tcp),
    };
    Some(Port {
        number: number.parse().ok()?,
        transport,
        state: PortState::Open,
        service,
        version: None,
    })
}

/// "Stampante ufficio (_ipp._tcp)".
fn parse_service(s: &str, ip: &str) -> ServiceInfo {
    let (name, service_type) = match s.rsplit_once(" (") {
        Some((name, ty)) => (name, ty.trim_end_matches(')')),
        None => (s, ""),
    };
    ServiceInfo {
        service_type: service_type.into(),
        name: name.into(),
        host: ip.into(),
        port: 0,
        domain: None,
        txt: Vec::new(),
    }
}

fn parse_scan(s: &str) -> ScanProfile {
    [ScanProfile::Quick, ScanProfile::Standard, ScanProfile::Deep]
        .into_iter()
        .find(|p| scan_label(*p).eq_ignore_ascii_case(s) || format!("{p:?}").eq_ignore_ascii_case(s))
        .unwrap_or(ScanProfile::Standard)
}

fn parse_type(s: &str) -> DeviceType {
    use DeviceType::*;
    [
        Router, Modem, Computer, Laptop, Server, Phone, Tablet, Printer, Camera, Nvr, Nasc,
        SmartTv, Gaming, Voip, Audio, Iot, Other,
    ]
    .into_iter()
    .find(|t| type_label(*t).eq_ignore_ascii_case(s))
    .unwrap_or(Unknown)
}

/// Local date and time as epoch ms: "2026-10-07 14:30" or "07/10/2026
/// 14:30", seconds optional, time optional.
fn parse_time(s: &str) -> Option<i64> {
    let (date, time) = s.trim().split_once([' ', 'T']).unwrap_or((s.trim(), ""));
    if date.is_empty() {
        return None;
    }
    let numbers = |s: &str, sep: char| -> Option<Vec<i32>> {
        s.split(sep).map(|n| n.trim().parse().ok()).collect()
    };
    let (year, month, day) = match (numbers(date, '-'), numbers(date, '/'), numbers(date, '.')) {
        (Some(d), _, _) if d.len() == 3 => (d[0], d[1], d[2]),
        (_, Some(d), _) | (_, _, Some(d)) if d.len() == 3 => {
            (if d[2] < 100 { d[2] + 2000 } else { d[2] }, d[1], d[0])
        }
        _ => return None,
    };
    let time = if time.trim().is_empty() {
        vec![0, 0]
    } else {
        numbers(time.trim(), ':')?
    };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month - 1;
    tm.tm_mday = day;
    tm.tm_hour = *time.first()?;
    tm.tm_min = *time.get(1).unwrap_or(&0);
    tm.tm_sec = *time.get(2).unwrap_or(&0);
    tm.tm_isdst = -1;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // SAFETY: mktime only reads and normalizes the `tm` we hand it.
    let secs = unsafe { libc::mktime(&mut tm) };
    (secs != -1).then_some(secs as i64 * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::profiles_csv;

    fn host(ip: &str, mac: Option<&str>) -> Host {
        Host {
            ip: ip.into(),
            mac: mac.map(Into::into),
            vendor: None,
            hostnames: vec![],
            device_type: DeviceType::Unknown,
            open_ports: vec![],
            rtt_ms: None,
            mdns_services: vec![],
            ssdp_info: None,
            first_seen: 0,
            last_seen: 0,
            off_subnet: false,
            model: None,
        }
    }

    #[test]
    fn round_trip() {
        let mut nas = host("10.0.0.2", Some("aa:00:00:00:00:02"));
        nas.vendor = Some("Società; S.p.A.".into());
        nas.hostnames = vec!["nas".into(), "nas.lan".into()];
        nas.device_type = DeviceType::Nasc;
        nas.rtt_ms = Some(1.25);
        nas.model = Some("DS920+".into());
        nas.open_ports = vec![
            Port { number: 445, transport: Transport::Tcp, state: PortState::Open, service: Some("smb".into()), version: None },
            Port { number: 53, transport: Transport::Udp, state: PortState::Open, service: None, version: None },
        ];
        nas.mdns_services = vec![ServiceInfo {
            service_type: "_smb._tcp".into(),
            name: "NAS (casa)".into(),
            host: "10.0.0.2".into(),
            port: 445,
            domain: None,
            txt: vec![],
        }];
        let camera = host("172.20.20.5", Some("aa:00:00:00:00:05"));
        let mut stray = host("192.168.99.1", None);
        stray.off_subnet = true;
        let printer = host("10.0.0.9", None);
        let mut notes = HashMap::new();
        notes.insert(device_key(&nas), "=Armadio \"rack\";\npiano 1".to_string());
        notes.insert(device_key(&printer), "spenta di notte".to_string());
        let created_at = parse_time("2026-10-07 14:30").unwrap();
        let p = SavedProfile {
            id: "1".into(),
            name: "Ufficio".into(),
            created_at,
            target: "10.0.0.0/24, 172.20.20.0/24".into(),
            scan_profile: ScanProfile::Deep,
            hosts: vec![nas, camera, stray],
            offline_hosts: vec![printer],
            updated_at: Some(created_at + 3_600_000),
            notes,
            vlans: vec![
                ProfileVlan { tag: 69, network: "172.20.20.0/24".into() },
                ProfileVlan { tag: 70, network: "10.0.70.0/24".into() },
            ],
        };
        let empty = SavedProfile {
            id: "2".into(),
            name: "Vuoto".into(),
            hosts: vec![],
            offline_hosts: vec![],
            notes: HashMap::new(),
            vlans: vec![],
            updated_at: None,
            ..p.clone()
        };

        let back = parse_profiles_csv(&profiles_csv(&[p.clone(), empty])).unwrap();
        assert_eq!(back.len(), 2);
        let q = &back[0];
        assert_eq!(q.name, "Ufficio");
        assert_eq!(q.target, p.target);
        assert_eq!(q.scan_profile, ScanProfile::Deep);
        assert_eq!(q.created_at, p.created_at);
        assert_eq!(q.updated_at, p.updated_at);
        assert_eq!(q.vlans, p.vlans);
        assert_eq!(q.notes, p.notes);
        assert_eq!(q.hosts.len(), 3);
        assert_eq!(q.offline_hosts.len(), 1);
        let nas = q.hosts.iter().find(|h| h.ip == "10.0.0.2").unwrap();
        assert_eq!(nas.hostnames, ["nas", "nas.lan"]);
        assert_eq!(nas.vendor.as_deref(), Some("Società; S.p.A."));
        assert_eq!(nas.model.as_deref(), Some("DS920+"));
        assert_eq!(nas.device_type, DeviceType::Nasc);
        assert_eq!(nas.rtt_ms, Some(1.2));
        assert_eq!(nas.open_ports.len(), 2);
        assert_eq!(nas.open_ports[1].transport, Transport::Udp);
        assert_eq!(nas.mdns_services[0].name, "NAS (casa)");
        assert_eq!(nas.mdns_services[0].service_type, "_smb._tcp");
        assert!(q.hosts.iter().find(|h| h.ip == "192.168.99.1").unwrap().off_subnet);
        assert_eq!(q.offline_hosts[0].ip, "10.0.0.9");
        assert_eq!(back[1].name, "Vuoto");
        assert!(back[1].hosts.is_empty());
    }

    #[test]
    fn spreadsheet_resave() {
        let csv = "Profilo,IP,Stato,Salvato il,Note\r\nCasa,192.168.1.1,Acceso,07/10/2026 14:30,router\r\n,,,,\r\n";
        let back = parse_profiles_csv(csv).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].hosts[0].ip, "192.168.1.1");
        assert_eq!(back[0].created_at, parse_time("2026-10-07 14:30").unwrap());
        assert_eq!(back[0].notes.get("ip:192.168.1.1").map(String::as_str), Some("router"));
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse_profiles_csv("a;b\n1;2\n").is_err());
        assert!(parse_profiles_csv("").is_err());
    }
}
