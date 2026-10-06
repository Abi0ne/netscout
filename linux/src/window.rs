//! The main window: two pages, the scan (sidebar with target, profile,
//! start/stop, progress and device types; host table with search; device
//! card) and the saved profiles, over a status bar.
//!
//! State lives on the GTK main thread. Engine callbacks arrive on runtime
//! threads; [`Bridge`] forwards them through a channel that a main-loop future
//! drains. Host updates are coalesced and applied at most every 150 ms, like
//! the macOS app, so a burst of events does not rebuild the table hundreds of
//! times.

mod compare;
mod connect;
mod profiles;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use netscout_core::{
    new_scanner, open_profile_store, DeviceType, Host, NetworkInfo, ProfileStore, ProfileSummary,
    Progress, ScanConfig, ScanObserver, ScanProfile, Scanner, Summary,
};

use crate::table::{self, FilterState, Row, Table};
use crate::{debug_log, detail, labels};

/// How long host updates are held before being applied together.
const FLUSH_DELAY: Duration = Duration::from_millis(150);
/// How often the networks are re-read (catches changes no signal reports).
const NETWORK_POLL: Duration = Duration::from_secs(10);
/// How long the "network changed" notice stays in the status bar.
const NOTICE_TIME: Duration = Duration::from_secs(10);

/// One engine callback, as a value.
enum Event {
    Host(Box<Host>),
    Progress(Progress),
    Finished(Summary),
    Error(String),
}

/// An event tagged with the scan it belongs to. `deep` is the IP of a
/// single-host deep scan; `None` for the main scan.
struct Msg {
    generation: u64,
    deep: Option<String>,
    event: Event,
}

/// The engine's `ScanObserver`, forwarding every callback to the main loop.
/// The channel is FIFO, so events keep the order the engine sent.
struct Bridge {
    tx: async_channel::Sender<Msg>,
    generation: u64,
    deep: Option<String>,
}

impl Bridge {
    fn send(&self, event: Event) {
        let _ = self.tx.try_send(Msg {
            generation: self.generation,
            deep: self.deep.clone(),
            event,
        });
    }
}

impl ScanObserver for Bridge {
    fn on_host(&self, host: Host) {
        self.send(Event::Host(Box::new(host)));
    }
    fn on_progress(&self, progress: Progress) {
        self.send(Event::Progress(progress));
    }
    fn on_finished(&self, summary: Summary) {
        self.send(Event::Finished(summary));
    }
    fn on_error(&self, message: String) {
        self.send(Event::Error(message));
    }
}

struct Widgets {
    stack: adw::ViewStack,
    net_dropdown: gtk::DropDown,
    net_model: gtk::StringList,
    target: gtk::Entry,
    profile: adw::ToggleGroup,
    scan_button: gtk::Button,
    progress_box: gtk::Box,
    progress_bar: gtk::ProgressBar,
    progress_title: gtk::Label,
    progress_detail: gtk::Label,
    result_group: gtk::Box,
    compare_menu: gtk::MenuButton,
    types_group: gtk::Box,
    types_list: gtk::ListBox,
    content_stack: gtk::Stack,
    no_results: adw::StatusPage,
    detail_split: adw::OverlaySplitView,
    detail_bin: adw::Bin,
    detail_toggle: gtk::ToggleButton,
    sidebar_toggle: gtk::ToggleButton,
    notice: gtk::Label,
    search: gtk::SearchEntry,
    profiles: profiles::ProfilesWidgets,
}

pub struct Window {
    pub window: adw::ApplicationWindow,
    scanner: Option<Arc<Scanner>>,
    profile_store: Option<Arc<ProfileStore>>,
    tx: async_channel::Sender<Msg>,
    table: Table,
    filter_state: Rc<RefCell<FilterState>>,
    w: Widgets,

    networks: RefCell<Vec<NetworkInfo>>,
    generation: Cell<u64>,
    scanning: Cell<bool>,
    /// The last scan ran to the end (it can be saved or compared).
    finished: Cell<bool>,
    /// Target and depth of the last scan started (what a saved profile records).
    scanned_target: RefCell<String>,
    scanned_profile: Cell<ScanProfile>,
    /// IPs with a deep single-host scan in flight.
    deep_scanning: RefCell<HashSet<String>>,
    pending: RefCell<HashMap<String, Host>>,
    flush_scheduled: Cell<bool>,
    /// Saved profiles, newest first.
    profiles: RefCell<Vec<ProfileSummary>>,
    /// Set while the window itself moves the target/dropdown, so their change
    /// handlers don't echo the update back.
    syncing_target: Cell<bool>,
    /// Set while rows are replaced, so the selection churn doesn't rebuild
    /// the device card on every step.
    quiet_selection: Cell<bool>,
    notice_serial: Cell<u64>,
}

impl Window {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let (tx, rx) = async_channel::unbounded::<Msg>();
        let filter_state = Rc::new(RefCell::new(FilterState::default()));
        let table = table::build(filter_state.clone());

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("NetScout")
            .icon_name(crate::APP_ID)
            .default_width(1360)
            .default_height(800)
            .width_request(360)
            .height_request(400)
            .build();

        let (sidebar, sw) = build_sidebar();
        let (content, cw) = build_content(&table);
        let detail_bin = adw::Bin::new();
        detail_bin.set_child(Some(&detail::empty()));

