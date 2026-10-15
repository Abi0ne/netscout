//! NetScout's root helper on Linux, one per session (the macOS app's
//! `VLANAgent`): `netscout --agent COMMANDS APP_PID`, started through
//! `pkexec`.
//!
//! Listening to the frames on the cable and creating a VLAN interface both
//! need an administrator. Instead of asking for the password each time,
//! NetScout asks once: `pkexec` starts this same program as root, which
//! leaves a copy of itself in the background and returns. The copy takes the
//! app's commands until NetScout quits (or crashes), then removes the
//! interfaces it made and its directory.
//!
//! Commands go through a directory of the app's (`N.cmd`, one line each),
//! answers come back in a directory of the agent's (`N.res`: a status, 0 =
//! done, and a line of text), owned by root so only the agent can replace
//! them, readable by the user alone. The agent never runs what it reads: it
//! knows five commands, and checks every argument (a tag is a number, a port
//! an existing interface, an address four numbers).
//!
//! - `up TAG PORT dhcp|manual ADDRESS NETMASK`: makes a VLAN interface on
//!   PORT and answers "vlan69 172.20.20.250/24". With DHCP, only the address
//!   is taken: the VLAN's router and DNS are never used, so it can't take
//!   over the default route or the name servers.
//! - `down INTERFACE`: removes one it made.
//! - `listen PORT SECONDS`: listens for the tagged frames other devices send
//!   on PORT, summed up line by line ("frames TAG COUNT", "mac TAG MAC",
//!   "net TAG A.B.C COUNT", then "end") in the file it answers with.
//! - `quiet`: stops listening.
//! - `neighbor PORT SECONDS`: waits up to SECONDS for the LLDP or CDP
//!   announcement of the device at the other end of PORT's cable, and
//!   answers once done with the path of a capture file (pcap, without
//!   frames if none came).
//!
//! Frames are read with packet sockets (no tcpdump), and DHCP is spoken
//! here too (no dhclient), so nothing beyond `ip` (iproute2) is needed.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;
use std::io::Write;
use std::net::Ipv4Addr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `PATH` for the tools the agent runs.
const PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

/// Entry point of `netscout --agent COMMANDS APP_PID`; returns the exit code.
pub fn main(args: &[String]) -> i32 {
    let (Some(commands), Some(app)) = (args.first(), args.get(1)) else {
        eprintln!("usage: netscout --agent COMMANDS APP_PID");
        return 2;
    };
    // SAFETY: plain libc call.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("netscout --agent must run as root (through pkexec)");
        return 2;
    }
    let Some(owner) = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|u| u.parse::<u32>().ok())
    else {
        eprintln!("netscout --agent must be started through pkexec");
        return 2;
    };
    let Ok(app) = app.parse::<i32>() else {
        return 2;
    };
    let commands = PathBuf::from(commands);
    // The commands directory must be the user's own, private, real one.
    match fs::symlink_metadata(&commands) {
        Ok(m) if m.is_dir() && m.uid() == owner && m.mode() & 0o077 == 0 => {}
        _ => {
            eprintln!("bad commands directory");
            return 2;
        }
    }
    let Some(out) = make_out_dir() else {
        eprintln!("cannot create the agent directory");
        return 1;
    };

    // Leave a copy in the background and return, so pkexec returns too.
    // SAFETY: single-threaded here; the child only goes on with this code.
    match unsafe { libc::fork() } {
        -1 => {
            let _ = fs::remove_dir_all(&out);
            return 1;
        }
        0 => {}
        _ => {
            println!("{}", out.display());
            return 0;
        }
    }
    detach();
    Agent {
        commands,
        out,
        owner,
        app,
        made: Vec::new(),
        listening: None,
    }
    .run();
    0
}

/// The background copy: its own session, deaf to the terminal's and the
/// desktop's signals (only KILL stops it), and no stdio.
fn detach() {
    // SAFETY: plain libc calls on our own process.
    unsafe {
        libc::setsid();
        for sig in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM, libc::SIGPIPE] {
            libc::signal(sig, libc::SIG_IGN);
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null >= 0 {
            for fd in 0..3 {
                libc::dup2(null, fd);
            }
            if null > 2 {
                libc::close(null);
            }
        }
    }
}

fn make_out_dir() -> Option<PathBuf> {
    let mut template = *b"/tmp/netscout-agent.XXXXXX\0";
    // SAFETY: a writable NUL-terminated template.
    let p = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
    if p.is_null() {
        return None;
    }
    let path = PathBuf::from(std::str::from_utf8(&template[..template.len() - 1]).ok()?);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).ok()?;
    Some(path)
}

