//! Parsing of user-facing target specs into concrete IPv4 addresses.
//!
//! A target spec is one of (comma-separated lists are accepted anywhere):
//!
//! * a single IP:            `"192.168.1.10"`
//! * a CIDR:                 `"192.168.1.0/24"`
//! * a decimal range:        `"192.168.1.10-200"`
//! * a full range:           `"192.168.1.10-192.168.1.200"`
//!
//! For CIDRs up to `/30` the network and broadcast addresses are excluded
//! (they are not hosts, and a broadcast probe would be ambiguous). A `/31` is a
//! point-to-point link with two usable addresses (RFC 3021); a `/32` is the
//! single address.

use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

use crate::error::ScanError;

/// A parsed target spec, either an explicit address list or a CIDR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSpec {
    /// Explicit single-host addresses (a single-IP spec is a one-element list).
    Ipv4(Vec<Ipv4Addr>),
    /// A CIDR; expanded at scan time (network/broadcast rules applied).
    Cidr(Ipv4Net),
}

impl TargetSpec {
    /// Every concrete address this spec covers.
    pub fn expand(&self) -> Vec<Ipv4Addr> {
        match self {
            Self::Ipv4(addrs) => addrs.clone(),
            Self::Cidr(net) => expand_cidr(net),
        }
    }

    /// How many addresses [`TargetSpec::expand`] yields (cheap for CIDRs).
    pub fn host_count(&self) -> u64 {
        match self {
            Self::Ipv4(addrs) => addrs.len() as u64,
            Self::Cidr(net) => cidr_host_count(net),
        }
    }
}

/// Expand a CIDR into concrete addresses: network and broadcast excluded up
/// to `/30`, both addresses of a `/31`, the single address of a `/32`.
pub fn expand_cidr(net: &Ipv4Net) -> Vec<Ipv4Addr> {
    let first = u32::from(net.network());
    let last = u32::from(net.broadcast());
    match net.prefix_len() {
        31 | 32 => (first..=last).map(Ipv4Addr::from).collect(),
        _ => (first + 1..last).map(Ipv4Addr::from).collect(),
    }
}

/// Number of addresses [`expand_cidr`] would yield, without allocating.
pub fn cidr_host_count(net: &Ipv4Net) -> u64 {
    match net.prefix_len() {
        p @ (31 | 32) => 1u64 << (32 - p),
        p => (1u64 << (32 - p)) - 2,
    }
}

/// Parse a single (no commas) target spec.
pub fn parse_spec(spec: &str) -> Result<TargetSpec, ScanError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(ScanError::InvalidConfig("empty target".into()));
    }

    // CIDR: "a.b.c.d/n" (n: 0..32). Checked first: a bare IPv4 never
    // contains '/', so this can't shadow the other forms.
    if spec.contains('/') {
        return Ok(TargetSpec::Cidr(parse_cidr(spec).map_err(|e| {
            ScanError::InvalidConfig(format!("'{spec}': {e}"))
        })?));
    }

    // Range: "a.b.c.d-e" or "a.b.c.d-w.x.y.z" — contains exactly one '-'.
    if spec.contains('-') {
        return parse_range(spec)
            .map(TargetSpec::Ipv4)
            .map_err(|e| ScanError::InvalidConfig(format!("'{spec}': {e}")));
    }

    // Single IP.
    let addr: Ipv4Addr = spec
        .parse()
        .map_err(|_| ScanError::InvalidConfig(format!("'{spec}' is not an IPv4 address")))?;
    Ok(TargetSpec::Ipv4(vec![addr]))
}

/// Parse `targets` — a `Vec` of specs, each of which may itself be a
/// comma-separated list. The result preserves order and contains no
/// duplicates (a duplicate is an invalid config: it would double-probe).
pub fn parse_targets(targets: &[String]) -> Result<Vec<TargetSpec>, ScanError> {
    let mut out: Vec<TargetSpec> = Vec::new();
    for target in targets {
        for piece in target.split(',') {
            let piece = piece.trim();
            if piece.is_empty() {
                // "192.168.1.1," — trailing comma, not an error.
                continue;
            }
            let spec = parse_spec(piece)?;
            if !out.iter().any(|s| s == &spec) {
                out.push(spec);
            }
        }
    }
    if out.is_empty() {
        return Err(ScanError::InvalidConfig("no valid targets".into()));
    }
    Ok(out)
}