        let detail_split = adw::OverlaySplitView::builder()
            .content(&content)
            .sidebar(&detail_bin)
            .sidebar_position(gtk::PackType::End)
            .min_sidebar_width(280.0)
            .max_sidebar_width(420.0)
            .sidebar_width_fraction(0.22)
            .build();
        let main_split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&detail_split)
            .min_sidebar_width(316.0)
            .max_sidebar_width(340.0)
            .sidebar_width_fraction(0.23)
            .build();

        let pw = profiles::build();
        let stack = adw::ViewStack::new();
        stack.add_titled_with_icon(
            &main_split,
            Some("scan"),
            "Scansione",
            "network-transmit-receive-symbolic",
        );
        stack.add_titled_with_icon(
            &pw.root,
            Some("profiles"),
            "Profili salvati",
            "document-save-symbolic",
        );

        let search = gtk::SearchEntry::builder()
            .placeholder_text("IP, nome, produttore, MAC")
            .width_chars(24)
            .build();
        let header = adw::HeaderBar::new();
        let switcher = adw::ViewSwitcher::builder()
            .stack(&stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        header.set_title_widget(Some(&switcher));
        let sidebar_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Mostra o nascondi la barra laterale")
            .build();
        main_split
            .bind_property("show-sidebar", &sidebar_toggle, "active")
            .bidirectional()
            .sync_create()
            .build();
        header.pack_start(&sidebar_toggle);
        let detail_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text("Mostra o nascondi la scheda del dispositivo")
            .build();
        detail_split
            .bind_property("show-sidebar", &detail_toggle, "active")
            .bidirectional()
            .sync_create()
            .build();
        header.pack_end(&main_menu_button());
        header.pack_end(&detail_toggle);
        header.pack_end(&search);

        let notice = gtk::Label::new(None);
        notice.add_css_class("accent");
        notice.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let status = build_status_bar(&notice);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_bottom_bar(&status);
        toolbar.set_content(Some(&stack));
        window.set_content(Some(&toolbar));

        // Narrow windows: the device card, then the sidebars, become overlays.
        for (width, splits) in [
            (1000, vec![detail_split.clone()]),
            (680, vec![main_split.clone(), pw.split.clone()]),
        ] {
            let bp = adw::Breakpoint::new(
                adw::BreakpointCondition::parse(&format!("max-width: {width}sp"))
                    .expect("valid breakpoint"),
            );
            for split in &splits {
                bp.add_setter(split, "collapsed", Some(&true.to_value()));
            }
            window.add_breakpoint(bp);
        }

        let w = Widgets {
            stack,
            net_dropdown: sw.net_dropdown,
            net_model: sw.net_model,
            target: sw.target,
            profile: sw.profile,
            scan_button: sw.scan_button,
            progress_box: sw.progress_box,
            progress_bar: sw.progress_bar,
            progress_title: sw.progress_title,
            progress_detail: sw.progress_detail,
            result_group: sw.result_group,
            compare_menu: sw.compare_menu,
            types_group: sw.types_group,
            types_list: sw.types_list,
            content_stack: cw.stack,
            no_results: cw.no_results,
            detail_split,
            detail_bin,
            detail_toggle,
            sidebar_toggle,
            notice,
            search,
            profiles: pw,
        };

        let profiles_dir = glib::user_data_dir().join("NetScout").join("Profiles");
        let profile_store = open_profile_store(profiles_dir.to_string_lossy().into_owned());
        let this = Rc::new(Self {
            window,
            scanner: new_scanner().ok(),
            profile_store: profile_store.as_ref().ok().cloned(),
            tx,
            table,
            filter_state,
            w,
            networks: RefCell::new(Vec::new()),
            generation: Cell::new(0),
            scanning: Cell::new(false),
            finished: Cell::new(false),
            scanned_target: RefCell::new(String::new()),
            scanned_profile: Cell::new(ScanProfile::Standard),
            deep_scanning: RefCell::new(HashSet::new()),
            pending: RefCell::new(HashMap::new()),
            flush_scheduled: Cell::new(false),
            profiles: RefCell::new(Vec::new()),
            syncing_target: Cell::new(false),
            quiet_selection: Cell::new(false),
            notice_serial: Cell::new(0),
        });

        this.connect_signals();
        this.connect_profile_signals();
        this.install_actions(app);

        let weak = Rc::downgrade(&this);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let Some(this) = weak.upgrade() else { break };
                this.handle(msg);
            }
        });

        if this.scanner.is_none() {
            this.show_error("Impossibile avviare il motore di scansione.");
        } else {
            this.detect_networks();
            this.watch_networks();
        }
        if let Err(e) = profile_store {
            this.show_error(&format!("Impossibile aprire i profili salvati: {e}"));
        }
        this.refresh_profiles();
        this.update_scan_ui();
        this
    }

    // -----------------------------------------------------------------------
    // Wiring
    // -----------------------------------------------------------------------

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.w
            .net_dropdown
            .connect_selected_notify(with(&weak, |this, dd: &gtk::DropDown| {
                if this.syncing_target.get() {
                    return;
                }
                let i = dd.selected() as usize;
                if let Some(net) = this.networks.borrow().get(i) {
                    this.set_target(&net.cidr);
                }
            }));
        self.w
            .target
            .connect_changed(with(&weak, |this, _: &gtk::Entry| this.sync_dropdown()));
        self.w
            .target
            .connect_activate(with(&weak, |this, _: &gtk::Entry| this.start_scan()));
        self.w
            .scan_button
            .connect_clicked(with(&weak, |this, _: &gtk::Button| this.toggle_scan()));

        self.w
            .search
            .connect_search_changed(with(&weak, |this, e: &gtk::SearchEntry| {
                this.filter_state.borrow_mut().query = e.text().to_string();
                this.refilter();
            }));
        let weak2 = weak.clone();
        self.w.types_list.connect_row_activated(move |_, row| {
            let Some(this) = weak2.upgrade() else { return };
            let picked = type_of_row(row);
            let current = this.filter_state.borrow().device_type;
            this.filter_state.borrow_mut().device_type = if picked.is_some() && picked == current {
                None
            } else {
                picked
            };
            this.refilter();
            this.rebuild_types();
        });
        self.table.selection.connect_selected_item_notify(with(
            &weak,
            |this, _: &gtk::SingleSelection| {
                if !this.quiet_selection.get() {
                    this.show_detail();
                }
            },
        ));
        self.table
            .view
            .connect_activate(with2(&weak, |this, _, _: u32| {
                this.w.detail_split.set_show_sidebar(true);
            }));
        // The search and the device card belong to the scan page.
        self.w.stack.connect_visible_child_name_notify(with(
            &weak,
            |this, stack: &adw::ViewStack| {
                let scan = stack.visible_child_name().as_deref() == Some("scan");
                this.w.search.set_visible(scan);
                this.w.detail_toggle.set_visible(scan);
                this.w.sidebar_toggle.set_visible(scan);
                if !scan {
                    this.refresh_profiles();
                }
            },
        ));
    }

    fn install_actions(self: &Rc<Self>, app: &adw::Application) {
        let weak = Rc::downgrade(self);
        type Handler = Box<dyn Fn(&Rc<Window>)>;
        let add = |name: &str, accels: &[&str], f: Handler| {
            let action = gio::SimpleAction::new(name, None);
            let weak = weak.clone();
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    f(&this);
                }
            });
            self.window.add_action(&action);
            if !accels.is_empty() {
                app.set_accels_for_action(&format!("win.{name}"), accels);
            }
        };
        add("scan", &["<Ctrl>r"], Box::new(|this| this.toggle_scan()));
        add(
            "find",
            &["<Ctrl>f"],
            Box::new(|this| {
                this.w.stack.set_visible_child_name("scan");
                this.w.search.grab_focus();
            }),
        );
        add(
            "save-profile",
            &["<Ctrl>s"],
            Box::new(|this| this.ask_save_profile()),
        );
        add(
            "help",
            &["F1"],
            Box::new(|this| crate::help::show(&this.window)),
        );
        add(
            "about",
            &[],
            Box::new(|this| crate::help::show_about(&this.window)),
        );

        let compare = gio::SimpleAction::new("compare", Some(glib::VariantTy::STRING));
        let weak2 = weak.clone();
        compare.connect_activate(move |_, id| {
            if let (Some(this), Some(id)) = (weak2.upgrade(), id.and_then(|v| v.str())) {
                this.compare_with(id);
            }
        });
        self.window.add_action(&compare);
    }

    // -----------------------------------------------------------------------
    // Networks and target
    // -----------------------------------------------------------------------

    fn detect_networks(&self) {
        let Some(scanner) = &self.scanner else { return };
        match scanner.clone().detect_networks() {
            Ok(nets) => {
                debug_log(&format!("networks: {nets:?}"));
                *self.networks.borrow_mut() = nets;
                self.rebuild_network_model();
                if self.target().is_empty() {
                    if let Some(net) = self.preferred_network() {
                        self.set_target(&net.cidr);
                    }
                }
            }
            Err(e) => self.show_error(&format!("Nessuna rete trovata: {e}")),
        }
    }

    /// The network to scan by default: the one with a gateway, else the first.
    fn preferred_network(&self) -> Option<NetworkInfo> {
        let nets = self.networks.borrow();
        nets.iter()
            .find(|n| n.gateway.is_some())
            .or(nets.first())
            .cloned()
    }

    /// Follow interface and address changes while the app runs: GIO reports
    /// connectivity changes (re-read now and again shortly, as DHCP may still
    /// be assigning the address), and a light poll catches the rest.
    fn watch_networks(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        gio::NetworkMonitor::default().connect_network_changed(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.refresh_networks();
            }
            let weak = weak.clone();
            glib::timeout_add_local_once(Duration::from_secs(3), move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_networks();
                }
            });
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(NETWORK_POLL, move || match weak.upgrade() {
            Some(this) => {
                this.refresh_networks();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }

    /// Re-read the networks; when they changed, update the list and, if the
    /// target was a network that no longer exists (or none), move it to the
    /// current one. A target the user typed is left alone.
    fn refresh_networks(self: &Rc<Self>) {
        let Some(scanner) = &self.scanner else { return };
        let current = scanner.clone().detect_networks().unwrap_or_default();
        let key = |nets: &[NetworkInfo]| -> Vec<String> {
            nets.iter()
                .map(|n| {
                    format!(
                        "{} {} {} {:?} {:?}",
                        n.interface, n.ipv4, n.cidr, n.gateway, n.dns
                    )
                })
                .collect()
        };
        if key(&current) == key(&self.networks.borrow()) {
            return;
        }
        let target = self.target();
        let old = std::mem::replace(&mut *self.networks.borrow_mut(), current.clone());
        debug_log(&format!("networks changed: {current:?}"));
        self.rebuild_network_model();

        let target_was_detected = old.iter().any(|n| n.cidr == target);
        let target_gone = !current.iter().any(|n| n.cidr == target);
        if target.is_empty() || (target_was_detected && target_gone) {
            if let Some(net) = self.preferred_network() {
                self.set_target(&net.cidr);
            }
        }
        let summary = if current.is_empty() {
            "nessuna rete attiva".to_string()
        } else {
            current
                .iter()
                .map(|n| format!("{} {}", n.interface, n.ipv4))
                .collect::<Vec<_>>()
                .join(", ")
        };
        self.show_notice(&format!("Rete cambiata: {summary}"));
    }

    fn rebuild_network_model(&self) {
        self.syncing_target.set(true);
        let labels: Vec<String> = self
            .networks
            .borrow()
            .iter()
            .map(|n| format!("{} · {}", n.interface, n.cidr))
            .chain(std::iter::once("Personalizzata".to_string()))
            .collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let n = self.w.net_model.n_items();
        self.w.net_model.splice(0, n, &refs);
        self.w
            .net_dropdown
            .set_visible(!self.networks.borrow().is_empty());
        self.syncing_target.set(false);
        self.sync_dropdown();
    }

    /// Point the dropdown at the network matching the target, or "custom".
    fn sync_dropdown(&self) {
        let target = self.target();
        let nets = self.networks.borrow();
        let i = nets
            .iter()
            .position(|n| n.cidr == target)
            .unwrap_or(nets.len());
        self.syncing_target.set(true);
        self.w.net_dropdown.set_selected(i as u32);
        self.syncing_target.set(false);
    }

    fn target(&self) -> String {
        self.w.target.text().trim().to_string()
    }

    fn set_target(&self, cidr: &str) {
        self.w.target.set_text(cidr);
    }

    fn profile(&self) -> ScanProfile {
        let active = self.w.profile.active_name();
        labels::PROFILES
            .into_iter()
            .find(|p| active.as_deref() == Some(labels::profile_name(*p)))
            .unwrap_or(ScanProfile::Standard)
    }

    // -----------------------------------------------------------------------
    // Scanning
    // -----------------------------------------------------------------------

    fn toggle_scan(self: &Rc<Self>) {
        if self.scanning.get() {
            self.cancel();
        } else {
            self.start_scan();
        }
    }

    pub fn start_scan(self: &Rc<Self>) {
        let Some(scanner) = &self.scanner else { return };
        if self.scanning.get() {
            return;
        }
        let target = self.target();
        if target.is_empty() {
            self.show_error("Indica cosa scansionare (es. 192.168.1.0/24).");
            return;
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.pending.borrow_mut().clear();
        self.table.store.remove_all();
        self.deep_scanning.borrow_mut().clear();
        self.filter_state.borrow_mut().device_type = None;
        self.finished.set(false);
        *self.scanned_target.borrow_mut() = target.clone();
        self.scanned_profile.set(self.profile());

        let observer = Bridge {
            tx: self.tx.clone(),
            generation,
            deep: None,
        };
        let profile = self.profile();
        let config = ScanConfig {
            targets: vec![target.clone()],
            profile,
            concurrency: 64,
            per_host_concurrency: 8,
            timeout_ms: 0,
        };
        match scanner.clone().start_scan(config, Box::new(observer)) {
            Ok(()) => {
                debug_log(&format!("scan started: {target} {profile:?}"));
                self.scanning.set(true);
                self.update_progress(None);
            }
            Err(e) => self.show_error(&e.to_string()),
        }
        self.update_scan_ui();
        self.rebuild_types();
        self.show_detail();
    }

    fn cancel(self: &Rc<Self>) {
        if let Some(scanner) = &self.scanner {
            let _ = scanner.clone().cancel();
        }
        self.generation.set(self.generation.get() + 1);
        self.pending.borrow_mut().clear();
        self.scanning.set(false);
        self.deep_scanning.borrow_mut().clear();
        self.w.progress_box.set_visible(false);
        self.update_scan_ui();
        self.show_detail();
    }

    /// Re-scan one host with the deep profile; its row is updated in place.
    fn deep_scan(self: &Rc<Self>, ip: &str) {
        let Some(scanner) = &self.scanner else { return };
        if !self.deep_scanning.borrow_mut().insert(ip.to_string()) {
            return;
        }
        let observer = Bridge {
            tx: self.tx.clone(),
            generation: self.generation.get(),
            deep: Some(ip.to_string()),
        };
        if let Err(e) = scanner
            .clone()
            .scan_host(ip.to_string(), Box::new(observer))
        {
            self.deep_scanning.borrow_mut().remove(ip);
            self.show_error(&e.to_string());
        }
        self.show_detail();
    }

    fn handle(self: &Rc<Self>, msg: Msg) {
        if msg.generation != self.generation.get() {
            return;
        }
        match (msg.deep, msg.event) {
            (Some(_), Event::Host(host)) => self.apply_hosts(vec![*host]),
            (Some(ip), Event::Finished(_)) => {
                self.deep_scanning.borrow_mut().remove(&ip);
                self.show_detail();
            }
            (Some(_), Event::Progress(_)) => {}
            (None, Event::Host(host)) => self.enqueue(*host),
            (None, Event::Progress(p)) => self.update_progress(Some(&p)),
            (None, Event::Finished(summary)) => {
                self.flush();
                self.scanning.set(false);
                self.finished.set(true);
                self.finish_progress(&summary);
                self.update_scan_ui();
            }
            (_, Event::Error(message)) => self.show_error(&message),
        }
    }

    fn enqueue(self: &Rc<Self>, host: Host) {
        self.pending.borrow_mut().insert(host.ip.clone(), host);
        if self.flush_scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(FLUSH_DELAY, move || {
            if let Some(this) = weak.upgrade() {
                this.flush();
            }
        });
    }

    fn flush(self: &Rc<Self>) {
        self.flush_scheduled.set(false);
        let pending: Vec<Host> = self.pending.borrow_mut().drain().map(|(_, h)| h).collect();
        if !pending.is_empty() {
            self.apply_hosts(pending);
        }
    }

    /// Insert or update live hosts in the table. An updated row is replaced
    /// by a new item (GTK does not re-bind an item it already shows), and the
    /// selection is put back on it afterwards. A device that turns up is no
    /// longer listed as off.
    fn apply_hosts(self: &Rc<Self>, hosts: Vec<Host>) {
        let store = &self.table.store;
        let selected = self.selected_id();
        let mut selected_changed = false;
        self.quiet_selection.set(true);

        let found_macs: HashSet<String> = hosts
            .iter()
            .filter_map(|h| h.mac.as_deref().map(str::to_lowercase))
            .collect();
        let mut i = 0;
        while i < store.n_items() {
            let drop = store.item(i).is_some_and(|obj| {
                let row = table::row_of(&obj);
                row.offline
                    && row
                        .host
                        .mac
                        .as_deref()
                        .is_some_and(|m| found_macs.contains(&m.to_lowercase()))
            });
            if drop {
                store.remove(i);
            } else {
                i += 1;
            }
        }

        let mut index: HashMap<String, u32> = HashMap::new();
        for i in 0..store.n_items() {
            if let Some(obj) = store.item(i) {
                let row = table::row_of(&obj);
                if !row.offline {
                    index.insert(row.host.ip.clone(), i);
                }
            }
        }
        for host in hosts {
            selected_changed |= selected.as_deref() == Some(host.ip.as_str());
            let ip = host.ip.clone();
            let item = glib::BoxedAnyObject::new(Row {
                host,
                offline: false,
            });
            match index.get(&ip) {
                Some(&i) => store.splice(i, 1, &[item]),
                None => {
                    index.insert(ip, store.n_items());
                    store.append(&item);
                }
            }
        }
        if selected_changed {
            if let Some(id) = &selected {
                self.select_id(id);
            }
        }
        self.quiet_selection.set(false);
        self.after_rows_changed();
        if selected_changed || self.selected_id() != selected {
            self.show_detail();
        }
    }

    /// Select the row with `id` (if it is visible).
    fn select_id(&self, id: &str) {
        let sel = &self.table.selection;
        let pos = (0..sel.n_items()).find(|&i| {
            sel.item(i)
                .is_some_and(|obj| table::row_of(&obj).id() == id)
        });
        if let Some(pos) = pos {
            sel.set_selected(pos);
        }
    }

    fn after_rows_changed(&self) {
        self.rebuild_types();
        self.update_content_page();
        self.update_result_section();
    }

    // -----------------------------------------------------------------------
    // Devices that are off
    // -----------------------------------------------------------------------

    /// Live hosts in the table, by IP.
    fn live_hosts(&self) -> Vec<Host> {
        self.rows(false)
    }

    /// Devices that are off in the table, by IP.
    fn offline_hosts(&self) -> Vec<Host> {
        self.rows(true)
    }

    fn rows(&self, offline: bool) -> Vec<Host> {
        let store = &self.table.store;
        let mut out: Vec<Host> = (0..store.n_items())
            .filter_map(|i| store.item(i))
            .filter_map(|obj| {
                let row = table::row_of(&obj);
                (row.offline == offline).then(|| row.host.clone())
            })
            .collect();
        out.sort_by_key(|h| labels::ip_value(&h.ip));
        out
    }

    /// Add devices that are off to this scan (skipping any found up or
    /// already listed).
    fn add_offline(&self, devices: &[Host]) {
        let live = self.live_hosts();
        let up_macs: HashSet<String> = live
            .iter()
            .filter_map(|h| h.mac.as_deref().map(str::to_lowercase))
            .collect();
        let up_ips: HashSet<&str> = live.iter().map(|h| h.ip.as_str()).collect();
        let mut listed: HashSet<String> =
            self.offline_hosts().iter().map(table::offline_id).collect();
        for device in devices {
            match device.mac.as_deref() {
                Some(mac) if up_macs.contains(&mac.to_lowercase()) => continue,
                None if up_ips.contains(device.ip.as_str()) => continue,
                _ => {}
            }
            if !listed.insert(table::offline_id(device)) {
                continue;
            }
            self.table.store.append(&glib::BoxedAnyObject::new(Row {
                host: device.clone(),
                offline: true,
            }));
        }
        self.after_rows_changed();
    }

    fn forget_offline(self: &Rc<Self>, id: &str) {
        let store = &self.table.store;
        let pos = (0..store.n_items()).find(|&i| {
            store.item(i).is_some_and(|obj| {
                let row = table::row_of(&obj);
                row.offline && row.id() == id
            })
        });
        if let Some(pos) = pos {
            store.remove(pos);
        }
        self.after_rows_changed();
        self.show_detail();
    }

    /// Send a Wake-on-LAN packet to `host`.
    fn wake(&self, host: &Host) -> Result<(), String> {
        let (Some(scanner), Some(mac)) = (&self.scanner, &host.mac) else {
            return Err("MAC sconosciuto: impossibile inviare Wake-on-LAN.".into());
        };
        scanner
            .clone()
            .wake_on_lan(mac.clone(), Some(host.ip.clone()))
            .map_err(|e| format!("Wake-on-LAN non inviato: {e}"))
    }

    // -----------------------------------------------------------------------
    // UI updates
    // -----------------------------------------------------------------------

    /// A finished scan with results can be saved or compared.
    fn has_finished_scan(&self) -> bool {
        self.finished.get() && !self.scanning.get() && !self.live_hosts().is_empty()
    }

    fn update_scan_ui(&self) {
        let running = self.scanning.get();
        let b = &self.w.scan_button;
        b.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name(if running {
                    "media-playback-stop-symbolic"
                } else {
                    "media-playback-start-symbolic"
                })
                .label(if running { "Ferma" } else { "Avvia scansione" })
                .build(),
        ));
        if running {
            b.remove_css_class("suggested-action");
            b.add_css_class("destructive-action");
        } else {
            b.remove_css_class("destructive-action");
            b.add_css_class("suggested-action");
        }
        b.set_sensitive(self.scanner.is_some());
        self.w.target.set_sensitive(!running);
        self.w.net_dropdown.set_sensitive(!running);
        self.w.profile.set_sensitive(!running);
        self.update_content_page();
        self.update_result_section();
        self.update_profile_buttons();
    }

    fn update_result_section(&self) {
        let finished = self.has_finished_scan();
        self.w.result_group.set_visible(finished);
        if let Some(a) = self
            .window
            .lookup_action("save-profile")
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_enabled(finished);
        }
    }

    fn update_progress(&self, p: Option<&Progress>) {
        let w = &self.w;
        w.progress_box.set_visible(true);
        w.progress_bar.set_visible(true);
        match p {
            Some(p) => {
                w.progress_bar
                    .set_fraction(p.scanned_hosts as f64 / p.total_hosts.max(1) as f64);
                w.progress_title.set_text(labels::phase_label(p.phase));
                w.progress_detail.set_text(&progress_line(
                    p.discovered_hosts,
                    p.total_hosts,
                    p.open_ports_found,
                    p.elapsed_ms,
                ));
            }
            None => {
                w.progress_bar.set_fraction(0.0);
                w.progress_title.set_text("Avvio…");
                w.progress_detail.set_text("");
            }
        }
    }

    fn finish_progress(&self, s: &Summary) {
        let w = &self.w;
        w.progress_box.set_visible(true);
        w.progress_bar.set_visible(false);
        w.progress_title.set_text("Completata");
        w.progress_detail.set_text(&progress_line(
            s.discovered_hosts,
            s.total_hosts,
            s.total_open_ports,
            s.elapsed_ms,
        ));
    }

    /// Which page the table area shows: empty state, spinner, no matches, or
    /// the table.
    fn update_content_page(&self) {
        let any = self.table.store.n_items() > 0;
        let visible = self.table.filtered.n_items() > 0;
        let page = match (any, visible, self.scanning.get()) {
            (false, _, true) => "scanning",
            (false, _, false) => "empty",
            (true, false, _) => "no-results",
            (true, true, _) => "table",
        };
        if page == "no-results" {
            let q = self.filter_state.borrow().query.trim().to_string();
            self.w.no_results.set_description(Some(&if q.is_empty() {
                "Nessun dispositivo di questo tipo.".to_string()
            } else {
                format!("Nessun dispositivo corrisponde a “{q}”.")
            }));
        }
        self.w.content_stack.set_visible_child_name(page);
    }

    fn refilter(&self) {
        self.table.filter.changed(gtk::FilterChange::Different);
        self.update_content_page();
    }

    /// The per-type breakdown: "Tutti" plus each type found, most common first.
    fn rebuild_types(&self) {
        let list = &self.w.types_list;
        list.remove_all();
        let live = self.live_hosts();
        let mut counts: Vec<(DeviceType, u32)> = Vec::new();
        for host in &live {
            match counts.iter_mut().find(|(t, _)| *t == host.device_type) {
                Some((_, n)) => *n += 1,
                None => counts.push((host.device_type, 1)),
            }
        }
        self.w.types_group.set_visible(!live.is_empty());
        counts.sort_by_key(|(t, n)| {
            let order = labels::ALL_TYPES
                .iter()
                .position(|x| x == t)
                .unwrap_or(usize::MAX);
            (std::cmp::Reverse(*n), order)
        });
        let current = self.filter_state.borrow().device_type;
        let all = type_row("Tutti", "view-grid-symbolic", live.len() as u32, None);
        list.append(&all);
        if current.is_none() {
            list.select_row(Some(&all));
        }
        for (t, n) in counts {
            let row = type_row(labels::device_label(t), labels::device_icon(t), n, Some(t));
            list.append(&row);
            if current == Some(t) {
                list.select_row(Some(&row));
            }
        }
    }

    fn selected_row(&self) -> Option<(Host, bool)> {
        let obj = self.table.selection.selected_item()?;
        let row = table::row_of(&obj);
        Some((row.host.clone(), row.offline))
    }

    fn selected_id(&self) -> Option<String> {
        let obj = self.table.selection.selected_item()?;
        let id = table::row_of(&obj).id();
        Some(id)
    }

    fn show_detail(self: &Rc<Self>) {
        let child = match self.selected_row() {
            None => detail::empty(),
            Some((host, offline)) => {
                let deep = self.deep_scanning.borrow().contains(&host.ip);
                detail::build(&host, offline, deep, self.detail_actions())
            }
        };
        self.w.detail_bin.set_child(Some(&child));
    }

    fn detail_actions(self: &Rc<Self>) -> detail::Actions {
        let weak = Rc::downgrade(self);
        let call = move |f: fn(&Rc<Window>, &str)| {
            let weak = weak.clone();
            Box::new(move |s: &str| {
                if let Some(this) = weak.upgrade() {
                    f(&this, s);
                }
            }) as Box<dyn Fn(&str)>
        };
        let weak = Rc::downgrade(self);
        let weak2 = weak.clone();
        detail::Actions {
            deep_scan: call(|this, ip| this.deep_scan(ip)),
            open_url: call(|this, url| this.open_url(url)),
            forget: call(|this, id| this.forget_offline(id)),
            wake: Rc::new(move |host| match weak.upgrade() {
                Some(this) => this.wake(host),
                None => Err("La finestra è stata chiusa.".into()),
            }),
            connect: Box::new(move |host, kind, port| {
                if let Some(this) = weak2.upgrade() {
                    this.connect_remote(host, kind, port);
                }
            }),
        }
    }

    fn open_url(&self, url: &str) {
        gtk::UriLauncher::new(url).launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
    }

    fn show_notice(self: &Rc<Self>, text: &str) {
        let serial = self.notice_serial.get() + 1;
        self.notice_serial.set(serial);
        self.w.notice.set_text(text);
        self.w.notice.set_visible(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(NOTICE_TIME, move || {
            if let Some(this) = weak.upgrade() {
                if this.notice_serial.get() == serial {
                    this.w.notice.set_visible(false);
                }
            }
        });
    }

    pub fn show_error(&self, message: &str) {
        debug_log(&format!("error: {message}"));
        let dialog = adw::AlertDialog::new(Some("Errore"), Some(message));
        dialog.add_response("ok", "OK");
        dialog.present(Some(&self.window));
    }
}

