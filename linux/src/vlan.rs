//! A tagged VLAN that reaches the computer on the same cable as the network
//! in access, scanned along with the scan's target, and the switch at the
//! other end of that cable (the macOS app's `TaggedVLAN` and
//! `LinkedSwitch`).
//!
//! The VLAN is offered when the interface chosen for the scan is a wired
//! Ethernet port, and rides on that port. Turning the option on listens to
//! the port for a while: the tags of the frames other devices send there
//! fill a menu, with the network and the devices seen in each VLAN (any tag
//! can still be written by hand).
//!
//! The computer only sees a tagged VLAN through a VLAN interface of its own:
//! NetScout creates one on the port, gives it an address (from the VLAN's
//! DHCP, or the one written here) and from then on scans the VLAN's network
//! like any other. The interface stays up after the scan, so a deep scan or
//! a terminal session to a device of the VLAN still works, until the option
//! is turned off, its settings change or the app quits.
//!
//! Listening and creating interfaces both need an administrator: they go
//! through the root helper (`agent`), started with the password the first
//! time either is needed and kept until NetScout quits, so the password is
//! asked once per session and nothing is left behind (even after a crash).
//! The system's network settings are never touched, and a restart clears
//! the interface anyway.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{gio, glib};
use netscout_core::{parse_link_neighbor_pcap, LinkNeighbor, NetworkInfo};

use crate::debug_log;
use crate::segment::Ipv4Cidr;

/// How long a listening for VLANs lasts.
pub const LISTEN_SECONDS: u64 = 30;
/// LLDP every 30 s, CDP every 60 s, and some margin.
pub const SWITCH_SECONDS: u64 = 65;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VlanError {
    Invalid(String),
    Cancelled,
    Overlap(String, String),
    Failed(String),
}

impl VlanError {
    pub fn message(&self) -> String {
        match self {
            VlanError::Invalid(m) => m.clone(),
            VlanError::Cancelled => {
                "VLAN non attivata: password di amministratore non inserita.".into()
            }
            VlanError::Overlap(net, other) => format!(
                "La rete della VLAN, {net}, si sovrappone a {other}: è già raggiungibile senza VLAN."
            ),
            VlanError::Failed(m) => format!("Impossibile attivare la VLAN: {m}"),
        }
    }

    /// The message without "Impossibile attivare la VLAN".
    pub fn detail(&self) -> String {
        match self {
            VlanError::Failed(m) => m.clone(),
            other => other.message(),
        }
    }
}

// ---------------------------------------------------------------------------
// The root helper, from the app's side
// ---------------------------------------------------------------------------

/// The running root helper (see `agent` for its commands).
pub struct Agent {
    commands: PathBuf,
    results: PathBuf,
    next: Cell<u64>,
}