fn parse_cidr(spec: &str) -> Result<Ipv4Net, String> {
    let (addr, prefix) = spec
        .split_once('/')
        .ok_or_else(|| "missing '/prefix'".to_string())?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| "prefix is not a number".to_string())?;
    if prefix > 32 {
        return Err(format!("prefix /{prefix} is out of range (0..=32)"));
    }
    let addr: Ipv4Addr = addr
        .parse()
        .map_err(|_| "address part is not an IPv4 address".to_string())?;
    // Normalise "192.168.1.37/24" to its network so equal CIDRs dedupe.
    Ipv4Net::new(addr, prefix)
        .map(|n| n.trunc())
        .map_err(|e| e.to_string())
}

fn parse_range(spec: &str) -> Result<Vec<Ipv4Addr>, String> {
    let (lo, hi) = spec
        .split_once('-')
        .ok_or_else(|| "range requires two parts separated by '-'".to_string())?;
    let lo = lo.trim();
    let hi = hi.trim();

    if hi.contains('.') {
        // Full range "a.b.c.d-w.x.y.z": both sides are complete addresses.
        let start: Ipv4Addr = lo
            .parse()
            .map_err(|_| format!("'{lo}' is not an IPv4 address"))?;
        let end: Ipv4Addr = hi
            .parse()
            .map_err(|_| format!("'{hi}' is not an IPv4 address"))?;
        range_full(start, end)
    } else {
        // Short range "a.b.c.d-e": the leading part is an address, the
        // trailing part a final octet. (Trailing-digits with a dot would
        // have matched the full-range arm above, so `lo` is complete here.)
        let addr: Ipv4Addr = lo
            .parse()
            .map_err(|_| format!("'{lo}' is not an IPv4 address"))?;
        let last: u8 = hi
            .parse()
            .map_err(|_| format!("'{hi}' is not a final octet (0..=255)"))?;
        range_short(addr, last)
    }
}

fn range_full(start: Ipv4Addr, end: Ipv4Addr) -> Result<Vec<Ipv4Addr>, String> {
    if start > end {
        return Err(format!("range start {start} is after end {end}"));
    }
    Ok((start..=end).collect())
}