// ---------------------------------------------------------------------------
// Widget construction
// ---------------------------------------------------------------------------

struct SidebarWidgets {
    net_dropdown: gtk::DropDown,
    net_model: gtk::StringList,
    target: gtk::Entry,
    profile: adw::ToggleGroup,
    scan_button: gtk::Button,
    progress_box: gtk::Box,
    progress_bar: gtk::ProgressBar,
    progress_title: gtk::Label,
    progress_detail: gtk::Label,
    result_group: gtk::Box,
    compare_menu: gtk::MenuButton,
    types_group: gtk::Box,
    types_list: gtk::ListBox,
}

fn build_sidebar() -> (gtk::Widget, SidebarWidgets) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 18);
    root.set_margin_top(18);
    root.set_margin_bottom(18);
    root.set_margin_start(14);
    root.set_margin_end(14);

    let net_model = gtk::StringList::new(&[]);
    let net_dropdown = gtk::DropDown::new(Some(net_model.clone()), None::<gtk::Expression>);
    let target = gtk::Entry::builder()
        .placeholder_text("192.168.1.0/24")
        .build();
    target.set_tooltip_text(Some(
        "Una rete (CIDR), un IP, un intervallo (192.168.1.10-50) o un elenco",
    ));
    let profile = adw::ToggleGroup::builder().homogeneous(false).build();
    for p in labels::PROFILES {
        profile.add(
            adw::Toggle::builder()
                .label(labels::profile_label(p))
                .name(labels::profile_name(p))
                .build(),
        );
    }
    profile.set_active_name(Some(labels::profile_name(ScanProfile::Standard)));
    let net_box = section("Rete");
    net_box.append(&net_dropdown);
    net_box.append(&target);
    net_box.append(&profile);
    root.append(&net_box);

    let scan_button = gtk::Button::new();
    scan_button.add_css_class("pill");
    scan_button.set_tooltip_text(Some("Avvia o ferma la scansione (Ctrl+R)"));
    root.append(&scan_button);

    let progress_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let progress_bar = gtk::ProgressBar::new();
    let progress_title = gtk::Label::new(None);
    progress_title.set_xalign(0.0);
    progress_title.add_css_class("caption-heading");
    let progress_detail = gtk::Label::new(None);
    progress_detail.set_xalign(0.0);
    progress_detail.set_wrap(true);
    progress_detail.add_css_class("caption");
    progress_detail.add_css_class("dim-label");
    progress_box.append(&progress_bar);
    progress_box.append(&progress_title);
    progress_box.append(&progress_detail);
    progress_box.set_visible(false);
    root.append(&progress_box);

    let result_group = section("Risultato");
    let save = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("document-save-symbolic")
                .label("Salva come profilo…")
                .build(),
        )
        .action_name("win.save-profile")
        .tooltip_text(
            "Salva i dispositivi trovati per confrontarli con le prossime scansioni (Ctrl+S)",
        )
        .build();
    let compare_menu = gtk::MenuButton::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("view-dual-symbolic")
                .label("Confronta con profilo")
                .build(),
        )
        .tooltip_text("Trova le differenze rispetto a un profilo salvato")
        .build();
    result_group.append(&save);
    result_group.append(&compare_menu);
    result_group.set_visible(false);
    root.append(&result_group);

    let types_group = section("Dispositivi");
    let types_list = gtk::ListBox::new();
    types_list.add_css_class("navigation-sidebar");
    types_list.set_selection_mode(gtk::SelectionMode::Single);
    types_list.set_activate_on_single_click(true);
    types_group.append(&types_list);
    types_group.set_visible(false);
    root.append(&types_group);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_child(Some(&root));
    scroller.add_css_class("sidebar-pane");
    (
        scroller.upcast(),
        SidebarWidgets {
            net_dropdown,
            net_model,
            target,
            profile,
            scan_button,
            progress_box,
            progress_bar,
            progress_title,
            progress_detail,
            result_group,
            compare_menu,
            types_group,
            types_list,
        },
    )
}