impl Agent {
    /// Start the agent, asking for the administrator password (pkexec).
    async fn start() -> Result<Rc<Agent>, VlanError> {
        let commands = glib::tmp_dir().join(format!(
            "netscout-agent-{}",
            glib::uuid_string_random()
        ));
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&commands)
                .map_err(|e| VlanError::Failed(e.to_string()))?;
        }
        let fail = |message: String| {
            let _ = std::fs::remove_dir_all(&commands);
            Err(VlanError::Failed(message))
        };
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => return fail(e.to_string()),
        };
        let pid = std::process::id().to_string();
        let argv = [
            std::ffi::OsStr::new("pkexec"),
            exe.as_os_str(),
            std::ffi::OsStr::new("--agent"),
            commands.as_os_str(),
            std::ffi::OsStr::new(&pid),
        ];
        let process = match gio::Subprocess::newv(
            &argv,
            gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE,
        ) {
            Ok(p) => p,
            Err(e) => {
                return fail(format!(
                    "pkexec non disponibile ({e}): installa il pacchetto pkexec"
                ))
            }
        };
        let (stdout, stderr) = match process.communicate_utf8_future(None).await {
            Ok(out) => out,
            Err(e) => return fail(e.to_string()),
        };
        let stdout = stdout.map(|s| s.trim().to_string()).unwrap_or_default();
        let stderr = stderr.map(|s| s.trim().to_string()).unwrap_or_default();
        match process.exit_status() {
            0 => {}
            // The authentication dialog was dismissed.
            126 => {
                let _ = std::fs::remove_dir_all(&commands);
                return Err(VlanError::Cancelled);
            }
            127 => return fail("autorizzazione negata".into()),
            code => return fail(if stderr.is_empty() { format!("codice {code}") } else { stderr }),
        }
        if !stdout.starts_with('/') || !Path::new(&stdout).is_dir() {
            return fail(format!("risposta inattesa: {stdout}"));
        }
        debug_log(&format!("vlan agent up: {stdout}"));
        Ok(Rc::new(Agent {
            commands,
            results: PathBuf::from(stdout),
            next: Cell::new(1),
        }))
    }

    /// Still running: its directory goes with it.
    pub fn is_alive(&self) -> bool {
        self.results.is_dir()
    }

    /// Send `words` as one command; the answer's text, or what went wrong.
    pub async fn send(&self, words: &[&str], timeout: Duration) -> Result<String, VlanError> {
        let n = self.next.get();
        self.next.set(n + 1);
        let pending = self.commands.join(format!("{n}.tmp"));
        std::fs::write(&pending, words.join(" ") + "\n")
            .and_then(|()| std::fs::rename(&pending, self.commands.join(format!("{n}.cmd"))))
            .map_err(|e| VlanError::Failed(e.to_string()))?;
        let answer = self.results.join(format!("{n}.res"));
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(&answer) {
                let _ = std::fs::remove_file(&answer);
                let mut lines = text.lines();
                let status = lines.next().unwrap_or_default();
                let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
                return if status == "0" {
                    Ok(body)
                } else {
                    Err(VlanError::Failed(body))
                };
            }
            if !self.is_alive() {
                return Err(VlanError::Failed(
                    "l'aiuto con privilegi di NetScout si è chiuso".into(),
                ));
            }
            glib::timeout_future(Duration::from_millis(100)).await;
        }
        Err(VlanError::Failed(
            "nessuna risposta dall'aiuto con privilegi di NetScout".into(),
        ))
    }

    /// Let the agent go: within a moment it removes the interfaces it made
    /// and exits.
    pub fn stop(&self) {
        let _ = std::fs::remove_dir_all(&self.commands);
    }
}

// ---------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------

fn sys(iface: &str) -> PathBuf {
    Path::new("/sys/class/net").join(iface)
}

pub fn is_wifi(iface: &str) -> bool {
    sys(iface).join("wireless").exists() || sys(iface).join("phy80211").exists()
}

/// A wired Ethernet port of this computer: a real device (not a bridge,
/// tunnel or VLAN interface), not Wi-Fi. These can carry tagged VLANs.
pub fn is_ethernet_port(iface: &str) -> bool {
    let dir = sys(iface);
    std::fs::read_to_string(dir.join("type")).is_ok_and(|t| t.trim() == "1")
        && dir.join("device").exists()
        && !is_wifi(iface)
        && !Path::new("/proc/net/vlan").join(iface).exists()
}

/// The ports of the computer that can carry VLANs, among `networks`' and
/// every other one present.
fn ethernet_ports() -> HashSet<String> {
    std::fs::read_dir("/sys/class/net")
        .map(|dir| {
            dir.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| is_ethernet_port(name))
                .collect()
        })
        .unwrap_or_default()
}

/// The VLAN interfaces NetScout makes (`vlanN`, or `nsvlanN` when taken).
pub fn is_own_vlan_interface(iface: &str) -> bool {
    iface.starts_with("vlan") || iface.starts_with("nsvlan")
}

// ---------------------------------------------------------------------------
// The tagged VLAN
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Addressing {
    Dhcp,
    Manual,
}

/// What the VLAN interface is made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub tag: u16,
    pub parent: String,
    pub addressing: Addressing,
    /// Address and prefix, for manual addressing.
    pub address: Option<Ipv4Cidr>,
}

/// The VLAN interface up and its network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Active {
    pub settings: Settings,
    pub interface: String,
    pub ipv4: String,
    pub cidr: Ipv4Cidr,
}