struct Agent {
    commands: PathBuf,
    out: PathBuf,
    owner: u32,
    app: i32,
    /// The VLAN interfaces made, removed on the way out.
    made: Vec<String>,
    /// Stops the listening in progress.
    listening: Option<Arc<AtomicBool>>,
}

impl Agent {
    fn run(&mut self) {
        let mut n: u64 = 1;
        // SAFETY: kill with signal 0 only checks the process exists.
        while unsafe { libc::kill(self.app, 0) } == 0 && self.commands.is_dir() {
            let file = self.commands.join(format!("{n}.cmd"));
            match fs::symlink_metadata(&file) {
                Ok(m) if m.is_file() => {
                    let line = fs::read_to_string(&file).unwrap_or_default();
                    let _ = fs::remove_file(&file);
                    let words: Vec<&str> = line.split_whitespace().collect();
                    self.command(n, &words);
                    n += 1;
                }
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        self.quiet();
        for v in std::mem::take(&mut self.made) {
            remove_link(&v);
        }
        let _ = fs::remove_dir_all(&self.out);
    }

    fn command(&mut self, n: u64, words: &[&str]) {
        let arg = |i: usize| words.get(i).copied().unwrap_or("");
        match arg(0) {
            "up" => {
                let (status, text) = self.up(arg(1), arg(2), arg(3), arg(4), arg(5));
                self.reply(n, status, &text);
            }
            "down" => {
                let name = arg(1).to_string();
                if let Some(i) = self.made.iter().position(|v| *v == name) {
                    self.made.remove(i);
                    remove_link(&name);
                    self.reply(n, 0, "");
                } else {
                    self.reply(n, 2, "interfaccia sconosciuta");
                }
            }
            "listen" => self.listen(n, arg(1), arg(2)),
            "quiet" => {
                self.quiet();
                self.reply(n, 0, "");
            }
            "neighbor" => self.neighbor(n, arg(1), arg(2)),
            _ => self.reply(n, 2, "comando sconosciuto"),
        }
    }

    /// The answer to command `n`: its status (0 = done) and text.
    fn reply(&self, n: u64, status: u8, text: &str) {
        write_reply(&self.out, self.owner, n, status, text);
    }

    // -- VLAN interfaces ------------------------------------------------------

    fn up(&mut self, tag: &str, parent: &str, mode: &str, addr: &str, mask: &str) -> (u8, String) {
        let invalid = (2, "richiesta non valida".to_string());
        let Some(tag) = tag.parse::<u16>().ok().filter(|t| (1..=4094).contains(t)) else {
            return invalid;
        };
        if !is_interface(parent) {
            return invalid;
        }
        let manual = match mode {
            "dhcp" => None,
            "manual" => match (addr.parse::<Ipv4Addr>(), mask.parse::<Ipv4Addr>()) {
                (Ok(a), Ok(m)) => Some((a, u32::from(m).count_ones())),
                _ => return invalid,
            },
            _ => return invalid,
        };
        if let Some(existing) = existing_vlan(parent, tag) {
            return (3, format!("la VLAN {tag} su {parent} esiste già ({existing})"));
        }
        let Some(name) = [format!("vlan{tag}"), format!("nsvlan{tag}")]
            .into_iter()
            .find(|n| !Path::new("/sys/class/net").join(n).exists())
        else {
            return (1, format!("nessun nome libero per l'interfaccia della VLAN {tag}"));
        };
        if let Err(e) = ip(&[
            "link", "add", "link", parent, "name", &name, "type", "vlan", "id",
            &tag.to_string(),
        ]) {
            return (1, format!("impossibile creare {name}: {e}"));
        }
        // No IPv6 autoconfiguration and no NetworkManager on it: the
        // interface carries only the address given here.
        let _ = fs::write(format!("/proc/sys/net/ipv6/conf/{name}/disable_ipv6"), "1");
        let _ = Command::new("nmcli")
            .args(["device", "set", &name, "managed", "no"])
            .env("PATH", PATH)
            .output();
        if let Err(e) = ip(&["link", "set", &name, "up"]) {
            remove_link(&name);
            return (1, format!("impossibile collegare {name} a {parent}: {e}"));
        }
        let (address, prefix) = match manual {
            Some(m) => m,
            None => match crate::dhcp::lease(&name, Duration::from_secs(30)) {
                Ok(lease) => lease,
                Err(e) => {
                    remove_link(&name);
                    return (4, format!("nessuna risposta dal DHCP della VLAN {tag} in 30 secondi ({e})"));
                }
            },
        };
        if let Err(e) = ip(&["addr", "add", &format!("{address}/{prefix}"), "dev", &name]) {
            remove_link(&name);
            return (1, format!("indirizzo non accettato: {e}"));
        }
        self.made.push(name.clone());
        (0, format!("{name} {address}/{prefix}"))
    }

    // -- Listening ------------------------------------------------------------

    fn quiet(&mut self) {
        if let Some(stop) = self.listening.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// The tagged frames other devices send on `parent`, summed up line by
    /// line in N.vlans until "end".
    fn listen(&mut self, n: u64, parent: &str, secs: &str) {
        let Some(secs) = secs.parse::<u64>().ok().filter(|s| *s <= 600) else {
            return self.reply(n, 2, "richiesta non valida");
        };
        if !is_interface(parent) {
            return self.reply(n, 2, "richiesta non valida");
        }
        self.quiet();
        let Some(own) = mac_of(parent) else {
            return self.reply(n, 1, &format!("{parent} non ha un indirizzo MAC"));
        };
        let socket = match PacketSocket::open(parent, true) {
            Ok(s) => s,
            Err(e) => return self.reply(n, 1, &format!("impossibile ascoltare {parent}: {e}")),
        };
        let path = self.out.join(format!("{n}.vlans"));
        let Some(file) = own_file(&path, self.owner) else {
            return self.reply(n, 1, "impossibile creare il file dei risultati");
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.listening = Some(stop.clone());
        std::thread::spawn(move || {
            summarize_vlans(socket, own, file, Duration::from_secs(secs), &stop)
        });
        self.reply(n, 0, &path.display().to_string());
    }

    /// The first LLDP or CDP frame on `parent`, in N.pcap; answered when it
    /// came or the time is up.
    fn neighbor(&mut self, n: u64, parent: &str, secs: &str) {
        let Some(secs) = secs.parse::<u64>().ok().filter(|s| *s <= 300) else {
            return self.reply(n, 2, "richiesta non valida");
        };
        if !is_interface(parent) {
            return self.reply(n, 2, "richiesta non valida");
        }
        let socket = match PacketSocket::open(parent, true) {
            Ok(s) => s,
            Err(e) => return self.reply(n, 1, &format!("impossibile ascoltare {parent}: {e}")),
        };
        let path = self.out.join(format!("{n}.pcap"));
        let Some(mut file) = own_file(&path, self.owner) else {
            return self.reply(n, 1, "impossibile creare il file dei risultati");
        };
        let (out, owner) = (self.out.clone(), self.owner);
        std::thread::spawn(move || {
            let _ = file.write_all(&pcap_header());
            let deadline = Instant::now() + Duration::from_secs(secs);
            let mut buf = vec![0u8; 9216];
            while Instant::now() < deadline {
                let Some(frame) = socket.receive(&mut buf) else {
                    continue;
                };
                if frame.outgoing {
                    continue;
                }
                let data = &buf[..frame.len];
                if is_neighbor_frame(data) {
                    let _ = file.write_all(&pcap_record(data));
                    break;
                }
            }
            drop(file);
            write_reply(&out, owner, n, 0, &path.display().to_string());
        });
    }
}

fn write_reply(out: &Path, owner: u32, n: u64, status: u8, text: &str) {
    let tmp = out.join(format!("{n}.tmp"));
    if let Some(mut f) = own_file(&tmp, owner) {
        let _ = write!(f, "{status}\n{}\n", text.replace('\n', " "));
        drop(f);
        let _ = fs::rename(&tmp, out.join(format!("{n}.res")));
    }
}

/// A new file of the user's in the agent directory: they can read it,
/// nobody else, and only the agent can replace it.
fn own_file(path: &Path, owner: u32) -> Option<fs::File> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .ok()?;
    std::os::unix::fs::fchown(&file, Some(owner), None).ok()?;
    Some(file)
}

/// An existing network interface, by a plain name.
fn is_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !name.starts_with('.')
        && Path::new("/sys/class/net").join(name).exists()
}

/// The VLAN interface on `parent` with `tag`, if there is one
/// (`/proc/net/vlan/config` lists them once the 8021q module is loaded).
fn existing_vlan(parent: &str, tag: u16) -> Option<String> {
    let text = fs::read_to_string("/proc/net/vlan/config").ok()?;
    text.lines().skip(2).find_map(|line| {
        let f: Vec<&str> = line.split('|').map(str::trim).collect();
        (f.len() == 3 && f[1] == tag.to_string() && f[2] == parent).then(|| f[0].to_string())
    })
}

fn mac_of(iface: &str) -> Option<[u8; 6]> {
    let text = fs::read_to_string(format!("/sys/class/net/{iface}/address")).ok()?;
    parse_mac(text.trim())
}

pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let bytes: Vec<u8> = text
        .split(':')
        .map(|b| u8::from_str_radix(b, 16))
        .collect::<Result<_, _>>()
        .ok()?;
    bytes.try_into().ok()
}

fn ip(args: &[&str]) -> Result<(), String> {
    let out = Command::new("ip")
        .args(args)
        .env("PATH", PATH)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn remove_link(name: &str) {
    let _ = ip(&["link", "del", name]);
}

// -- Packet sockets ------------------------------------------------------------

/// `tpacket_auxdata.tp_status`: the frame had a VLAN tag, taken off by the
/// network card (`tp_vlan_tci`).
const TP_STATUS_VLAN_VALID: u32 = 1 << 4;

/// A frame received: its length and whether this machine sent it.
pub struct Frame {
    pub len: usize,
    pub outgoing: bool,
    /// The VLAN tag the card took off the frame, if it did.
    pub vlan: Option<u16>,
}

/// A raw packet socket on one interface, every protocol, in promiscuous
/// mode (as tcpdump would be), with the VLAN tags the card strips.
pub struct PacketSocket {
    fd: libc::c_int,
}

impl PacketSocket {
    pub fn open(iface: &str, promiscuous: bool) -> Result<Self, String> {
        let index = if_index(iface)?;
        let proto = (libc::ETH_P_ALL as u16).to_be() as libc::c_int;
        // SAFETY: plain socket call.
        let fd = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW, proto) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let socket = Self { fd };
        // SAFETY: a zeroed sockaddr_ll filled with valid fields.
        let mut addr: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        addr.sll_family = libc::AF_PACKET as u16;
        addr.sll_protocol = (libc::ETH_P_ALL as u16).to_be();
        addr.sll_ifindex = index;
        // SAFETY: the address and its size.
        let bound = unsafe {
            libc::bind(
                fd,
                (&addr as *const libc::sockaddr_ll).cast(),
                std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if bound != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        socket.set_int(libc::SOL_PACKET, libc::PACKET_AUXDATA, 1);
        if promiscuous {
            // SAFETY: a zeroed packet_mreq filled with valid fields.
            let mut mreq: libc::packet_mreq = unsafe { std::mem::zeroed() };
            mreq.mr_ifindex = index;
            mreq.mr_type = libc::PACKET_MR_PROMISC as u16;
            // SAFETY: the option value and its size.
            unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_PACKET,
                    libc::PACKET_ADD_MEMBERSHIP,
                    (&mreq as *const libc::packet_mreq).cast(),
                    std::mem::size_of::<libc::packet_mreq>() as libc::socklen_t,
                );
            }
        }
        // Wake up twice a second to check the time and the stop flag.
        let timeout = libc::timeval {
            tv_sec: 0,
            tv_usec: 500_000,
        };
        // SAFETY: the option value and its size.
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&timeout as *const libc::timeval).cast(),
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }
        Ok(socket)
    }

    fn set_int(&self, level: libc::c_int, name: libc::c_int, value: libc::c_int) {
        // SAFETY: a c_int option value and its size.
        unsafe {
            libc::setsockopt(
                self.fd,
                level,
                name,
                (&value as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
    }

    /// The next frame into `buf`, or `None` after half a second without.
    pub fn receive(&self, buf: &mut [u8]) -> Option<Frame> {
        // SAFETY: zeroed C structs, filled by recvmsg within their sizes.
        unsafe {
            let mut from: libc::sockaddr_ll = std::mem::zeroed();
            let mut iov = libc::iovec {
                iov_base: buf.as_mut_ptr().cast(),
                iov_len: buf.len(),
            };
            let mut control = [0u8; 256];
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_name = (&mut from as *mut libc::sockaddr_ll).cast();
            msg.msg_namelen = std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = control.len() as _;
            let len = libc::recvmsg(self.fd, &mut msg, 0);
            if len < 0 {
                return None;
            }
            let mut vlan = None;
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_PACKET
                    && (*cmsg).cmsg_type == libc::PACKET_AUXDATA
                {
                    let aux: libc::tpacket_auxdata =
                        std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast());
                    if aux.tp_status & TP_STATUS_VLAN_VALID != 0 {
                        vlan = Some(aux.tp_vlan_tci & 0x0fff);
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
            Some(Frame {
                len: (len as usize).min(buf.len()),
                outgoing: from.sll_pkttype == libc::PACKET_OUTGOING,
                vlan,
            })
        }
    }
}

impl Drop for PacketSocket {
    fn drop(&mut self) {
        // SAFETY: our own descriptor, closed once.
        unsafe {
            libc::close(self.fd);
        }
    }
}

pub fn if_index(iface: &str) -> Result<libc::c_int, String> {
    let name = CString::new(iface).map_err(|e| e.to_string())?;
    // SAFETY: a NUL-terminated name.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        Err(format!("{iface}: interfaccia inesistente"))
    } else {
        Ok(index as libc::c_int)
    }
}

/// Listen until `wait` is over or `stop` is set, writing what the tagged
/// frames from other devices tell (see the module docs), then "end".
fn summarize_vlans(
    socket: PacketSocket,
    own: [u8; 6],
    mut file: fs::File,
    wait: Duration,
    stop: &AtomicBool,
) {
    let deadline = Instant::now() + wait;
    let mut summary = VlanSummary::default();
    let mut buf = vec![0u8; 9216];
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        let Some(frame) = socket.receive(&mut buf) else {
            continue;
        };
        if frame.outgoing {
            continue;
        }
        let lines = summary.add(&buf[..frame.len], frame.vlan, &own);
        if !lines.is_empty() {
            let _ = file.write_all(lines.as_bytes());
        }
    }
    let _ = file.write_all(b"end\n");
}

/// The counts behind the lines written so far.
#[derive(Default)]
pub struct VlanSummary {
    frames: HashMap<u16, u64>,
    macs: HashSet<(u16, [u8; 6])>,
    nets: HashMap<(u16, [u8; 3]), u64>,
}

impl VlanSummary {
    /// Take in one frame (`vlan`: the tag the card took off, if it did); the
    /// lines it adds. Untagged frames and this machine's own are skipped.
    pub fn add(&mut self, frame: &[u8], vlan: Option<u16>, own: &[u8; 6]) -> String {
        if frame.len() < 14 || frame[6..12] == own[..] {
            return String::new();
        }
        let mut src = [0u8; 6];
        src.copy_from_slice(&frame[6..12]);
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        let (tag, inner, payload) = match vlan {
            Some(tag) => (tag, ethertype, &frame[14..]),
            None if matches!(ethertype, 0x8100 | 0x88a8) && frame.len() >= 18 => (
                u16::from_be_bytes([frame[14], frame[15]]) & 0x0fff,
                u16::from_be_bytes([frame[16], frame[17]]),
                &frame[18..],
            ),
            None => return String::new(),
        };
        if !(1..=4094).contains(&tag) {
            return String::new();
        }
        let mut out = String::new();
        let f = self.frames.entry(tag).or_default();
        *f += 1;
        if is_milestone(*f) {
            out += &format!("frames {tag} {f}\n");
        }
        if self.macs.insert((tag, src)) {
            out += &format!("mac {tag} {}\n", format_mac(&src));
        }
        let ip = match inner {
            0x0800 if payload.len() >= 20 => Some([payload[12], payload[13], payload[14], payload[15]]),
            // ARP: the sender's address.
            0x0806 if payload.len() >= 18 => Some([payload[14], payload[15], payload[16], payload[17]]),
            _ => None,
        };
        if let Some(ip) = ip.filter(is_unicast) {
            let c = self.nets.entry((tag, [ip[0], ip[1], ip[2]])).or_default();
            *c += 1;
            if is_milestone(*c) {
                out += &format!("net {tag} {}.{}.{} {c}\n", ip[0], ip[1], ip[2]);
            }
        }
        out
    }
}

fn is_milestone(n: u64) -> bool {
    matches!(n, 1 | 10 | 100 | 1000 | 10000 | 100000)
}

/// An address a device would have: not 0.x, loopback, link-local or
/// multicast and above.
fn is_unicast(ip: &[u8; 4]) -> bool {
    !(ip[0] == 0 || ip[0] == 127 || ip[0] >= 224 || (ip[0] == 169 && ip[1] == 254))
}

pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// An LLDP frame (ethertype 0x88cc) or a CDP one (to 01:00:0c:cc:cc:cc).
fn is_neighbor_frame(frame: &[u8]) -> bool {
    frame.len() >= 14
        && (frame[0..6] == [0x01, 0x00, 0x0c, 0xcc, 0xcc, 0xcc]
            || u16::from_be_bytes([frame[12], frame[13]]) == 0x88cc)
}

/// A classic pcap file header, Ethernet frames.
fn pcap_header() -> Vec<u8> {
    let mut h = Vec::with_capacity(24);
    h.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&0i32.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&65535u32.to_le_bytes());
    h.extend_from_slice(&1u32.to_le_bytes());
    h
}