/// A titled vertical group for the sidebar.
fn section(title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let l = gtk::Label::new(Some(title));
    l.set_xalign(0.0);
    l.add_css_class("heading");
    l.add_css_class("dim-label");
    b.append(&l);
    b
}

/// A device-type row: icon, name, count. The type is kept in the widget name
/// (`all` = "Tutti").
fn type_row(label: &str, icon: &str, count: u32, t: Option<DeviceType>) -> gtk::ListBoxRow {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    b.append(&gtk::Image::from_icon_name(icon));
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    b.append(&l);
    let n = gtk::Label::new(Some(&count.to_string()));
    n.add_css_class("dim-label");
    n.add_css_class("numeric");
    b.append(&n);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&b));
    let index = t.and_then(|t| labels::ALL_TYPES.iter().position(|x| *x == t));
    row.set_widget_name(&index.map_or("all".to_string(), |i| i.to_string()));
    row
}

fn type_of_row(row: &gtk::ListBoxRow) -> Option<DeviceType> {
    row.widget_name()
        .parse::<usize>()
        .ok()
        .and_then(|i| labels::ALL_TYPES.get(i).copied())
}

struct ContentWidgets {
    stack: gtk::Stack,
    no_results: adw::StatusPage,
}

fn build_content(table: &Table) -> (gtk::Widget, ContentWidgets) {
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);

    let empty = adw::StatusPage::builder()
        .icon_name("network-transmit-receive-symbolic")
        .title("Nessuna scansione")
        .description("Scegli la rete e premi Avvia (Ctrl+R).")
        .build();
    stack.add_named(&empty, Some("empty"));

    let spinner = adw::StatusPage::builder()
        .paintable(&adw::SpinnerPaintable::new(None::<&gtk::Widget>))
        .title("Scansione in corso…")
        .build();
    stack.add_named(&spinner, Some("scanning"));

    let no_results = adw::StatusPage::builder()
        .icon_name("edit-find-symbolic")
        .title("Nessun risultato")
        .build();
    stack.add_named(&no_results, Some("no-results"));

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&table.view));
    scroller.set_vexpand(true);
    stack.add_named(&scroller, Some("table"));
    stack.set_visible_child_name("empty");

    (stack.clone().upcast(), ContentWidgets { stack, no_results })
}

