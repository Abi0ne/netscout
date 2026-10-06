//! User-facing names and icons for the engine's enums (Italian, like the
//! macOS app), plus small display helpers on `Host`.

use netscout_core::{DeviceType, Host, ScanPhase, ScanProfile};

/// Every device type, in the order the sidebar lists them on a tie.
pub const ALL_TYPES: [DeviceType; 18] = [
    DeviceType::Router,
    DeviceType::Modem,
    DeviceType::Computer,
    DeviceType::Laptop,
    DeviceType::Server,
    DeviceType::Phone,
    DeviceType::Tablet,
    DeviceType::Printer,
    DeviceType::Camera,
    DeviceType::Nvr,
    DeviceType::Nasc,
    DeviceType::SmartTv,
    DeviceType::Gaming,
    DeviceType::Voip,
    DeviceType::Audio,
    DeviceType::Iot,
    DeviceType::Other,
    DeviceType::Unknown,
];

pub fn device_label(t: DeviceType) -> &'static str {
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

/// Adwaita symbolic icon name.
pub fn device_icon(t: DeviceType) -> &'static str {
    match t {
        DeviceType::Router => "network-wireless-symbolic",
        DeviceType::Modem => "modem-symbolic",
        DeviceType::Computer | DeviceType::Laptop => "computer-symbolic",
        DeviceType::Server => "network-server-symbolic",
        DeviceType::Phone => "phone-symbolic",
        DeviceType::Tablet => "tablet-symbolic",
        DeviceType::Printer => "printer-symbolic",
        DeviceType::Camera => "camera-web-symbolic",
        DeviceType::Nvr => "camera-video-symbolic",
        DeviceType::Nasc => "drive-multidisk-symbolic",
        DeviceType::SmartTv => "tv-symbolic",
        DeviceType::Gaming => "input-gaming-symbolic",
        DeviceType::Voip => "call-start-symbolic",
        DeviceType::Audio => "audio-speakers-symbolic",
        DeviceType::Iot => "applications-science-symbolic",
        DeviceType::Other => "network-workgroup-symbolic",
        DeviceType::Unknown => "dialog-question-symbolic",
    }
}

pub const PROFILES: [ScanProfile; 3] =
    [ScanProfile::Quick, ScanProfile::Standard, ScanProfile::Deep];

pub fn profile_label(p: ScanProfile) -> &'static str {
    match p {
        ScanProfile::Quick => "Rapida",
        ScanProfile::Standard => "Standard",
        ScanProfile::Deep => "Approfondita",
    }
}

/// Stable name used for the profile toggles.
pub fn profile_name(p: ScanProfile) -> &'static str {
    match p {
        ScanProfile::Quick => "quick",
        ScanProfile::Standard => "standard",
        ScanProfile::Deep => "deep",
    }
}

pub fn phase_label(p: ScanPhase) -> &'static str {
    match p {
        ScanPhase::Probing => "Ricerca dispositivi",
        ScanPhase::Resolving => "Nomi e produttori",
        ScanPhase::PortScanning => "Porte",
        ScanPhase::Done => "Completata",
    }
}

/// Best display name: first hostname without the ".local" suffix.
pub fn display_name(host: &Host) -> Option<String> {
    host.hostnames
        .first()
        .map(|n| n.strip_suffix(".local").unwrap_or(n).to_string())
}

/// Locally administered MAC: phones and tablets use random ones per network.
pub fn has_private_mac(host: &Host) -> bool {
    host.mac
        .as_deref()
        .and_then(|m| m.get(..2))
        .and_then(|b| u8::from_str_radix(b, 16).ok())
        .is_some_and(|b| b & 0x02 != 0)
}

/// Numeric value of a dotted IPv4 address, for sorting.
pub fn ip_value(ip: &str) -> u32 {
    ip.parse::<std::net::Ipv4Addr>().map(u32::from).unwrap_or(0)
}

/// `http(s)://ip:port` for the ports that usually serve a web page.
pub fn web_url(ip: &str, port: u16) -> Option<String> {
    match port {
        80 | 8080 | 8000 | 8008 | 8888 | 3000 => Some(format!("http://{ip}:{port}")),
        443 | 8443 => Some(format!("https://{ip}:{port}")),
        _ => None,
    }
}

/// Local date and time of an epoch-ms timestamp ("6 ott 2026, 21:13").
pub fn format_time(epoch_ms: i64, with_date: bool) -> String {
    format_epoch(
        epoch_ms,
        if with_date {
            "%-d %b %Y, %H:%M"
        } else {
            "%H:%M"
        },
    )
}

/// Local time with seconds of an epoch-ms timestamp ("21:13:05").
pub fn format_clock(epoch_ms: i64) -> String {
    format_epoch(epoch_ms, "%H:%M:%S")
}

fn format_epoch(epoch_ms: i64, format: &str) -> String {
    gtk::glib::DateTime::from_unix_local(epoch_ms / 1000)
        .and_then(|d| d.format(format))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "—".into())
}
