//! The main window, laid out like the macOS app's: one split view for both
//! tabs (only the panes' contents switch, so the header bars keep their
//! layout), the tabs as pills at the start of the middle pane, the app's
//! name in the middle of the window, and the search field above the device
//! card, which shares the sidebar's shade. The scan's sidebar lists the
//! devices by type at the top, with the scan controls pinned at the bottom
//! (on two rows when the window is short); a status bar runs below.
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
    device_key, new_scanner, open_profile_store, recognize_network, DeviceType, Host, NetworkInfo,
    NetworkMatch, ProfileStore, ProfileSummary, Progress, SavedProfile, ScanConfig, ScanObserver,
    ScanProfile, Scanner, Summary,
};

use crate::table::{self, FilterState, Row, Table};
use crate::{debug_log, detail, labels};

/// Below this sidebar height the scan controls take two rows.
const COMPACT_BELOW: i32 = 560;
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

/// The window's tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Scan,
    Profiles,
}

struct Widgets {
    tab_scan: gtk::ToggleButton,
    tab_profiles: gtk::ToggleButton,
    /// The sidebar and middle pane contents of each tab.
    sidebar_stack: gtk::Stack,
    content_pages: gtk::Stack,
    main_split: adw::OverlaySplitView,
    title: gtk::Label,
    tabs: gtk::Widget,
    /// Where the moving header widgets go (see `place_header_widgets`).
    sidebar_start: gtk::Box,
    content_start: gtk::Box,
    content_end: gtk::Box,
    card_slot: gtk::Box,
    /// The scan's search and the device card's button, together.
    trailing: gtk::Box,
    net_dropdown: gtk::DropDown,
    net_model: gtk::StringList,
    net_menu: gtk::MenuButton,
    net_label: gtk::Label,
    target: gtk::Entry,
    profile: adw::ToggleGroup,
    profile_row: gtk::Box,
    scan_button: gtk::Button,
    scan_panel: gtk::Box,
    progress_box: gtk::Box,
    progress_bar: gtk::ProgressBar,
    progress_title: gtk::Label,
    progress_detail: gtk::Label,
    recognized_group: gtk::Box,
    recognized: gtk::Button,
    result_group: gtk::Box,
    compare_menu: gtk::MenuButton,
    types_group: gtk::Box,
    types_list: gtk::ListBox,
    sidebar_hint: gtk::Label,
    notes: NotesSection,
    content_stack: gtk::Stack,
    no_results: adw::StatusPage,
    detail_split: adw::OverlaySplitView,
    detail_bin: adw::Bin,
    detail_toggle: gtk::ToggleButton,
    sidebar_toggle: gtk::ToggleButton,
    notice: gtk::Label,
    search: gtk::SearchEntry,
    /// The profiles tab's search, compare button and export menu.
    profile_search: gtk::SearchEntry,
    profile_compare: gtk::Button,
    export_menu: gtk::MenuButton,
    profiles: profiles::ProfilesWidgets,
}

/// The sidebar's "Note" section of the scan.
struct NotesSection {
    group: gtk::Box,
    title: gtk::Label,
    hint: gtk::Label,
    icon: gtk::Image,
    unsaved: gtk::Widget,
}

/// The device card's note, updated in place while it is edited.
struct CardNote {
    key: String,
    buffer: gtk::TextBuffer,
    caption: gtk::Label,
    unsaved: gtk::Widget,
    profile: String,
}