fn build_status_bar(notice: &gtk::Label) -> gtk::Widget {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    bar.add_css_class("status-bar");
    let version = gtk::Label::new(Some(&format!("NetScout {}", netscout_core::VERSION)));
    version.add_css_class("caption-heading");
    version.add_css_class("dim-label");
    version.set_selectable(true);
    bar.append(&version);
    notice.add_css_class("caption");
    notice.set_visible(false);
    bar.append(notice);
    bar.upcast()
}

fn main_menu_button() -> gtk::MenuButton {
    let menu = gio::Menu::new();
    let section = gio::Menu::new();
    section.append(Some("Guida di NetScout"), Some("win.help"));
    section.append(Some("Informazioni su NetScout"), Some("win.about"));
    menu.append_section(None, &section);
    let quit = gio::Menu::new();
    quit.append(Some("Esci"), Some("app.quit"));
    menu.append_section(None, &quit);
    gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Menu principale")
        .build()
}

fn progress_line(found: u32, total: u32, ports: u32, elapsed_ms: u64) -> String {
    format!(
        "{found} attivi su {total} · {ports} porte aperte · {:.1} s",
        elapsed_ms as f64 / 1000.0
    )
}

/// A signal handler that upgrades `weak` and forwards to `f`.
fn with<W: 'static>(
    weak: &Weak<Window>,
    f: impl Fn(&Rc<Window>, &W) + 'static,
) -> impl Fn(&W) + 'static {
    let weak = weak.clone();
    move |w| {
        if let Some(this) = weak.upgrade() {
            f(&this, w);
        }
    }
}

/// Two-argument form of [`with`].
fn with2<W: 'static, A>(
    weak: &Weak<Window>,
    f: impl Fn(&Rc<Window>, &W, A) + 'static,
) -> impl Fn(&W, A) + 'static {
    let weak = weak.clone();
    move |w, a| {
        if let Some(this) = weak.upgrade() {
            f(&this, w, a);
        }
    }
}