/// A VLAN heard on the cable: its tag, the devices that sent frames tagged
/// with it and the networks of their addresses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveredVlan {
    pub tag: u16,
    /// Source MACs.
    pub devices: HashSet<String>,
    /// First three octets ("172.20.20") → frames seen, at least.
    pub networks: HashMap<String, u64>,
}

impl DiscoveredVlan {
    /// The network heard most, as its first three octets.
    pub fn main_network(&self) -> Option<&str> {
        self.networks
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(k, _)| k.as_str())
    }

    /// "VLAN 69 · 172.20.20.x · 4 dispositivi".
    pub fn label(&self) -> String {
        let count = if self.devices.len() == 1 {
            "1 dispositivo".to_string()
        } else {
            format!("{} dispositivi", self.devices.len())
        };
        let mut parts = vec![format!("VLAN {}", self.tag)];
        if let Some(net) = self.main_network() {
            parts.push(format!("{net}.x"));
        }
        parts.push(count);
        parts.join(" · ")
    }

    /// The agent's summary lines ("mac TAG MAC", "net TAG A.B.C COUNT"),
    /// by tag.
    pub fn parse(text: &str) -> Vec<DiscoveredVlan> {
        let mut by_tag: HashMap<u16, DiscoveredVlan> = HashMap::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            let Some(tag) = f
                .get(1)
                .and_then(|t| t.parse::<u16>().ok())
                .filter(|t| (1..=4094).contains(t))
            else {
                continue;
            };
            if f.len() < 3 {
                continue;
            }
            let vlan = by_tag.entry(tag).or_insert_with(|| DiscoveredVlan {
                tag,
                ..Default::default()
            });
            match f[0] {
                "mac" => {
                    vlan.devices.insert(f[2].to_string());
                }
                "net" if f.len() >= 4 => {
                    let count = f[3].parse().unwrap_or(0);
                    let e = vlan.networks.entry(f[2].to_string()).or_default();
                    *e = (*e).max(count);
                }
                _ => {}
            }
        }
        let mut list: Vec<DiscoveredVlan> = by_tag.into_values().collect();
        list.sort_by_key(|v| v.tag);
        list
    }
}

/// The VLAN option's state. Changes are announced through `on_change`.
pub struct TaggedVlan {
    /// Not remembered between launches: every session starts with the
    /// option off and empty.
    enabled: Cell<bool>,
    /// The VLAN ID as written (1–4094).
    tag: RefCell<String>,
    /// The tag is written by hand ("Altro…" in the menu) rather than picked
    /// among the VLANs heard on the cable.
    typing_tag: Cell<bool>,
    /// The port the VLAN rides on: the scan's interface, when it can carry
    /// VLANs; `None` hides the option.
    parent: RefCell<Option<String>>,
    addressing: Cell<Addressing>,
    /// The manual address with its prefix, e.g. "172.20.20.250/24".
    address: RefCell<String>,

    active: RefCell<Option<Active>>,
    preparing: Cell<bool>,
    /// The ports of the computer that can carry VLANs.
    capable: RefCell<HashSet<String>>,

    discovered: RefCell<Vec<DiscoveredVlan>>,
    discovering: Cell<bool>,
    /// Why the listening found nothing, if it did.
    discovery_problem: RefCell<Option<String>>,
    /// Bumped to drop a listening in progress.
    discovery_serial: Cell<u64>,

    agent: RefCell<Option<Rc<Agent>>>,
    agent_starting: Cell<bool>,
    agent_error: RefCell<Option<VlanError>>,

    on_change: RefCell<Option<Rc<dyn Fn()>>>,
}

impl TaggedVlan {
    pub fn new() -> Rc<Self> {
        // What earlier sessions left in the temporary folder: the commands
        // directory of an agent long gone.
        if let Ok(dir) = std::fs::read_dir(glib::tmp_dir()) {
            for entry in dir.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("netscout-agent-")
                {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        Rc::new(Self {
            enabled: Cell::new(false),
            tag: RefCell::new(String::new()),
            typing_tag: Cell::new(false),
            parent: RefCell::new(None),
            addressing: Cell::new(Addressing::Dhcp),
            address: RefCell::new(String::new()),
            active: RefCell::new(None),
            preparing: Cell::new(false),
            capable: RefCell::new(HashSet::new()),
            discovered: RefCell::new(Vec::new()),
            discovering: Cell::new(false),
            discovery_problem: RefCell::new(None),
            discovery_serial: Cell::new(0),
            agent: RefCell::new(None),
            agent_starting: Cell::new(false),
            agent_error: RefCell::new(None),
            on_change: RefCell::new(None),
        })
    }

    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Rc::new(f));
    }