/// Where a note edit came from (the other views are brought up to date).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NoteSource {
    ScanTable,
    ProfileTable,
    Card,
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
    /// The same profiles with their devices (search, recognition, export).
    saved: RefCell<Vec<SavedProfile>>,
    /// Notes edited but not saved: profile id → all its notes by device key.
    note_drafts: RefCell<HashMap<String, HashMap<String, String>>>,
    /// The saved profile this scan's network was recognized as.
    recognized: RefCell<Option<NetworkMatch>>,
    /// The profile this scan was saved as, if it was.
    saved_scan_profile: RefCell<Option<String>>,
    /// The device card's note, while the card shows one.
    card_note: RefCell<Option<CardNote>>,
    /// Set while a note is written into a view, so it doesn't echo back.
    syncing_note: Cell<bool>,
    tab: Cell<Tab>,
    /// The device card was shown on the scan tab (it hides on the other).
    card_wanted: Cell<bool>,
    /// The scan controls are on two rows (short window).
    compact: Cell<bool>,
    /// Identifiable devices at the last recognition attempt; the next one
    /// runs only when there are more.
    recognition_basis: Cell<i64>,
    /// The recognized network was already offered for this scan.
    suggested: Cell<bool>,
    /// Compare with this profile when the scan finishes.
    pending_compare: RefCell<Option<String>>,
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
        // The notes columns reach the window once it exists.
        let weak_cell = Rc::new(RefCell::new(Weak::<Window>::new()));
        let table = table::build(
            filter_state.clone(),
            Some(Window::scan_notes_column(weak_cell.clone())),
        );

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("NetScout")
            .icon_name(crate::APP_ID)
            .default_width(1360)
            .default_height(800)
            .width_request(360)
            .height_request(400)
            .build();

        let (scan_sidebar, sw) = build_sidebar();
        let (scan_content, cw) = build_content(&table);
        let pw = profiles::build(Window::notes_column(weak_cell.clone()));

        // Each tab's sidebar and middle pane; the split views are shared.
        let sidebar_stack = gtk::Stack::new();
        sidebar_stack.add_named(&scan_sidebar, Some("scan"));
        sidebar_stack.add_named(&pw.sidebar, Some("profiles"));
        let content_pages = gtk::Stack::new();
        content_pages.add_named(&scan_content, Some("scan"));
        content_pages.add_named(&pw.content, Some("profiles"));
        for stack in [&sidebar_stack, &content_pages] {
            stack.set_transition_type(gtk::StackTransitionType::Crossfade);
            stack.set_transition_duration(120);
            stack.set_vexpand(true);
        }

        // Header bars: one per pane, so the sidebar's and the card's take
        // their pane's shade. The widgets that move with the panes (sidebar
        // toggle, search) sit in slots.
        let sidebar_start = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let sidebar_header = adw::HeaderBar::builder()
            .title_widget(&adw::Bin::new())
            .build();
        sidebar_header.pack_start(&sidebar_start);
        sidebar_header.pack_end(&main_menu_button());
        let sidebar_pane = adw::ToolbarView::new();
        sidebar_pane.add_top_bar(&sidebar_header);
        sidebar_pane.set_content(Some(&sidebar_stack));

        let (tabs, tab_scan, tab_profiles) = build_tabs();
        let content_start = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let start = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        start.append(&content_start);
        start.append(&tabs);
        let content_end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let profile_search = gtk::SearchEntry::builder()
            .placeholder_text("Profilo, rete, IP, nome, MAC, note")
            .width_chars(26)
            .build();
        let profile_compare = gtk::Button::builder()
            .icon_name("view-dual-symbolic")
            .tooltip_text("Confronta con la scansione corrente")
            .build();
        let export_menu = export_menu_button();
        for w in [
            profile_search.upcast_ref::<gtk::Widget>(),
            profile_compare.upcast_ref(),
            export_menu.upcast_ref(),
        ] {
            w.set_visible(false);
        }
        let end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        end.append(&content_end);
        end.append(&profile_search);
        end.append(&profile_compare);
        end.append(&export_menu);
        let content_header = adw::HeaderBar::builder()
            .title_widget(&adw::Bin::new())
            .build();
        content_header.pack_start(&start);
        content_header.pack_end(&end);
        let content_pane = adw::ToolbarView::new();
        content_pane.add_top_bar(&content_header);
        content_pane.set_content(Some(&content_pages));

        let detail_bin = adw::Bin::new();
        detail_bin.set_child(Some(&detail::empty()));
        let card_slot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        card_slot.set_hexpand(true);
        let card_header = adw::HeaderBar::builder().title_widget(&card_slot).build();
        let card_pane = adw::ToolbarView::new();
        card_pane.add_top_bar(&card_header);
        card_pane.set_content(Some(&detail_bin));

        let detail_split = adw::OverlaySplitView::builder()
            .content(&content_pane)
            .sidebar(&card_pane)
            .sidebar_position(gtk::PackType::End)
            .min_sidebar_width(320.0)
            .max_sidebar_width(440.0)
            .sidebar_width_fraction(0.25)
            .build();
        let main_split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar_pane)
            .content(&detail_split)
            .min_sidebar_width(290.0)
            .max_sidebar_width(360.0)
            .sidebar_width_fraction(0.21)
            .build();

        let search = gtk::SearchEntry::builder()
            .placeholder_text("IP, nome, produttore, MAC")
            .hexpand(true)
            .build();
        let detail_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text("Mostra o nascondi la scheda del dispositivo")
            .build();
        detail_split
            .bind_property("show-sidebar", &detail_toggle, "active")
            .bidirectional()
            .sync_create()
            .build();
        let trailing = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        trailing.set_hexpand(true);
        trailing.append(&search);
        trailing.append(&detail_toggle);
        let sidebar_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Mostra o nascondi la barra laterale")
            .build();
        main_split
            .bind_property("show-sidebar", &sidebar_toggle, "active")
            .bidirectional()
            .sync_create()
            .build();

        let notice = gtk::Label::new(None);
        notice.add_css_class("accent");
        notice.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let status = build_status_bar(&notice);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_bottom_bar(&status);
        toolbar.set_content(Some(&main_split));
        // The app's name in the middle of the window, over the header bars.
        let title = gtk::Label::new(Some("NetScout"));
        title.add_css_class("window-title");
        title.set_halign(gtk::Align::Center);
        title.set_valign(gtk::Align::Start);
        title.set_can_target(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&toolbar));
        overlay.add_overlay(&title);
        window.set_content(Some(&overlay));

        // Narrow windows: the device card, then the sidebar, become overlays.
        for (width, split) in [(1000, detail_split.clone()), (680, main_split.clone())] {
            let bp = adw::Breakpoint::new(
                adw::BreakpointCondition::parse(&format!("max-width: {width}sp"))
                    .expect("valid breakpoint"),
            );
            bp.add_setter(&split, "collapsed", Some(&true.to_value()));
            if width == 680 {
                bp.add_setter(&detail_split, "collapsed", Some(&true.to_value()));
            }
            window.add_breakpoint(bp);
        }

        let w = Widgets {
            tab_scan,
            tab_profiles,
            sidebar_stack,
            content_pages,
            main_split,
            title,
            tabs: tabs.upcast(),
            sidebar_start,
            content_start,
            content_end,
            card_slot,
            trailing,
            net_dropdown: sw.net_dropdown,
            net_model: sw.net_model,
            net_menu: sw.net_menu,
            net_label: sw.net_label,
            target: sw.target,
            profile: sw.profile,
            profile_row: sw.profile_row,
            scan_button: sw.scan_button,
            scan_panel: sw.scan_panel,
            progress_box: sw.progress_box,
            progress_bar: sw.progress_bar,
            progress_title: sw.progress_title,
            progress_detail: sw.progress_detail,
            recognized_group: sw.recognized_group,
            recognized: sw.recognized,
            result_group: sw.result_group,
            compare_menu: sw.compare_menu,
            types_group: sw.types_group,
            types_list: sw.types_list,
            sidebar_hint: sw.hint,
            notes: sw.notes,
            content_stack: cw.stack,
            no_results: cw.no_results,
            detail_split,
            detail_bin,
            detail_toggle,
            sidebar_toggle,
            notice,
            search,
            profile_search,
            profile_compare,
            export_menu,
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
            saved: RefCell::new(Vec::new()),
            note_drafts: RefCell::new(HashMap::new()),
            recognized: RefCell::new(None),
            saved_scan_profile: RefCell::new(None),
            card_note: RefCell::new(None),
            syncing_note: Cell::new(false),
            tab: Cell::new(Tab::Scan),
            card_wanted: Cell::new(true),
            compact: Cell::new(false),
            recognition_basis: Cell::new(-1),
            suggested: Cell::new(false),
            pending_compare: RefCell::new(None),
            syncing_target: Cell::new(false),
            quiet_selection: Cell::new(false),
            notice_serial: Cell::new(0),
        });

        *weak_cell.borrow_mut() = Rc::downgrade(&this);
        this.connect_signals();
        this.connect_profile_signals();
        this.install_actions(app);
        this.connect_notes_section();
        this.place_header_widgets();
        this.watch_layout();

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
        this.update_notes_section();
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
        // The tabs: two toggles, one always on.
        for (button, tab) in [
            (self.w.tab_scan.clone(), Tab::Scan),
            (self.w.tab_profiles.clone(), Tab::Profiles),
        ] {
            button.connect_toggled(with(&weak, move |this, b: &gtk::ToggleButton| {
                if b.is_active() {
                    this.set_tab(tab);
                } else if this.tab.get() == tab {
                    b.set_active(true);
                }
            }));
        }
        self.w
            .profile_compare
            .connect_clicked(with(&weak, |this, _: &gtk::Button| {
                if let Some(id) = this.selected_profile_id() {
                    this.compare_with(&id);
                }
            }));
        // Panes shown or hidden: the header widgets follow them.
        for split in [&self.w.main_split, &self.w.detail_split] {
            for property in ["show-sidebar", "collapsed"] {
                let weak = weak.clone();
                split.connect_notify_local(Some(property), move |_, _| {
                    if let Some(this) = weak.upgrade() {
                        this.place_header_widgets();
                    }
                });
            }
        }
        self.w.detail_split.connect_show_sidebar_notify(with(
            &weak,
            |this, split: &adw::OverlaySplitView| {
                if this.tab.get() == Tab::Scan {
                    this.card_wanted.set(split.shows_sidebar());
                }
            },
        ));
        self.w
            .recognized
            .connect_clicked(with(&weak, |this, _: &gtk::Button| {
                let id = this
                    .recognized
                    .borrow()
                    .as_ref()
                    .map(|m| m.profile_id.clone());
                if let Some(id) = id {
                    this.show_profile_page(&id);
                }
            }));
        // Notes not saved yet: ask before closing.
        let weak2 = weak.clone();
        self.window
            .connect_close_request(move |_| match weak2.upgrade() {
                Some(this) if !this.confirm_close() => glib::Propagation::Stop,
                _ => glib::Propagation::Proceed,
            });
    }

    fn on_profiles_page(&self) -> bool {
        self.tab.get() == Tab::Profiles
    }

    /// Switch tab: only the panes' contents change; the device card shows
    /// on the scan tab only (as it was left there).
    fn set_tab(self: &Rc<Self>, tab: Tab) {
        if self.tab.replace(tab) == tab {
            return;
        }
        let scan = tab == Tab::Scan;
        self.w.tab_scan.set_active(scan);
        self.w.tab_profiles.set_active(!scan);
        let name = if scan { "scan" } else { "profiles" };
        self.w.sidebar_stack.set_visible_child_name(name);
        self.w.content_pages.set_visible_child_name(name);
        self.w.detail_toggle.set_visible(scan);
        for w in [
            self.w.profile_search.upcast_ref::<gtk::Widget>(),
            self.w.profile_compare.upcast_ref(),
            self.w.export_menu.upcast_ref(),
        ] {
            w.set_visible(!scan);
        }
        if scan {
            self.w.detail_split.set_show_sidebar(self.card_wanted.get());
        } else {
            let wanted = self.card_wanted.get();
            self.w.detail_split.set_show_sidebar(false);
            self.card_wanted.set(wanted);
            self.refresh_profiles();
        }
        self.place_header_widgets();
    }

    /// Put the moving header widgets where the panes are: the sidebar's
    /// button at the start of the sidebar's header (or before the tabs when
    /// the sidebar is hidden), the scan's search and the card's button in
    /// the card's header, as wide as the card (or at the end of the middle
    /// pane's header when the card is hidden or on the other tab).
    fn place_header_widgets(&self) {
        let w = &self.w;
        let sidebar = w.main_split.shows_sidebar() && !w.main_split.is_collapsed();
        move_to(
            &w.sidebar_toggle,
            if sidebar {
                &w.sidebar_start
            } else {
                &w.content_start
            },
        );
        let card = self.tab.get() == Tab::Scan
            && w.detail_split.shows_sidebar()
            && !w.detail_split.is_collapsed();
        move_to(
            &w.trailing,
            if card { &w.card_slot } else { &w.content_end },
        );
        w.trailing.set_hexpand(card);
        w.search.set_width_request(if card { -1 } else { 240 });
        w.trailing.set_visible(self.tab.get() == Tab::Scan);
    }

    /// Every frame: hide the centred title while the header's controls would
    /// cover it, and put the scan controls on two rows when the sidebar is
    /// short.
    fn watch_layout(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.add_tick_callback(move |_, _| {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            this.update_title_visibility();
            let height = this.w.sidebar_stack.height();
            if height > 0 {
                let compact = height < COMPACT_BELOW;
                if compact != this.compact.get() {
                    this.set_compact(compact);
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn update_title_visibility(&self) {
        let w = &self.w;
        let root = self.window.upcast_ref::<gtk::Widget>();
        let Some(title) = w.title.compute_bounds(root) else {
            return;
        };
        let (left, right) = (title.x() - 12.0, title.x() + title.width() + 12.0);
        let covered = [
            w.tabs.clone(),
            w.content_start.clone().upcast(),
            w.trailing.clone().upcast(),
            w.profile_search.clone().upcast(),
        ]
        .iter()
        .filter(|widget| widget.is_drawable())
        .filter_map(|widget| widget.compute_bounds(root))
        .any(|b| b.width() > 0.0 && b.x() < right && b.x() + b.width() > left);
        w.title.set_opacity(if covered { 0.0 } else { 1.0 });
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
                if this.on_profiles_page() {
                    this.w.profile_search.grab_focus();
                } else {
                    this.w.search.grab_focus();
                }
            }),
        );
        add(
            "save-profile",
            &[],
            Box::new(|this| this.ask_save_profile()),
        );
        // Ctrl+S: the notes not saved yet (the profile's on the profiles
        // tab, the scan's on the scan tab), else save the scan as a profile.
        add(
            "save",
            &["<Ctrl>s"],
            Box::new(|this| {
                if this.on_profiles_page() {
                    this.save_selected_notes();
                } else if this.scan_notes_unsaved() {
                    this.save_scan_notes();
                } else {
                    this.ask_save_profile();
                }
            }),
        );
        add(
            "tab-scan",
            &["<Ctrl>1"],
            Box::new(|this| this.set_tab(Tab::Scan)),
        );
        add(
            "tab-profiles",
            &["<Ctrl>2"],
            Box::new(|this| this.set_tab(Tab::Profiles)),
        );
        add(
            "preferences",
            &["<Ctrl>comma"],
            Box::new(|this| crate::settings::show(&this.window)),
        );
        add(
            "export-selected",
            &[],
            Box::new(|this| this.export_selected()),
        );
        add("export-listed", &[], Box::new(|this| this.export_listed()));
        add("export-all", &[], Box::new(|this| this.export_all()));
        add(
            "rename-profile",
            &[],
            Box::new(|this| this.ask_rename_profile()),
        );
        add(
            "delete-profile",
            &[],
            Box::new(|this| this.ask_delete_profile()),
        );

        let pick = gio::SimpleAction::new("pick-network", Some(glib::VariantTy::UINT32));
        let weak3 = weak.clone();
        pick.connect_activate(move |_, i| {
            let (Some(this), Some(i)) = (weak3.upgrade(), i.and_then(|v| v.get::<u32>())) else {
                return;
            };
            let cidr = this
                .networks
                .borrow()
                .get(i as usize)
                .map(|n| n.cidr.clone());
            if let Some(cidr) = cidr {
                this.set_target(&cidr);
            }
        });
        self.window.add_action(&pick);
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
        let has_networks = !self.networks.borrow().is_empty();
        self.w
            .net_dropdown
            .set_visible(has_networks && !self.compact.get());
        self.w
            .net_menu
            .set_visible(has_networks && self.compact.get());
        self.rebuild_network_menu();
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
        *self.recognized.borrow_mut() = None;
        self.recognition_basis.set(-1);
        self.suggested.set(false);
        *self.pending_compare.borrow_mut() = None;
        *self.saved_scan_profile.borrow_mut() = None;
        self.update_recognized();
        self.scan_notes_changed();
        *self.scanned_target.borrow_mut() = target.clone();
        self.scanned_profile.set(self.profile());

        let observer = Bridge {
            tx: self.tx.clone(),
            generation,
            deep: None,
        };
        let profile = self.profile();
        scanner
            .clone()
            .set_name_servers(crate::settings::name_servers());
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
        scanner
            .clone()
            .set_name_servers(crate::settings::name_servers());
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
                let pending = self.pending_compare.borrow_mut().take();
                if let Some(id) = pending {
                    self.compare_with(&id);
                }
                self.update_recognized();
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
            self.recognize_network();
        }
    }

    // -----------------------------------------------------------------------
    // Recognizing the network
    // -----------------------------------------------------------------------

    /// Look for this scan's network among the saved profiles, each time
    /// devices with a stable identifier turn up, and offer the match once.
    fn recognize_network(self: &Rc<Self>) {
        if self.saved.borrow().is_empty() {
            return;
        }
        let live = self.live_hosts();
        let basis = live
            .iter()
            .filter(|h| h.mac.is_some() || h.ssdp_info.is_some())
            .count() as i64;
        if basis <= self.recognition_basis.get() {
            return;
        }
        self.recognition_basis.set(basis);
        let gateway = self
            .networks
            .borrow()
            .iter()
            .filter_map(|n| n.gateway.clone())
            .find(|g| live.iter().any(|h| &h.ip == g));
        let found = recognize_network(&live, gateway, &self.saved.borrow());
        let before = self.scan_notes_profile_id();
        *self.recognized.borrow_mut() = found.clone();
        self.update_recognized();
        if self.scan_notes_profile_id() != before {
            self.scan_notes_changed();
        }
        if let Some(m) = found {
            if !self.suggested.replace(true) {
                debug_log(&format!("network recognized: {m:?}"));
                self.offer_recognized(&m);
            }
        }
    }

    fn offer_recognized(self: &Rc<Self>, m: &NetworkMatch) {
        let dialog = adw::AlertDialog::new(Some("Rete riconosciuta"), Some(&explanation(m)));
        let compare = if self.scanning.get() {
            "Confronta a fine scansione"
        } else {
            "Confronta"
        };
        dialog.add_responses(&[
            ("ignore", "Ignora"),
            ("show", "Mostra profilo"),
            ("compare", compare),
        ]);
        dialog.set_response_appearance("compare", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("compare"));
        dialog.set_close_response("ignore");
        let weak = Rc::downgrade(self);
        let id = m.profile_id.clone();
        dialog.connect_response(None, move |_, response| {
            let Some(this) = weak.upgrade() else { return };
            match response {
                "compare" if this.scanning.get() => {
                    *this.pending_compare.borrow_mut() = Some(id.clone());
                    this.update_recognized();
                }
                "compare" => this.compare_with(&id),
                "show" => this.show_profile_page(&id),
                _ => {}
            }
        });
        dialog.present(Some(&self.window));
    }

    /// The sidebar line naming the recognized network.
    fn update_recognized(&self) {
        let b = &self.w.recognized;
        match &*self.recognized.borrow() {
            Some(m) => {
                let mut text = format!("Rete di «{}»", m.profile_name);
                if self.pending_compare.borrow().is_some() {
                    text.push_str("\nConfronto a fine scansione");
                }
                b.set_child(Some(
                    &adw::ButtonContent::builder()
                        .icon_name("emblem-ok-symbolic")
                        .label(text)
                        .can_shrink(true)
                        .build(),
                ));
                b.set_tooltip_text(Some(&format!(
                    "{} Clic per aprire il profilo.",
                    explanation(m)
                )));
                self.w.recognized_group.set_visible(true);
            }
            None => self.w.recognized_group.set_visible(false),
        }
    }

    /// Open the profiles page on profile `id`.
    fn show_profile_page(self: &Rc<Self>, id: &str) {
        self.w.profile_search.set_text("");
        self.set_tab(Tab::Profiles);
        let list = &self.w.profiles.list;
        let mut child = list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() {
                if row.widget_name() == id {
                    list.select_row(Some(row));
                    break;
                }
            }
            child = widget.next_sibling();
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

    /// Bind the scan's rows again (notes changed), without rebuilding the
    /// device card, where the note may be being written.
    fn refresh_scan_rows(&self) {
        self.quiet_selection.set(true);
        self.table.refresh_rows();
        self.quiet_selection.set(false);
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
        self.update_sidebar_hint();
        self.update_notes_section();
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
        let icon = if running {
            "media-playback-stop-symbolic"
        } else {
            "media-playback-start-symbolic"
        };
        // Compact: a round button with the icon alone, beside the profile.
        if self.compact.get() {
            b.set_icon_name(icon);
            b.remove_css_class("pill");
            b.add_css_class("circular");
        } else {
            b.set_child(Some(
                &adw::ButtonContent::builder()
                    .icon_name(icon)
                    .label(if running { "Ferma" } else { "Avvia scansione" })
                    .build(),
            ));
            b.remove_css_class("circular");
            b.add_css_class("pill");
        }
        b.set_tooltip_text(Some(if running {
            "Ferma la scansione (Ctrl+R)"
        } else {
            "Avvia la scansione (Ctrl+R)"
        }));
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
        self.update_sidebar_hint();
        self.update_notes_section();
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

    /// The scan controls on two rows (short window) or in full.
    fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        let w = &self.w;
        let has_networks = !self.networks.borrow().is_empty();
        w.net_label.set_visible(!compact);
        w.net_dropdown.set_visible(!compact && has_networks);
        w.net_menu.set_visible(compact && has_networks);
        w.profile_row.set_orientation(if compact {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        });
        w.progress_title.set_visible(!compact);
        w.progress_detail.set_wrap(!compact);
        w.progress_detail.set_ellipsize(if compact {
            gtk::pango::EllipsizeMode::Middle
        } else {
            gtk::pango::EllipsizeMode::None
        });
        if compact {
            w.scan_panel.add_css_class("compact");
        } else {
            w.scan_panel.remove_css_class("compact");
        }
        self.update_scan_ui();
    }

    /// The sidebar's hint while the list is empty.
    fn update_sidebar_hint(&self) {
        let empty = self.live_hosts().is_empty();
        self.w.sidebar_hint.set_visible(empty);
        self.w.sidebar_hint.set_text(if self.scanning.get() {
            "Ricerca dei dispositivi…"
        } else {
            "I dispositivi trovati compariranno qui, divisi per tipo."
        });
    }

    /// The Save and Discard of the sidebar's "Note" section.
    fn connect_notes_section(self: &Rc<Self>) {
        let (weak, weak2) = (Rc::downgrade(self), Rc::downgrade(self));
        let bar = detail::unsaved_bar(
            Box::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.save_scan_notes();
                }
            }),
            Box::new(move || {
                if let Some(this) = weak2.upgrade() {
                    this.discard_scan_notes();
                }
            }),
            Some("Ctrl+S"),
        );
        bar.set_hexpand(true);
        bar.set_visible(true);
        if let Some(slot) = self.w.notes.unsaved.downcast_ref::<gtk::Box>() {
            slot.append(&bar);
        }
    }

    /// The sidebar's "Note" section: where the scan's notes go, or why
    /// there are none yet; "Non salvate" while they have changes.
    fn update_notes_section(&self) {
        let n = &self.w.notes;
        n.group.set_visible(!self.live_hosts().is_empty());
        match self.scan_notes_profile() {
            Some(profile) => {
                n.icon.set_icon_name(Some("document-edit-symbolic"));
                n.icon.add_css_class("accent");
                n.title.set_visible(true);
                n.title.set_text(&format!("Nel profilo «{}»", profile.name));
                n.title.set_tooltip_text(Some(&format!(
                    "Le note scritte su questa scansione vanno nel profilo «{}»",
                    profile.name
                )));
                n.hint.set_text(
                    "Passa sulla riga e clicca la matita, o scrivi nella scheda del dispositivo.",
                );
                n.unsaved
                    .set_visible(self.note_drafts.borrow().contains_key(&profile.id));
            }
            None => {
                n.icon.set_icon_name(Some("action-unavailable-symbolic"));
                n.icon.remove_css_class("accent");
                n.title.set_visible(false);
                n.hint.set_text(if self.has_finished_scan() {
                    "Per scrivere note, salva la scansione come profilo."
                } else {
                    "Le note si potranno scrivere a scansione finita, salvandola come profilo, o subito se la rete è già in un profilo."
                });
                n.unsaved.set_visible(false);
            }
        }
    }

    /// The interface menu of the compact scan controls.
    fn rebuild_network_menu(&self) {
        let menu = gio::Menu::new();
        for (i, n) in self.networks.borrow().iter().enumerate() {
            let item = gio::MenuItem::new(Some(&format!("{} · {}", n.interface, n.cidr)), None);
            item.set_action_and_target_value(
                Some("win.pick-network"),
                Some(&(i as u32).to_variant()),
            );
            menu.append_item(&item);
        }
        self.w.net_menu.set_menu_model(Some(&menu));
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
        *self.card_note.borrow_mut() = None;
        let Some((host, offline)) = self.selected_row() else {
            self.w.detail_bin.set_child(Some(&detail::empty()));
            return;
        };
        let deep = self.deep_scanning.borrow().contains(&host.ip);
        let profile = self.scan_notes_profile();
        let (weak, weak2, weak3) = (
            Rc::downgrade(self),
            Rc::downgrade(self),
            Rc::downgrade(self),
        );
        let h = host.clone();
        let note = detail::NoteCard {
            profile_name: profile.as_ref().map(|p| p.name.clone()),
            finished: self.has_finished_scan(),
            text: profile
                .as_ref()
                .map(|p| self.note_of(&p.id, &host))
                .unwrap_or_default(),
            set: Box::new(move |text| {
                let Some(this) = weak.upgrade() else { return };
                if this.syncing_note.get() {
                    return;
                }
                if let Some(id) = this.scan_notes_profile_id() {
                    this.set_note(&id, &h, text, NoteSource::Card);
                }
            }),
            save: Box::new(move || {
                if let Some(this) = weak2.upgrade() {
                    this.save_scan_notes();
                }
            }),
            discard: Box::new(move || {
                if let Some(this) = weak3.upgrade() {
                    this.discard_scan_notes();
                }
            }),
        };
        let card = detail::build(&host, offline, deep, note, self.detail_actions());
        if let (Some(profile), Some(buffer), Some(caption), Some(unsaved)) =
            (profile, card.note, card.caption, card.unsaved)
        {
            *self.card_note.borrow_mut() = Some(CardNote {
                key: device_key(&host),
                buffer,
                caption,
                unsaved,
                profile: profile.name,
            });
        }
        self.w.detail_bin.set_child(Some(&card.widget));
        self.update_card_note();
    }

    /// The card's "Not saved" bar and caption, after a note changed.
    fn update_card_note(&self) {
        let card = self.card_note.borrow();
        let Some(card) = card.as_ref() else { return };
        let unsaved = self
            .scan_notes_profile_id()
            .is_some_and(|id| self.note_drafts.borrow().contains_key(&id));
        card.unsaved.set_visible(unsaved);
        let (start, end) = card.buffer.bounds();
        let text = card.buffer.text(&start, &end, false);
        card.caption.set_visible(!unsaved);
        card.caption
            .set_text(&detail::note_caption(&card.profile, &text));
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
    net_menu: gtk::MenuButton,
    net_label: gtk::Label,
    target: gtk::Entry,
    profile: adw::ToggleGroup,
    profile_row: gtk::Box,
    scan_button: gtk::Button,
    scan_panel: gtk::Box,
    progress_box: gtk::Box,
    progress_bar: gtk::ProgressBar,
    progress_title: gtk::Label,
    progress_detail: gtk::Label,
    recognized_group: gtk::Box,
    recognized: gtk::Button,
    result_group: gtk::Box,
    compare_menu: gtk::MenuButton,
    types_group: gtk::Box,
    types_list: gtk::ListBox,
    hint: gtk::Label,
    notes: NotesSection,
}

