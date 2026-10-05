//! Device-type classification from what a scan already knows about a host.
//!
//! Evidence, strongest first; the first rule that matches wins:
//!
//! 1. **Gateway** — the interface's default router is a `Router`; and
//!    **single-purpose vendors** whose devices carry misleading names (a
//!    Fanvil video phone runs Android and calls itself "Android-3").
//! 2. **Hostname** — device names are explicit when present
//!    ("iPhone-di-…", "MacBook-Pro-…", "ShellyPlus1-…", "SRV-…").
//! 3. **Service fingerprints** — ports that only one kind of device opens:
//!    9100 (raw printing) ⇒ printer, 62078 (iOS lockdown) ⇒ phone.
//! 4. **Vendor** — the IEEE OUI owner, refined by ports where a vendor
//!    builds several kinds of device (Apple, HP).
//! 5. **Generic ports** — RDP/SMB ⇒ a Windows computer.
//!
//! Anything else stays `Unknown`: a guess shown as fact is worse than an
//! honest blank. In particular a randomized (locally administered) MAC with
//! nothing else is *not* assumed to be a phone.

use std::net::Ipv4Addr;

use crate::types::{DeviceType, Host};

/// Hostname substrings (lowercase) → type. Checked in order.
const NAME_RULES: &[(&[&str], DeviceType)] = &[
    (&["ipad", "tablet", "galaxy-tab"], DeviceType::Tablet),
    (
        &[
            "iphone", "android", "galaxy", "pixel", "redmi", "oneplus", "huawei-p",
        ],
        DeviceType::Phone,
    ),
    (
        &[
            "macbook", "book-pro", "book-air", "laptop", "notebook", "thinkpad",
        ],
        DeviceType::Laptop,
    ),
    (
        &[
            "imac",
            "mac-mini",
            "macmini",
            "mac-studio",
            "mac-pro",
            "desktop-",
        ],
        DeviceType::Computer,
    ),
    (
        &["nas", "synology", "diskstation", "qnap", "truenas"],
        DeviceType::Nasc,
    ),
    (
        &[
            "printer",
            "laserjet",
            "officejet",
            "deskjet",
            "brother",
            "epson",
        ],
        DeviceType::Printer,
    ),
    (
        &[
            "appletv",
            "apple-tv",
            "bravia",
            "roku",
            "chromecast",
            "firetv",
            "smarttv",
            "-tv",
        ],
        DeviceType::SmartTv,
    ),
    (
        &["sonos", "homepod", "echo-", "airport-express"],
        DeviceType::Audio,
    ),
    (
        &["playstation", "ps4", "ps5", "xbox", "nintendo"],
        DeviceType::Gaming,
    ),
    (&["camera", "ipcam", "doorbell"], DeviceType::Camera),
    (&["nvr"], DeviceType::Nvr),
    (
        &[
            "shelly",
            "esp-",
            "esp32",
            "esp8266",
            "tasmota",
            "sonoff",
            "tuya",
            "hue-bridge",
            "resiot",
        ],
        DeviceType::Iot,
    ),
    (
        &[
            "srv", "server", "proxmox", "esxi", "lnx", "ubuntu", "debian",
        ],
        DeviceType::Server,
    ),
];

/// Vendor substrings (lowercase) → type, for vendors that make one kind of
/// device. Checked in order, after the Apple/HP special cases.
const VENDOR_RULES: &[(&[&str], DeviceType)] = &[
    (
        &[
            "fanvil",
            "snom",
            "yealink",
            "grandstream",
            "polycom",
            "gigaset",
            "avaya",
            "mitel",
        ],
        DeviceType::Voip,
    ),
    (
        &["sonos", "bose", "denon", "marantz", "bang & olufsen"],
        DeviceType::Audio,
    ),
    (
        &[
            "hikvision",
            "dahua",
            "axis communications",
            "reolink",
            "hanwha",
            "uniview",
            "ezviz",
        ],
        DeviceType::Camera,
    ),
    (
        &["synology", "qnap", "buffalo", "asustor"],
        DeviceType::Nasc,
    ),
    (
        &[
            "brother",
            "canon",
            "seiko epson",
            "kyocera",
            "lexmark",
            "xerox",
            "ricoh",
            "konica",
        ],
        DeviceType::Printer,
    ),
    (&["sony interactive", "nintendo"], DeviceType::Gaming),
    (&["roku", "vizio", "tcl king"], DeviceType::SmartTv),
    (
        &[
            "espressif",
            "allterco",
            "shelly",
            "tuya",
            "itead",
            "signify",
            "philips lighting",
            "nest labs",
            "ring llc",
        ],
        DeviceType::Iot,
    ),
    (
        &["vmware", "proxmox", "qemu", "xensource", "super micro"],
        DeviceType::Server,
    ),
    (
        &[
            "ubiquiti",
            "mikrotik",
            "routerboard",
            "cisco",
            "aruba",
            "ruckus",
            "juniper",
            "sophos",
            "fortinet",
            "palo alto",
            "zyxel",
            "draytek",
            "hewlett packard enterprise",
            "netgear",
            "tp-link",
            "avm",
        ],
        DeviceType::Router,
    ),
    (
        &[
            "dell",
            "lenovo",
            "asustek",
            "intel corporate",
            "micro-star",
            "giga-byte",
            "acer",
            "bizlink",
            "liteon",
            "azurewave",
            "raspberry pi",
        ],
        DeviceType::Computer,
    ),
];

