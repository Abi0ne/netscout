//! CSV export of saved profiles, one row per device.
//!
//! The file is meant to open as-is in a spreadsheet set to Italian: UTF-8
//! with a byte-order mark (so accented letters are read right), `;` between
//! fields and a decimal comma, CRLF line ends. Fields are quoted per RFC 4180
//! when they hold the separator, a quote, a line break or surrounding blanks,
//! and text starting like a formula (`=`, `+`, `-`, `@`) gets a leading `'`,
//! since names and notes can come from devices on the network.

use crate::profiles::{device_key, SavedProfile};
use crate::types::{DeviceType, Host, PortState, ScanProfile, Transport};

const SEPARATOR: char = ';';

const HEADER: [&str; 17] = [
    "Profilo",
    "Rete scansionata",
    "Tipo di scansione",
    "Salvato il",
    "Aggiornato il",
    "Stato",
    "IP",
    "Nome",
    "Altri nomi",
    "Tipo",
    "Produttore",
    "MAC",
    "Latenza (ms)",
    "Porte aperte",
    "Servizi mDNS",
    "Dispositivo UPnP",
    "Note",
];

/// `profiles` as CSV text, devices up first, then those off, each by IP. A
/// profile without devices still gets a row.
#[uniffi::export]
pub fn profiles_csv(profiles: &[SavedProfile]) -> String {
    let mut out = String::from("\u{feff}");
    push_row(&mut out, HEADER.iter().map(|s| s.to_string()));
    for p in profiles {
        let head = [
            p.name.clone(),
            p.target.clone(),
            scan_label(p.scan_profile).into(),
            local_time(p.created_at),
            p.updated_at.map(local_time).unwrap_or_default(),
        ];
        let mut devices: Vec<(&Host, bool)> = p
            .hosts
            .iter()
            .map(|h| (h, false))
            .chain(p.offline_hosts.iter().map(|h| (h, true)))
            .collect();
        devices.sort_by_key(|(h, off)| (*off, ip_key(&h.ip)));
        if devices.is_empty() {
            push_row(
                &mut out,
                head.iter()
                    .cloned()
                    .chain(std::iter::repeat_n(String::new(), 12)),
            );
        }
        for (host, off) in devices {
            let note = p.notes.get(&device_key(host)).cloned().unwrap_or_default();
            push_row(
                &mut out,
                head.iter().cloned().chain(device_fields(host, off, note)),
            );
        }
    }
    out
}

fn device_fields(h: &Host, off: bool, note: String) -> [String; 12] {
    let names: Vec<String> = h
        .hostnames
        .iter()
        .map(|n| n.strip_suffix(".local").unwrap_or(n).to_string())
        .collect();
    let ports: Vec<String> = h
        .open_ports
        .iter()
        .filter(|p| p.state == PortState::Open)
        .map(|p| {
            let proto = match p.transport {
                Transport::Tcp => "",
                Transport::Udp => "/udp",
            };
            match &p.service {
                Some(s) => format!("{}{proto} {s}", p.number),
                None => format!("{}{proto}", p.number),
            }
        })
        .collect();
    let mut services: Vec<String> = h
        .mdns_services
        .iter()
        .map(|s| format!("{} ({})", s.name, s.service_type))
        .collect();
    services.dedup();
    [
        if off { "Spento" } else { "Acceso" }.into(),
        h.ip.clone(),
        names.first().cloned().unwrap_or_default(),
        names.get(1..).unwrap_or_default().join(", "),
        type_label(h.device_type).into(),
        h.vendor.clone().unwrap_or_default(),
        h.mac.clone().unwrap_or_default(),
        match (off, h.rtt_ms) {
            (false, Some(ms)) => format!("{ms:.1}").replace('.', ","),
            _ => String::new(),
        },
        ports.join(", "),
        services.join(", "),
        h.ssdp_info
            .as_ref()
            .and_then(|s| s.friendly_name.clone())
            .unwrap_or_default(),
        note,
    ]
}

fn push_row(out: &mut String, fields: impl Iterator<Item = String>) {
    for (i, field) in fields.enumerate() {
        if i > 0 {
            out.push(SEPARATOR);
        }
        out.push_str(&escape(&field));
    }
    out.push_str("\r\n");
}

/// One field, quoted when needed and defused when it reads like a formula.
fn escape(field: &str) -> String {
    let field = if field.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{field}")
    } else {
        field.to_string()
    };
    let needs_quotes = field.contains([SEPARATOR, '"', '\n', '\r'])
        || field.starts_with(char::is_whitespace)
        || field.ends_with(char::is_whitespace);
    if needs_quotes {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field
    }
}