/// The scan's sidebar: the devices found, by type (click a type to filter
/// the table), the notes, what the scan recognized and saved, and, pinned at
/// the bottom, the target, profile, start/stop and progress.
fn build_sidebar() -> (gtk::Widget, SidebarWidgets) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 18);
    root.set_margin_top(6);
    root.set_margin_bottom(18);
    root.set_margin_start(10);
    root.set_margin_end(10);

    let types_group = section("Dispositivi");
    if let Some(heading) = types_group.first_child() {
        // A little larger than the other headings, and apart from its list.
        heading.add_css_class("devices-heading");
    }
    let types_list = gtk::ListBox::new();
    types_list.add_css_class("navigation-sidebar");
    types_list.set_selection_mode(gtk::SelectionMode::Single);
    types_list.set_activate_on_single_click(true);
    types_group.append(&types_list);
    types_group.set_visible(false);
    root.append(&types_group);

    // Where the scan's notes go, and the notes not saved yet.
    let notes_group = section("Note");
    let icon = gtk::Image::from_icon_name("document-edit-symbolic");
    icon.set_valign(gtk::Align::Start);
    let notes_title = gtk::Label::new(None);
    notes_title.set_xalign(0.0);
    notes_title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let notes_hint = gtk::Label::new(None);
    notes_hint.set_xalign(0.0);
    notes_hint.set_wrap(true);
    notes_hint.add_css_class("caption");
    notes_hint.add_css_class("dim-label");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&notes_title);
    text.append(&notes_hint);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.set_margin_start(6);
    line.append(&icon);
    line.append(&text);
    notes_group.append(&line);
    let unsaved = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    unsaved.set_margin_start(6);
    notes_group.append(&unsaved);
    notes_group.set_visible(false);
    root.append(&notes_group);

    // The saved profile this network was recognized as.
    let recognized_group = section("Rete riconosciuta");
    let recognized = gtk::Button::new();
    recognized.add_css_class("flat");
    recognized.add_css_class("success");
    recognized.set_halign(gtk::Align::Fill);
    recognized_group.append(&recognized);
    recognized_group.set_visible(false);
    root.append(&recognized_group);

    let result_group = section("Risultato");
    let save = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("document-save-symbolic")
                .label("Salva come profilo…")
                .build(),
        )
        .action_name("win.save-profile")
        .halign(gtk::Align::Start)
        .tooltip_text("Salva i dispositivi trovati per confrontarli con le prossime scansioni")
        .build();
    let compare_menu = gtk::MenuButton::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("view-dual-symbolic")
                .label("Confronta con profilo")
                .build(),
        )
        .halign(gtk::Align::Start)
        .tooltip_text("Trova le differenze rispetto a un profilo salvato")
        .build();
    result_group.append(&save);
    result_group.append(&compare_menu);
    result_group.set_visible(false);
    root.append(&result_group);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&root));
    let hint = gtk::Label::new(None);
    hint.set_wrap(true);
    hint.set_justify(gtk::Justification::Center);
    hint.add_css_class("dim-label");
    hint.set_margin_start(24);
    hint.set_margin_end(24);
    hint.set_valign(gtk::Align::Center);
    let list_area = gtk::Overlay::new();
    list_area.set_child(Some(&scroller));
    list_area.add_overlay(&hint);
    hint.set_can_target(false);

    // The scan controls, pinned at the bottom.
    let net_label = gtk::Label::new(Some("Rete"));
    net_label.set_xalign(0.0);
    net_label.add_css_class("caption-heading");
    net_label.add_css_class("dim-label");
    let net_model = gtk::StringList::new(&[]);
    let net_dropdown = gtk::DropDown::new(Some(net_model.clone()), None::<gtk::Expression>);
    let target = gtk::Entry::builder()
        .placeholder_text("192.168.1.0/24")
        .hexpand(true)
        .build();
    target.set_tooltip_text(Some(
        "Una rete (CIDR), un IP, un intervallo (192.168.1.10-50) o un elenco",
    ));
    // On two rows, the interface is chosen from a menu beside the target.
    let net_menu = gtk::MenuButton::builder()
        .icon_name("network-wired-symbolic")
        .tooltip_text("Scegli l'interfaccia di rete")
        .visible(false)
        .build();
    net_menu.add_css_class("flat");
    let target_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    target_row.append(&target);
    target_row.append(&net_menu);
    let profile = adw::ToggleGroup::builder()
        .homogeneous(false)
        .hexpand(true)
        .build();
    profile.add_css_class("scan-profile");
    for p in labels::PROFILES {
        profile.add(
            adw::Toggle::builder()
                .label(labels::profile_label(p))
                .name(labels::profile_name(p))
                .build(),
        );
    }
    profile.set_active_name(Some(labels::profile_name(ScanProfile::Standard)));
    let scan_button = gtk::Button::new();
    scan_button.add_css_class("pill");
    scan_button.set_tooltip_text(Some("Avvia o ferma la scansione (Ctrl+R)"));
    // The profile and the button: one above the other, side by side when
    // compact.
    let profile_row = gtk::Box::new(gtk::Orientation::Vertical, 8);
    profile_row.append(&profile);
    profile_row.append(&scan_button);

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

    let scan_panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
    scan_panel.add_css_class("scan-panel");
    scan_panel.append(&net_label);
    scan_panel.append(&net_dropdown);
    scan_panel.append(&target_row);
    scan_panel.append(&profile_row);
    scan_panel.append(&progress_box);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.append(&list_area);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    page.append(&scan_panel);
    (
        page.upcast(),
        SidebarWidgets {
            net_dropdown,
            net_model,
            net_menu,
            net_label,
            target,
            profile,
            profile_row,
            scan_button,
            scan_panel,
            progress_box,
            progress_bar,
            progress_title,
            progress_detail,
            recognized_group,
            recognized,
            result_group,
            compare_menu,
            types_group,
            types_list,
            hint,
            notes: NotesSection {
                group: notes_group,
                title: notes_title,
                hint: notes_hint,
                icon,
                unsaved: unsaved.upcast(),
            },
        },
    )
}