/// Classify `host`. `gateways` are the default routers of the scanned
/// networks.
pub fn classify(host: &Host, gateways: &[Ipv4Addr]) -> DeviceType {
    let has = |p: u16| host.open_ports.iter().any(|port| port.number == p);

    // 1. Gateway.
    if host
        .ip
        .parse::<Ipv4Addr>()
        .is_ok_and(|ip| gateways.contains(&ip))
    {
        return DeviceType::Router;
    }
    let vendor = host.vendor.as_deref().map(str::to_lowercase);
    if let Some(t) = vendor.as_deref().and_then(|v| first_match(v, VENDOR_RULES)) {
        if t == DeviceType::Voip {
            return t;
        }
    }

    // 2. Hostname.
    for name in &host.hostnames {
        let name = name.to_lowercase();
        if let Some(t) = first_match(&name, NAME_RULES) {
            return t;
        }
    }

    // 3. Service fingerprints.
    if has(9100) {
        return DeviceType::Printer;
    }
    if has(62078) {
        return DeviceType::Phone;
    }

    // 4. Vendor.
    if let Some(vendor) = vendor {
        if vendor.starts_with("apple") {
            // Macs offer SSH/SMB/AirPlay/screen sharing; bare Apple devices
            // without services are typically phones/tablets asleep.
            return if [22, 445, 548, 5900, 7000].iter().any(|&p| has(p)) {
                DeviceType::Computer
            } else {
                DeviceType::Unknown
            };
        }
        let hp = vendor.starts_with("hp ") || vendor == "hp inc." || vendor == "hewlett packard";
        if hp && (has(631) || has(515)) {
            return DeviceType::Printer;
        }
        if let Some(t) = first_match(&vendor, VENDOR_RULES) {
            // A server-class OS on PC hardware is still a server.
            if t == DeviceType::Computer && looks_like_server(host) {
                return DeviceType::Server;
            }
            return t;
        }
    }

    // 5. Generic ports.
    if has(3389) || (has(445) && has(139)) {
        return if looks_like_server(host) {
            DeviceType::Server
        } else {
            DeviceType::Computer
        };
    }
    DeviceType::Unknown
}

/// Services that desktops rarely run.
fn looks_like_server(host: &Host) -> bool {
    const SERVER_PORTS: &[u16] = &[53, 1433, 3306, 5432, 2049, 111];
    host.open_ports
        .iter()
        .any(|p| SERVER_PORTS.contains(&p.number))
}