/// Local date and time, "2026-10-07 14:30" (spreadsheets read it as a date).
fn local_time(epoch_ms: i64) -> String {
    let secs = (epoch_ms / 1000) as libc::time_t;
    // SAFETY: localtime_r only writes the `tm` we hand it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

fn ip_key(ip: &str) -> u32 {
    ip.parse::<std::net::Ipv4Addr>()
        .map(u32::from)
        .unwrap_or(u32::MAX)
}

fn scan_label(p: ScanProfile) -> &'static str {
    match p {
        ScanProfile::Quick => "Rapida",
        ScanProfile::Standard => "Standard",
        ScanProfile::Deep => "Approfondita",
    }
}

/// The apps' names for the device types.
fn type_label(t: DeviceType) -> &'static str {
    match t {
        DeviceType::Router => "Rete",
        DeviceType::Modem => "Modem",
        DeviceType::Computer => "Computer",
        DeviceType::Laptop => "Portatile",
        DeviceType::Server => "Server",
        DeviceType::Phone => "Telefono",
        DeviceType::Tablet => "Tablet",
        DeviceType::Printer => "Stampante",
        DeviceType::Camera => "Telecamera",
        DeviceType::Nvr => "NVR",
        DeviceType::Nasc => "NAS",
        DeviceType::SmartTv => "TV",
        DeviceType::Gaming => "Console",
        DeviceType::Voip => "Telefono VoIP",
        DeviceType::Audio => "Audio",
        DeviceType::Iot => "IoT",
        DeviceType::Other => "Altro",
        DeviceType::Unknown => "Sconosciuto",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Port;
    use std::collections::HashMap;

    #[test]
    fn escaping() {
        assert_eq!(escape("plain"), "plain");
        assert_eq!(escape("a;b"), "\"a;b\"");
        assert_eq!(escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(escape("two\nlines"), "\"two\nlines\"");
        assert_eq!(escape(" padded"), "\" padded\"");
        assert_eq!(escape("=1+1"), "'=1+1");
        assert_eq!(escape("-x;y"), "\"'-x;y\"");
        assert_eq!(escape("Città"), "Città");
    }

    #[test]
    fn rows_have_every_column() {
        let host = Host {
            ip: "10.0.0.2".into(),
            mac: Some("AA:00:00:00:00:02".into()),
            vendor: Some("Società; S.p.A.".into()),
            hostnames: vec!["nas.local".into(), "nas.lan".into()],
            device_type: DeviceType::Nasc,
            open_ports: vec![Port {
                number: 445,
                transport: Transport::Tcp,
                state: PortState::Open,
                service: Some("smb".into()),
                version: None,
            }],
            rtt_ms: Some(1.25),
            mdns_services: vec![],
            ssdp_info: None,
            first_seen: 0,
            last_seen: 0,
        };
        let mut notes = HashMap::new();
        notes.insert(
            "mac:aa:00:00:00:00:02".into(),
            "Armadio \"rack\" piano 1".into(),
        );
        let p = SavedProfile {
            id: "1".into(),
            name: "Ufficio".into(),
            created_at: 0,
            target: "10.0.0.0/24".into(),
            scan_profile: ScanProfile::Deep,
            hosts: vec![host.clone()],
            offline_hosts: vec![],
            updated_at: None,
            notes,
        };
        let empty = SavedProfile {
            id: "2".into(),
            name: "Vuoto".into(),
            hosts: vec![],
            notes: HashMap::new(),
            ..p.clone()
        };
        let csv = profiles_csv(&[p, empty]);
        assert!(csv.starts_with("\u{feff}Profilo;Rete scansionata;"));
        let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').split("\r\n").collect();
        assert_eq!(lines.len(), 4, "{csv}"); // header, device, empty profile, ""
        assert!(lines[1].starts_with("Ufficio;10.0.0.0/24;Approfondita;"));
        assert!(lines[1].ends_with(
            ";Acceso;10.0.0.2;nas;nas.lan;NAS;\"Società; S.p.A.\";AA:00:00:00:00:02;1,2;445 smb;;;\"Armadio \"\"rack\"\" piano 1\""
        ), "{}", lines[1]);
        assert_eq!(lines[2].matches(';').count(), HEADER.len() - 1);
        assert_eq!(lines[3], "");
    }
}