/// The tabs, as pills: "Scansione" (Ctrl+1) and "Profili salvati" (Ctrl+2).
fn build_tabs() -> (gtk::Box, gtk::ToggleButton, gtk::ToggleButton) {
    let tab = |label: &str, icon: &str, tip: &str| {
        let b = gtk::ToggleButton::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name(icon)
                    .label(label)
                    .build(),
            )
            .tooltip_text(tip)
            .build();
        b.add_css_class("tab-pill");
        b
    };
    let scan = tab(
        "Scansione",
        "network-transmit-receive-symbolic",
        "Scansione (Ctrl+1)",
    );
    let profiles = tab(
        "Profili salvati",
        "document-save-symbolic",
        "Profili salvati (Ctrl+2)",
    );
    profiles.set_group(Some(&scan));
    scan.set_active(true);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    b.add_css_class("tab-switcher");
    b.append(&scan);
    b.append(&profiles);
    (b, scan, profiles)
}

/// Move `widget` into `slot` (out of wherever it is).
fn move_to(widget: &impl IsA<gtk::Widget>, slot: &gtk::Box) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    let slot_widget: &gtk::Widget = slot.upcast_ref();
    if widget.parent().as_ref() == Some(slot_widget) {
        return;
    }
    if let Some(parent) = widget.parent().and_downcast::<gtk::Box>() {
        parent.remove(widget);
    }
    slot.append(widget);
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
    let prefs = gio::Menu::new();
    prefs.append(Some("Preferenze"), Some("win.preferences"));
    menu.append_section(None, &prefs);
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

/// "Esporta CSV"; its items follow the selection and the search
/// (`rebuild_export_menu`).
fn export_menu_button() -> gtk::MenuButton {
    gtk::MenuButton::builder()
        .icon_name("document-save-as-symbolic")
        .tooltip_text("Esporta i profili salvati in un file CSV, con le note")
        .build()
}

/// Why the network was recognized, for the user.
fn explanation(m: &NetworkMatch) -> String {
    let others = m.matched_devices - u32::from(m.gateway_matched);
    let evidence = if m.gateway_matched && others > 0 {
        format!("c'è lo stesso router (stesso MAC) e altri {others} dispositivi del profilo")
    } else if m.gateway_matched {
        "c'è lo stesso router (stesso MAC)".to_string()
    } else {
        format!(
            "ci sono {} dei {} dispositivi del profilo, riconosciuti dal MAC",
            m.matched_devices, m.profile_devices
        )
    };
    format!(
        "Questa sembra la rete del profilo «{}»: {evidence}.",
        m.profile_name
    )
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