fn first_match(haystack: &str, rules: &[(&[&str], DeviceType)]) -> Option<DeviceType> {
    rules
        .iter()
        .find(|(needles, _)| needles.iter().any(|n| haystack.contains(n)))
        .map(|&(_, t)| t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Port, PortState, Transport};

    fn host(ip: &str, vendor: Option<&str>, name: Option<&str>, ports: &[u16]) -> Host {
        Host {
            ip: ip.into(),
            mac: None,
            vendor: vendor.map(Into::into),
            hostnames: name.into_iter().map(Into::into).collect(),
            device_type: DeviceType::Unknown,
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
            rtt_ms: None,
            mdns_services: vec![],
            ssdp_info: None,
            first_seen: 0,
            last_seen: 0,
        }
    }

    const GW: &[Ipv4Addr] = &[Ipv4Addr::new(172, 31, 31, 1)];

    /// Cases taken from a real office LAN scan.
    #[test]
    fn real_world_cases() {
        let cases: &[(Host, DeviceType)] = &[
            (
                host("172.31.31.1", Some("Sophos Ltd"), None, &[22, 53, 8443]),
                DeviceType::Router,
            ),
            (
                host("172.31.31.2", Some("Sophos Ltd"), None, &[22, 53]),
                DeviceType::Router,
            ),
            (
                host(
                    "172.31.31.30",
                    Some("VMware, Inc."),
                    Some("SRVDCMANXX"),
                    &[53, 445, 3389],
                ),
                DeviceType::Server,
            ),
            (
                host(
                    "172.31.31.51",
                    Some("Hewlett Packard"),
                    Some("HP40B034CB38BF.local"),
                    &[80, 443, 631, 9100],
                ),
                DeviceType::Printer,
            ),
            (
                host(
                    "172.31.31.136",
                    Some("Apple, Inc."),
                    Some("iPhone-di-Fabio.local"),
                    &[62078],
                ),
                DeviceType::Phone,
            ),
            (
                host(
                    "172.31.31.181",
                    None,
                    Some("MacBook-Pro-di-Giulio.local"),
                    &[5000, 7000],
                ),
                DeviceType::Laptop,
            ),
            (
                host(
                    "172.31.31.131",
                    Some("Espressif Inc."),
                    Some("ShellyPro2-8813BFE4574C.local"),
                    &[80],
                ),
                DeviceType::Iot,
            ),
            (
                host("172.31.31.132", Some("Espressif Inc."), None, &[]),
                DeviceType::Iot,
            ),
            (
                host(
                    "172.31.31.128",
                    Some("Fanvil Technology Co., Ltd"),
                    None,
                    &[443, 554],
                ),
                DeviceType::Voip,
            ),
            (
                host(
                    "172.31.31.150",
                    Some("snom technology GmbH"),
                    None,
                    &[80, 443],
                ),
                DeviceType::Voip,
            ),
            (
                host("172.31.31.102", Some("Sonos, Inc."), None, &[]),
                DeviceType::Audio,
            ),
            (
                host("172.31.31.113", Some("Ubiquiti Inc"), None, &[8080]),
                DeviceType::Router,
            ),
            (
                host(
                    "172.31.31.134",
                    Some("Proxmox Server Solutions GmbH"),
                    None,
                    &[22],
                ),
                DeviceType::Server,
            ),
            (
                host(
                    "172.31.31.100",
                    Some("Intel Corporate"),
                    None,
                    &[80, 139, 445],
                ),
                DeviceType::Computer,
            ),
            (
                host("172.31.31.184", Some("Apple, Inc."), None, &[62078]),
                DeviceType::Phone,
            ),
            (
                host("172.31.31.200", None, Some("Android-3.local"), &[]),
                DeviceType::Phone,
            ),
            (
                host(
                    "172.31.31.182",
                    Some("Fanvil Technology Co., Ltd"),
                    Some("Android-3.local"),
                    &[443],
                ),
                DeviceType::Voip,
            ),
            (
                host(
                    "172.31.31.117",
                    Some("BIZLINK TECHNOLOGY, INC."),
                    Some("MackBook-Pro-di-Andrea.local"),
                    &[],
                ),
                DeviceType::Laptop,
            ),
        ];
        for (h, want) in cases {
            assert_eq!(
                classify(h, GW),
                *want,
                "{} {:?} {:?}",
                h.ip,
                h.vendor,
                h.hostnames
            );
        }
    }

    #[test]
    fn no_evidence_stays_unknown() {
        // Randomized MAC (no vendor), no name, no ports.
        assert_eq!(
            classify(&host("172.31.31.143", None, None, &[]), GW),
            DeviceType::Unknown
        );
        // A bare Apple device without services: phone or tablet, can't tell.
        assert_eq!(
            classify(&host("172.31.31.9", Some("Apple, Inc."), None, &[]), GW),
            DeviceType::Unknown
        );
    }

    #[test]
    fn windows_ports_without_vendor() {
        assert_eq!(
            classify(&host("10.0.0.5", None, None, &[139, 445]), &[]),
            DeviceType::Computer
        );
        assert_eq!(
            classify(&host("10.0.0.6", None, None, &[445, 139, 1433]), &[]),
            DeviceType::Server
        );
    }
}