fn pcap_record(frame: &[u8]) -> Vec<u8> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut r = Vec::with_capacity(16 + frame.len());
    r.extend_from_slice(&(now.as_secs() as u32).to_le_bytes());
    r.extend_from_slice(&now.subsec_micros().to_le_bytes());
    r.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    r.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    r.extend_from_slice(frame);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(src: u8, tagged: Option<u16>, ethertype: u16, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![0xff; 6];
        f.extend_from_slice(&[2, 0, 0, 0, 0, src]);
        if let Some(tag) = tagged {
            f.extend_from_slice(&0x8100u16.to_be_bytes());
            f.extend_from_slice(&tag.to_be_bytes());
        }
        f.extend_from_slice(&ethertype.to_be_bytes());
        f.extend_from_slice(payload);
        f
    }

    #[test]
    fn summarizes_tagged_frames() {
        let own = [2, 0, 0, 0, 0, 9];
        let mut s = VlanSummary::default();
        let mut ipv4 = vec![0u8; 20];
        ipv4[12..16].copy_from_slice(&[172, 20, 20, 7]);
        // Tag still in the frame.
        let out = s.add(&frame(1, Some(69), 0x0800, &ipv4), None, &own);
        assert_eq!(out, "frames 69 1\nmac 69 02:00:00:00:00:01\nnet 69 172.20.20 1\n");
        // Tag taken off by the card.
        let out = s.add(&frame(1, None, 0x0800, &ipv4), Some(69), &own);
        assert_eq!(out, "");
        // ARP sender.
        let mut arp = vec![0u8; 28];
        arp[14..18].copy_from_slice(&[10, 0, 2, 1]);
        let out = s.add(&frame(3, None, 0x0806, &arp), Some(2), &own);
        assert_eq!(out, "frames 2 1\nmac 2 02:00:00:00:00:03\nnet 2 10.0.2 1\n");
        // Untagged, own, tag 0.
        assert_eq!(s.add(&frame(4, None, 0x0800, &ipv4), None, &own), "");
        assert_eq!(s.add(&frame(9, Some(5), 0x0800, &ipv4), None, &own), "");
        assert_eq!(s.add(&frame(4, Some(0), 0x0800, &ipv4), None, &own), "");
    }

    #[test]
    fn neighbor_frames_and_pcap() {
        fn tlv(t: u16, v: &[u8]) -> Vec<u8> {
            let mut out = ((t << 9) | v.len() as u16).to_be_bytes().to_vec();
            out.extend_from_slice(v);
            out
        }
        let mut body = tlv(1, &[4, 2, 0, 0, 0, 0, 1]);
        body.extend(tlv(2, b"\x0719"));
        body.extend(tlv(3, &120u16.to_be_bytes()));
        body.extend(tlv(5, b"TEST-SW"));
        body.extend(tlv(0, b""));
        let mut lldp = vec![0x01, 0x80, 0xc2, 0x00, 0x00, 0x0e, 2, 0, 0, 0, 0, 1, 0x88, 0xcc];
        lldp.extend(body);
        assert!(is_neighbor_frame(&lldp));
        assert!(!is_neighbor_frame(&frame(1, None, 0x0800, &[0; 10])));
        let mut pcap = pcap_header();
        pcap.extend(pcap_record(&lldp));
        let n = netscout_core::parse_link_neighbor_pcap(pcap).expect("an LLDP announcement");
        assert_eq!(n.system_name.as_deref(), Some("TEST-SW"));
        assert_eq!(n.port.as_deref(), Some("19"));
        // Only the header: nothing announced.
        assert!(netscout_core::parse_link_neighbor_pcap(pcap_header()).is_none());
    }
}