    fn changed(&self) {
        let f = self.on_change.borrow().clone();
        if let Some(f) = f {
            f();
        }
    }

    // -- Reading ---------------------------------------------------------------

    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }
    pub fn tag(&self) -> String {
        self.tag.borrow().clone()
    }
    pub fn typing_tag(&self) -> bool {
        self.typing_tag.get()
    }
    pub fn parent(&self) -> Option<String> {
        self.parent.borrow().clone()
    }
    pub fn addressing(&self) -> Addressing {
        self.addressing.get()
    }
    pub fn address(&self) -> String {
        self.address.borrow().clone()
    }
    pub fn active(&self) -> Option<Active> {
        self.active.borrow().clone()
    }
    pub fn is_preparing(&self) -> bool {
        self.preparing.get()
    }
    pub fn is_capable(&self, iface: &str) -> bool {
        self.capable.borrow().contains(iface)
    }
    pub fn discovered(&self) -> Vec<DiscoveredVlan> {
        self.discovered.borrow().clone()
    }
    pub fn is_discovering(&self) -> bool {
        self.discovering.get()
    }
    pub fn discovery_problem(&self) -> Option<String> {
        self.discovery_problem.borrow().clone()
    }

    /// The settings as written, or what is wrong with them.
    pub fn settings(&self) -> Result<Settings, VlanError> {
        let written = self.tag.borrow().trim().to_string();
        if written.is_empty() && !self.typing_tag.get() {
            return Err(VlanError::Invalid("Scegli la VLAN dal menu.".into()));
        }
        let Some(tag) = written.parse::<u16>().ok().filter(|t| (1..=4094).contains(t)) else {
            return Err(VlanError::Invalid("Il tag VLAN va da 1 a 4094.".into()));
        };
        let Some(parent) = self.parent() else {
            return Err(VlanError::Invalid(
                "L'interfaccia scelta non può ricevere VLAN taggate.".into(),
            ));
        };
        let mut settings = Settings {
            tag,
            parent,
            addressing: self.addressing.get(),
            address: None,
        };
        if settings.addressing == Addressing::Manual {
            match Ipv4Cidr::parse(&self.address.borrow()).filter(|c| c.prefix <= 30) {
                Some(c) => settings.address = Some(c),
                None => {
                    return Err(VlanError::Invalid(format!(
                        "Scrivi l'indirizzo con il prefisso, per esempio {}.",
                        self.suggested_address()
                            .unwrap_or_else(|| "172.20.20.250/24".into())
                    )))
                }
            }
        }
        Ok(settings)
    }

    /// The option is offered (the scan's interface can carry VLANs).
    pub fn is_available(&self) -> bool {
        self.parent.borrow().is_some()
    }

    /// A VLAN is scanned along with the target.
    pub fn is_on(&self) -> bool {
        self.enabled.get() && self.is_available()
    }

    pub fn problem(&self) -> Option<String> {
        if !self.is_on() {
            return None;
        }
        self.settings().err().map(|e| e.message())
    }

    /// The chosen VLAN, as heard on the cable.
    pub fn chosen(&self) -> Option<DiscoveredVlan> {
        let tag = self.tag.borrow().clone();
        self.discovered
            .borrow()
            .iter()
            .find(|v| v.tag.to_string() == tag)
            .cloned()
    }

    /// A fixed address to suggest in the chosen VLAN: .250 in the network
    /// heard most.
    pub fn suggested_address(&self) -> Option<String> {
        self.chosen()
            .and_then(|v| v.main_network().map(|n| format!("{n}.250/24")))
    }

    /// Something to reset: the option on, a tag, or a VLAN interface up.
    pub fn can_reset(&self) -> bool {
        self.enabled.get()
            || self.active.borrow().is_some()
            || !self.tag.borrow().is_empty()
            || self.addressing.get() != Addressing::Dhcp
            || !self.address.borrow().is_empty()
    }

    // -- Writing ---------------------------------------------------------------

    pub fn set_enabled(self: &Rc<Self>, enabled: bool) {
        if self.enabled.replace(enabled) == enabled {
            return;
        }
        self.settings_changed();
        if enabled {
            self.discover();
        } else {
            self.stop_discovery();
        }
        self.changed();
    }

    /// A tag picked in the menu (`None`: "Altro…", written by hand).
    pub fn choose(self: &Rc<Self>, tag: Option<String>) {
        self.typing_tag.set(tag.is_none());
        if let Some(tag) = tag {
            *self.tag.borrow_mut() = tag;
        }
        self.settings_changed();
        self.changed();
    }

    pub fn set_tag(self: &Rc<Self>, tag: &str) {
        if *self.tag.borrow() == tag {
            return;
        }
        *self.tag.borrow_mut() = tag.to_string();
        self.settings_changed();
        self.changed();
    }

    pub fn set_addressing(self: &Rc<Self>, addressing: Addressing) {
        if self.addressing.replace(addressing) == addressing {
            return;
        }
        self.settings_changed();
        self.changed();
    }

    pub fn set_address(self: &Rc<Self>, address: &str) {
        if *self.address.borrow() == address {
            return;
        }
        *self.address.borrow_mut() = address.to_string();
        self.settings_changed();
        self.changed();
    }

    /// Re-read which ports can carry VLANs; a port that no longer can (the
    /// cable or the adapter was unplugged) stops being the parent.
    pub fn refresh_capable(self: &Rc<Self>) {
        *self.capable.borrow_mut() = ethernet_ports();
        let parent = self.parent();
        let active_parent = self.active.borrow().as_ref().map(|a| a.settings.parent.clone());
        if let Some(p) = parent {
            if !self.is_capable(&p) && active_parent.as_deref() != Some(p.as_str()) {
                self.set_parent(None);
            }
        }
    }

    /// The scan's interface changed to `interface`: the VLAN rides on it if
    /// it can. A target that is no interface's network (`None`) and the
    /// VLAN's own interface leave the choice as it is.
    pub fn follow(self: &Rc<Self>, interface: Option<&str>) {
        let Some(interface) = interface else { return };
        let active = self.active.borrow().clone();
        if active.as_ref().is_some_and(|a| a.interface == interface) {
            return;
        }
        let rides = self.is_capable(interface)
            || active.is_some_and(|a| a.settings.parent == interface);
        self.set_parent(rides.then(|| interface.to_string()));
    }

    fn set_parent(self: &Rc<Self>, parent: Option<String>) {
        if *self.parent.borrow() == parent {
            return;
        }
        *self.parent.borrow_mut() = parent.clone();
        self.settings_changed();
        self.discovered.borrow_mut().clear();
        *self.discovery_problem.borrow_mut() = None;
        if self.enabled.get() && parent.is_some() {
            self.discover();
        } else {
            self.stop_discovery();
        }
        self.changed();
    }

    // -- Listening for VLANs -----------------------------------------------------

    /// Listen to the port for `LISTEN_SECONDS`, filling `discovered` as the
    /// tags come (asking for the password if the agent is not running).
    pub fn discover(self: &Rc<Self>) {
        let Some(parent) = self.parent() else { return };
        let serial = self.discovery_serial.get() + 1;
        self.discovery_serial.set(serial);
        self.discovered.borrow_mut().clear();
        *self.discovery_problem.borrow_mut() = None;
        self.discovering.set(true);
        self.changed();
        let this = self.clone();
        glib::spawn_future_local(async move {
            this.listen(&parent, serial).await;
        });
    }

    async fn listen(self: &Rc<Self>, parent: &str, serial: u64) {
        let current = |this: &Self| this.discovery_serial.get() == serial;
        let result: Result<(), VlanError> = async {
            let agent = self.agent_ready().await?;
            let seconds = LISTEN_SECONDS.to_string();
            let path = agent
                .send(&["listen", parent, &seconds], Duration::from_secs(20))
                .await?;
            debug_log(&format!("vlan listen on {parent}: {path}"));
            while current(self) {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let found = DiscoveredVlan::parse(&text);
                if *self.discovered.borrow() != found {
                    *self.discovered.borrow_mut() = found;
                    self.changed();
                }
                if text.ends_with("end\n") || !agent.is_alive() {
                    break;
                }
                glib::timeout_future(Duration::from_secs(1)).await;
            }
            let _ = std::fs::remove_file(&path);
            Ok(())
        }
        .await;
        if !current(self) {
            return;
        }
        *self.discovery_problem.borrow_mut() = match result {
            Ok(()) if self.discovered.borrow().is_empty() => Some(format!(
                "Nessuna VLAN taggata sentita su {parent} in {LISTEN_SECONDS} secondi: scegli Altro… per scrivere il tag."
            )),
            Ok(()) => None,
            Err(VlanError::Cancelled) => Some(
                "Ricerca delle VLAN annullata: password non inserita. Puoi scrivere il tag con Altro…"
                    .into(),
            ),
            Err(e) => Some(format!("Ricerca delle VLAN non riuscita: {}", e.detail())),
        };
        self.discovering.set(false);
        self.changed();
    }

    pub fn stop_discovery(self: &Rc<Self>) {
        if !self.discovering.get() {
            return;
        }
        self.discovery_serial.set(self.discovery_serial.get() + 1);
        self.discovering.set(false);
        let agent = self.agent.borrow().clone();
        if let Some(agent) = agent {
            glib::spawn_future_local(async move {
                let _ = agent.send(&["quiet"], Duration::from_secs(5)).await;
            });
        }
        self.changed();
    }

    // -- The VLAN interface --------------------------------------------------------

    /// The VLAN interface up with the current settings: the one already
    /// made, else a new one (asking for the administrator password if the
    /// agent is not running). `networks` are the other networks of the
    /// computer: a VLAN network that overlaps one of them is refused.
    pub async fn prepare(self: &Rc<Self>, networks: &[NetworkInfo]) -> Result<Active, VlanError> {
        let settings = self.settings()?;
        if let Some(active) = self.active() {
            if active.settings == settings {
                return Ok(active);
            }
        }
        self.deactivate();
        if let Some(iface) = settings.address {
            if let Some(clash) = overlap(&iface.network(), networks) {
                return Err(VlanError::Overlap(iface.network().to_string(), clash));
            }
        }
        self.preparing.set(true);
        self.changed();
        let result = self.make(&settings, networks).await;
        self.preparing.set(false);
        if let Ok(active) = &result {
            debug_log(&format!("vlan up: {active:?}"));
            *self.active.borrow_mut() = Some(active.clone());
        }
        self.changed();
        result
    }

    async fn make(&self, settings: &Settings, networks: &[NetworkInfo]) -> Result<Active, VlanError> {
        let agent = self.agent_ready().await?;
        let tag = settings.tag.to_string();
        let (mode, ip, mask) = match settings.address {
            Some(a) => ("manual", a.ip(), a.netmask()),
            None => ("dhcp", "-".into(), "-".into()),
        };
        // DHCP: up to 30 s for the lease.
        let output = agent
            .send(
                &["up", &tag, &settings.parent, mode, &ip, &mask],
                Duration::from_secs(60),
            )
            .await?;
        // "vlan69 172.20.20.250/24".
        let fields: Vec<&str> = output.split_whitespace().collect();
        let (Some(name), Some(cidr)) = (
            fields.first(),
            fields.get(1).and_then(|c| Ipv4Cidr::parse(c)),
        ) else {
            if let Some(name) = fields.first() {
                let _ = agent.send(&["down", name], Duration::from_secs(10)).await;
            }
            return Err(VlanError::Failed(format!("risposta inattesa: {output}")));
        };
        let others: Vec<NetworkInfo> = networks
            .iter()
            .filter(|n| n.interface != *name)
            .cloned()
            .collect();
        if let Some(clash) = overlap(&cidr.network(), &others) {
            let _ = agent.send(&["down", name], Duration::from_secs(10)).await;
            return Err(VlanError::Overlap(cidr.network().to_string(), clash));
        }
        Ok(Active {
            settings: settings.clone(),
            interface: name.to_string(),
            ipv4: cidr.ip(),
            cidr: cidr.network(),
        })
    }

    /// Remove the VLAN interface.
    pub fn deactivate(&self) {
        let Some(active) = self.active.borrow_mut().take() else {
            return;
        };
        debug_log(&format!("vlan down: {}", active.interface));
        let agent = self.agent.borrow().clone();
        if let Some(agent) = agent {
            glib::spawn_future_local(async move {
                let _ = agent
                    .send(&["down", &active.interface], Duration::from_secs(10))
                    .await;
            });
        }
        self.changed();
    }

    /// Back to the settings of a new session: the option off, no tag, DHCP,
    /// and the VLAN interface removed. The helper keeps running, so the
    /// password is not asked again.
    pub fn reset(self: &Rc<Self>) {
        self.enabled.set(false);
        self.stop_discovery();
        self.deactivate();
        self.tag.borrow_mut().clear();
        self.typing_tag.set(false);
        self.addressing.set(Addressing::Dhcp);
        self.address.borrow_mut().clear();
        self.discovered.borrow_mut().clear();
        *self.discovery_problem.borrow_mut() = None;
        self.changed();
    }

    /// NetScout is quitting: the agent removes what it made and exits.
    pub fn shutdown(&self) {
        if let Some(agent) = self.agent.borrow_mut().take() {
            agent.stop();
        }
        *self.active.borrow_mut() = None;
    }

    /// The running agent, else a new one (one password prompt at a time).
    pub async fn agent_ready(&self) -> Result<Rc<Agent>, VlanError> {
        let alive = self.agent.borrow().clone().filter(|a| a.is_alive());
        if let Some(agent) = alive {
            return Ok(agent);
        }
        if self.agent_starting.get() {
            while self.agent_starting.get() {
                glib::timeout_future(Duration::from_millis(100)).await;
            }
            let agent = self.agent.borrow().clone();
            return match agent {
                Some(a) => Ok(a),
                None => Err(self
                    .agent_error
                    .borrow()
                    .clone()
                    .unwrap_or(VlanError::Cancelled)),
            };
        }
        *self.agent.borrow_mut() = None;
        *self.active.borrow_mut() = None;
        self.agent_starting.set(true);
        let result = Agent::start().await;
        self.agent_starting.set(false);
        match result {
            Ok(agent) => {
                *self.agent.borrow_mut() = Some(agent.clone());
                *self.agent_error.borrow_mut() = None;
                Ok(agent)
            }
            Err(e) => {
                *self.agent_error.borrow_mut() = Some(e.clone());
                Err(e)
            }
        }
    }

    fn settings_changed(&self) {
        let Some(active) = self.active() else { return };
        if !self.is_on() || self.settings().ok().as_ref() != Some(&active.settings) {
            self.deactivate();
        }
    }
}