fn range_short(addr: Ipv4Addr, last: u8) -> Result<Vec<Ipv4Addr>, String> {
    let o = addr.octets();
    if o[3] > last {
        return Err(format!(
            "range start {addr} (last octet {}) is after end octet {last}",
            o[3]
        ));
    }
    let base = (o[0] as u32) << 24 | (o[1] as u32) << 16 | (o[2] as u32) << 8;
    let mut out = Vec::with_capacity(last as usize - o[3] as usize + 1);
    for l in o[3]..=last {
        out.push(Ipv4Addr::from(base | l as u32));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn single_ip() {
        assert_eq!(
            parse_spec("192.168.1.10").unwrap(),
            TargetSpec::Ipv4(vec![v4("192.168.1.10")])
        );
    }

    #[test]
    fn single_ipv6_rejected() {
        // The engine is IPv4-only in Phase 2.
        assert!(parse_spec("fe80::1").is_err());
    }

    #[test]
    fn cidr_excludes_network_and_broadcast() {
        let spec = parse_spec("192.168.1.0/24").unwrap();
        let addrs = spec.expand();
        assert_eq!(addrs.len(), 254);
        assert_eq!(spec.host_count(), 254);
        assert_eq!(addrs[0], v4("192.168.1.1"));
        assert_eq!(addrs[253], v4("192.168.1.254"));
        assert_eq!(
            spec,
            TargetSpec::Cidr(Ipv4Net::new(v4("192.168.1.0"), 24).unwrap())
        );
    }

    #[test]
    fn cidr_31_keeps_both_addresses() {
        // RFC 3021 point-to-point: both addresses are hosts.
        let spec = parse_spec("10.0.0.0/31").unwrap();
        assert_eq!(spec.expand(), vec![v4("10.0.0.0"), v4("10.0.0.1")]);
        assert_eq!(spec.host_count(), 2);
    }

    #[test]
    fn cidr_32_is_the_single_address() {
        assert_eq!(
            parse_spec("10.0.0.7/32").unwrap().expand(),
            vec![v4("10.0.0.7")]
        );
    }

    #[test]
    fn cidr_30_excludes_network_and_broadcast() {
        let spec = parse_spec("10.0.0.0/30").unwrap();
        assert_eq!(spec.expand(), vec![v4("10.0.0.1"), v4("10.0.0.2")]);
    }

    #[test]
    fn cidr_host_part_is_normalised() {
        // "192.168.1.37/24" means the whole /24.
        assert_eq!(parse_spec("192.168.1.37/24").unwrap().expand().len(), 254);
    }

    #[test]
    fn cidr_prefix_out_of_range_rejected() {
        assert!(parse_spec("192.168.1.0/33").is_err());
        assert!(parse_spec("192.168.1.0/").is_err());
        assert!(parse_spec("192.168.1.0/x").is_err());
    }

    #[test]
    fn range_short_form() {
        let spec = parse_spec("192.168.1.10-200").unwrap();
        let addrs = spec.expand();
        assert_eq!(addrs.len(), 191);
        assert_eq!(addrs[0], v4("192.168.1.10"));
        assert_eq!(addrs[1], v4("192.168.1.11"));
        assert_eq!(addrs[190], v4("192.168.1.200"));
    }

    #[test]
    fn range_short_form_single_address_when_equal() {
        assert_eq!(
            parse_spec("192.168.1.10-10").unwrap().expand(),
            vec![v4("192.168.1.10")]
        );
    }

    #[test]
    fn range_short_form_inverted_rejected() {
        assert!(parse_spec("192.168.1.50-10").is_err());
    }

    #[test]
    fn range_full_form() {
        let spec = parse_spec("192.168.1.100-192.168.2.5").unwrap();
        let addrs = spec.expand();
        assert_eq!(addrs[0], v4("192.168.1.100"));
        assert_eq!(addrs.last().unwrap(), &v4("192.168.2.5"));
        assert_eq!(addrs.len(), 162); // .1.100..=.1.255 = 156, .2.0..=.2.5 = 6
    }

    #[test]
    fn range_full_form_inverted_rejected() {
        assert!(parse_spec("192.168.1.50-192.168.1.10").is_err());
    }

    #[test]
    fn range_malformed_rejected() {
        assert!(parse_spec("192.168.1.10-").is_err());
        assert!(parse_spec("-192.168.1.10").is_err());
        assert!(parse_spec("192.168.1.10-abc").is_err());
        assert!(parse_spec("not-an-ip-foo").is_err());
    }

    #[test]
    fn comma_separated_list() {
        let specs = parse_targets(&["192.168.1.0/24, 10.0.0.1, 10.0.0.5-8".into()]).unwrap();
        assert_eq!(specs.len(), 3);
        assert!(matches!(&specs[0], TargetSpec::Cidr(n) if n.prefix_len() == 24));
        assert_eq!(specs[1], TargetSpec::Ipv4(vec![v4("10.0.0.1")]));
        assert_eq!(
            specs[2].expand(),
            vec![
                v4("10.0.0.5"),
                v4("10.0.0.6"),
                v4("10.0.0.7"),
                v4("10.0.0.8")
            ]
        );
    }

    #[test]
    fn duplicate_spec_in_list_deduplicated() {
        let specs = parse_targets(&["192.168.1.10, 192.168.1.10".into()]).unwrap();
        assert_eq!(specs.len(), 1);
    }

    #[test]
    fn empty_and_garbage_lists_rejected() {
        assert!(parse_targets(&[String::new()]).is_err());
        assert!(parse_targets(&[" , ".into()]).is_err());
        assert!(parse_targets(&["999.999.999.999".into()]).is_err());
    }

    #[test]
    fn total_address_count_across_specs() {
        let specs = parse_targets(&["192.168.1.0/24".into()]).unwrap();
        let total = specs.iter().map(|s| s.expand().len()).sum::<usize>();
        assert_eq!(total, 254);
    }
}