/// The interface of the first network in `networks` overlapping `net`.
fn overlap(net: &Ipv4Cidr, networks: &[NetworkInfo]) -> Option<String> {
    networks
        .iter()
        .find(|n| Ipv4Cidr::parse(&n.cidr).is_some_and(|o| o.overlaps(net)))
        .map(|n| format!("{} ({})", n.interface, n.cidr))
}

// ---------------------------------------------------------------------------
// The switch at the other end of the cable
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum SwitchState {
    Idle,
    Listening { port: String },
    Found { neighbor: Box<LinkNeighbor>, port: String, at: i64 },
    /// Nothing announced on the port within `SWITCH_SECONDS`.
    Silent { port: String },
    Failed(String),
}

/// The switch the computer is plugged into, from the LLDP or CDP
/// announcement it sends on the port. Hearing it needs an administrator: it
/// goes through the root helper of the tagged VLANs, so the password is
/// asked once per session for both. The answer is about one port and is
/// kept for that port only.
pub struct LinkedSwitch {
    state: RefCell<SwitchState>,
    serial: Cell<u64>,
    vlan: Rc<TaggedVlan>,
    on_change: RefCell<Option<Rc<dyn Fn()>>>,
}

impl LinkedSwitch {
    pub fn new(vlan: Rc<TaggedVlan>) -> Rc<Self> {
        Rc::new(Self {
            state: RefCell::new(SwitchState::Idle),
            serial: Cell::new(0),
            vlan,
            on_change: RefCell::new(None),
        })
    }

    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Rc::new(f));
    }

    fn set_state(&self, state: SwitchState) {
        *self.state.borrow_mut() = state;
        let f = self.on_change.borrow().clone();
        if let Some(f) = f {
            f();
        }
    }

    pub fn state(&self) -> SwitchState {
        self.state.borrow().clone()
    }

    pub fn is_listening(&self) -> bool {
        matches!(*self.state.borrow(), SwitchState::Listening { .. })
    }

    /// The port the last answer (or listening) is about.
    fn port(&self) -> Option<String> {
        match &*self.state.borrow() {
            SwitchState::Listening { port }
            | SwitchState::Found { port, .. }
            | SwitchState::Silent { port } => Some(port.clone()),
            _ => None,
        }
    }

    /// Listen on `port` for the announcement (asking for the password if
    /// the helper is not running).
    pub fn identify(self: &Rc<Self>, port: &str) {
        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        self.set_state(SwitchState::Listening {
            port: port.to_string(),
        });
        let this = self.clone();
        let port = port.to_string();
        glib::spawn_future_local(async move { this.listen(&port, serial).await });
    }

    /// Listen when there is no answer yet for `port`.
    pub fn identify_if_needed(self: &Rc<Self>, port: &str) {
        match self.state() {
            SwitchState::Idle | SwitchState::Failed(_) => self.identify(port),
            _ if self.port().as_deref() != Some(port) => self.identify(port),
            _ => {}
        }
    }

    pub fn cancel(&self) {
        self.serial.set(self.serial.get() + 1);
        self.set_state(SwitchState::Idle);
    }

    /// The scan's interface changed: an answer about another port no longer
    /// holds.
    pub fn follow(&self, port: Option<&str>) {
        match self.state() {
            SwitchState::Failed(_) => self.set_state(SwitchState::Idle),
            SwitchState::Idle => {}
            _ => {
                if self.port().as_deref() != port {
                    self.cancel();
                }
            }
        }
    }

    async fn listen(&self, port: &str, serial: u64) {
        let result: Result<Option<LinkNeighbor>, VlanError> = async {
            let agent = self.vlan.agent_ready().await?;
            let seconds = SWITCH_SECONDS.to_string();
            let path = agent
                .send(
                    &["neighbor", port, &seconds],
                    Duration::from_secs(SWITCH_SECONDS + 15),
                )
                .await?;
            let data = std::fs::read(&path).unwrap_or_default();
            let _ = std::fs::remove_file(&path);
            Ok(parse_link_neighbor_pcap(data))
        }
        .await;
        if self.serial.get() != serial {
            return;
        }
        self.set_state(match result {
            Ok(Some(neighbor)) => {
                debug_log(&format!("neighbor on {port}: {neighbor:?}"));
                SwitchState::Found {
                    neighbor: Box::new(neighbor),
                    port: port.to_string(),
                    at: glib::real_time() / 1000,
                }
            }
            Ok(None) => SwitchState::Silent {
                port: port.to_string(),
            },
            Err(VlanError::Cancelled) => {
                SwitchState::Failed("Password di amministratore non inserita.".into())
            }
            Err(e) => SwitchState::Failed(format!("Ascolto non riuscito: {}", e.detail())),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_agent_summary() {
        let text = "frames 69 1\nmac 69 aa:bb:cc:00:00:01\nnet 69 172.20.20 1\nmac 69 aa:bb:cc:00:00:02\nnet 69 172.20.20 10\nframes 2 1\nmac 2 aa:bb:cc:00:00:03\nend\n";
        let found = DiscoveredVlan::parse(text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].tag, 2);
        assert_eq!(found[0].label(), "VLAN 2 · 1 dispositivo");
        assert_eq!(found[1].label(), "VLAN 69 · 172.20.20.x · 2 dispositivi");
        assert_eq!(found[1].networks["172.20.20"], 10);
    }
}
