//! Page widgets and their update functions. Presentation only: values,
//! units and sorting come from `nysm-core`. Layout follows the reference
//! design (calm overview with cards); labels that would misstate a metric
//! in the reference are corrected here (pressure is never shown as usage,
//! unavailable sensors are shown as unavailable).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use nysm_core::alerts::{ActiveAlert, AlertState};
use nysm_core::history::HistoryPoint;
use nysm_core::query::{self, ProcessSort};
use nysm_core::raw::DiskKind;
use nysm_core::sanitize::for_terminal;
use nysm_core::snapshot::{Pressure, Snapshot};
use nysm_core::units::{self, RateUnit};
use nysm_core::{Reading, Status};

use crate::chart::{self, Chart, Series};
use crate::proctable::{self, ProcTable};
use crate::stats::{self, CoreTile, Tile, WatchTile};

/// Cap for the (non-virtualized) group list.
const MAX_LIST_ROWS: usize = 200;
const TOP_ROWS: usize = 6;

/// Called with (pid, start_ticks) when the user activates a process row.
type DetailsCallback = Rc<RefCell<Option<Box<dyn Fn(u32, u64)>>>>;

fn safe(s: &str) -> String {
    for_terminal(s).into_owned()
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn clear(list: &gtk::ListBox) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
}

/// A table-like row of fixed-width cells; width 0 = flexible column.
fn row(cells: &[(String, i32, f32)]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    b.set_hexpand(true);
    b.set_margin_top(4);
    b.set_margin_bottom(4);
    b.set_margin_start(6);
    b.set_margin_end(6);
    for (text, width, xalign) in cells {
        let l = gtk::Label::new(Some(text));
        l.set_xalign(*xalign);
        l.add_css_class("numeric");
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if *width > 0 {
            l.set_width_chars(*width);
            l.set_max_width_chars(*width);
        } else {
            l.set_hexpand(true);
            l.set_width_chars(16);
        }
        b.append(&l);
    }
    b
}

/// Update (or lazily create) row `k`; rows are reused across refreshes so
/// no widgets are created or destroyed in steady state.
fn set_row(list: &gtk::ListBox, k: usize, cells: &[(String, i32, f32)]) {
    match list.row_at_index(k as i32) {
        Some(r) => {
            r.set_visible(true);
            let mut child = r.child().and_then(|b| b.first_child());
            for (text, _, _) in cells {
                let Some(w) = child else { break };
                if let Some(l) = w.downcast_ref::<gtk::Label>()
                    && l.text() != text.as_str()
                {
                    l.set_text(text);
                }
                child = w.next_sibling();
            }
        }
        None => list.append(&row(cells)),
    }
}

fn hide_rows_from(list: &gtk::ListBox, n: usize) {
    let mut k = n as i32;
    while let Some(r) = list.row_at_index(k) {
        r.set_visible(false);
        k += 1;
    }
}

fn missing<T>(r: &Reading<T>) -> String {
    format!("— ({})", r.status.label())
}

fn psi_text(r: &Reading<Pressure>) -> String {
    match r.live() {
        Some(p) => {
            let now = p
                .some
                .interval_pct
                .map_or("—".into(), |v| format!("{v:.1}%"));
            format!("pressure {now}")
        }
        None if r.status == Status::Unsupported => "pressure n/a".into(),
        None => format!("pressure {}", r.status.label()),
    }
}

/// (value, hint) for a pressure tile: the stall share over the last
/// sample, with the kernel's 1-minute average as context.
fn psi_tile(r: &Reading<Pressure>) -> (String, String) {
    match r.live() {
        Some(p) => (
            p.some
                .interval_pct
                .map_or("—".into(), |v| format!("{v:.1}%")),
            format!("1 min avg {:.1}%", p.some.avg60_pct),
        ),
        None => ("—".into(), r.status.label().into()),
    }
}

/// Kernel process state letter in words.
fn state_name(c: char) -> &'static str {
    match c {
        'R' => "running",
        'S' => "sleeping",
        'D' => "waiting on I/O (uninterruptible)",
        'Z' => "zombie (exited, not yet reaped)",
        'T' | 't' => "stopped",
        'I' => "idle kernel thread",
        'X' => "dead",
        _ => "unknown",
    }
}

fn pct(v: f64) -> String {
    format!("{v:.0}%")
}

fn bytes_rate(v: f64) -> String {
    units::rate(v, RateUnit::Bytes)
}

fn bits_rate(v: f64) -> String {
    units::rate(v, RateUnit::Bits)
}

fn series(h: &[HistoryPoint], f: impl Fn(&HistoryPoint) -> Option<f32>) -> Vec<Option<f64>> {
    h.iter().map(|p| f(p).map(|v| v as f64)).collect()
}

fn gaps(h: &[HistoryPoint]) -> Vec<usize> {
    h.iter()
        .enumerate()
        .filter(|(i, p)| *i > 0 && p.gap_before)
        .map(|(i, _)| i)
        .collect()
}

fn span_s(h: &[HistoryPoint]) -> f64 {
    match (h.first(), h.last()) {
        (Some(a), Some(b)) => (b.timestamp_ms - a.timestamp_ms) as f64 / 1000.0,
        _ => 0.0,
    }
}

pub(crate) fn page(stack: &gtk::Stack, name: &str, title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 14);
    b.set_margin_top(18);
    b.set_margin_bottom(18);
    b.set_margin_start(22);
    b.set_margin_end(22);
    b.append(&label(title, &["page-title"]));
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&b));
    // Horizontal scrolling (when needed) keeps the window usable on small
    // screens with large text, instead of forcing a wide minimum size.
    scroll.set_hscrollbar_policy(gtk::PolicyType::Automatic);
    stack.add_named(&scroll, Some(name));
    b
}

/// A card: returns (card, header box). The header holds the title on the
/// left; callers append right-aligned content.
fn card(title: &str) -> (gtk::Box, gtk::Box) {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 8);
    c.add_css_class("card");
    c.set_hexpand(true);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let t = label(title, &["card-title"]);
    t.set_hexpand(true);
    header.append(&t);
    if !title.is_empty() {
        c.append(&header);
    }
    (c, header)
}

/// Legend entry "● Name value" with the series colour on the dot.
fn legend(header: &gtk::Box, name: &str, dot_class: &str) -> gtk::Label {
    let dot = label("●", &[dot_class]);
    let n = label(name, &["dim-label"]);
    let v = label("—", &["numeric"]);
    for w in [&dot, &n, &v] {
        w.set_wrap(false);
        header.append(w);
    }
    v
}

/// (pid, start ticks, name) of each table row, in display order.
type RowIds = Vec<(u32, u64, String)>;

struct ProcState {
    last: Option<Arc<Snapshot>>,
    rows: RowIds,
    rendered_ts: Option<i64>,
    /// Process shown in the details panel, refreshed while visible.
    details_for: Option<(u32, u64)>,
    details_at: Option<std::time::Instant>,
}

/// Most processes that can be watched at once (engine limit).
const MAX_WATCHED: usize = nysm_core::history::ProcessHistory::DEFAULT_MAX_PINS;

type WatchCallback = Rc<RefCell<Option<Box<dyn Fn(nysm_core::raw::ProcessId, Option<String>)>>>>;

pub struct Pages {
    rate: RateUnit,
    stack: gtk::Stack,
    /// Visible time window for charts, seconds.
    window_s: Cell<f64>,
    // Overview
    source: gtk::Label,
    alert_banner: gtk::Label,
    cpu_value: gtk::Label,
    cpu_sub: gtk::Label,
    cpu_detail: gtk::Label,
    cpu_chart: Chart,
    mem_value: gtk::Label,
    mem_pct: gtk::Label,
    mem_detail: gtk::Label,
    mem_mini: Chart,
    net_rx: gtk::Label,
    net_tx: gtk::Label,
    net_ov_chart: Chart,
    fs_value: gtk::Label,
    fs_pct: gtk::Label,
    fs_bar: gtk::LevelBar,
    fs_detail: gtk::Label,
    disk_r: gtk::Label,
    disk_w: gtk::Label,
    disk_ov_chart: Chart,
    top: gtk::ListBox,
    // Processes
    proc_search: gtk::SearchEntry,
    proc_sort: gtk::DropDown,
    proc_count: gtk::Label,
    proc_table: ProcTable,
    proc_details: gtk::Label,
    watch_btn: gtk::Button,
    watch_card: gtk::Box,
    watch_title: gtk::Label,
    watch_box: gtk::FlowBox,
    watch_tiles: RefCell<Vec<(nysm_core::raw::ProcessId, WatchTile)>>,
    /// Called with (id, Some(name)) to watch, (id, None) to stop.
    watch_cb: WatchCallback,
    watched_count: Rc<Cell<usize>>,
    proc_state: Rc<RefCell<ProcState>>,
    details_cb: DetailsCallback,
    // CPU
    cpu_stats: Vec<Tile>,
    cpu_page_chart: Chart,
    cpu_cores: gtk::FlowBox,
    core_tiles: RefCell<Vec<CoreTile>>,
    core_seq: std::cell::Cell<u64>,
    // Memory
    mem_stats: Vec<Tile>,
    mem_chart: Chart,
    mem_psi_chart: Chart,
    // Network
    net_chart: Chart,
    net_list: gtk::ListBox,
    // Storage
    disk_chart: Chart,
    disk_info: gtk::Label,
    disk_list: gtk::ListBox,
    fs_list: gtk::ListBox,
    // Containers & services
    groups_kind: gtk::DropDown,
    groups_sort: gtk::DropDown,
    names: ContainerNames,
    groups_count: gtk::Label,
    groups_list: gtk::ListBox,
}

impl Pages {
    pub fn new(stack: &gtk::Stack, rate: RateUnit, source: String, container_names: bool) -> Self {
        let net_fmt: fn(f64) -> String = if rate == RateUnit::Bits {
            bits_rate
        } else {
            bytes_rate
        };

        // ---------------- Overview
        let ov = page(stack, "overview", "System overview");
        let alert_banner = label("", &["alert-banner"]);
        alert_banner.set_visible(false);
        ov.append(&alert_banner);

        let (cpu_card, cpu_head) = card("CPU usage");
        let cpu_right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let cpu_value = label("—", &["big-value"]);
        cpu_value.set_xalign(1.0);
        let cpu_sub = label("", &["dim-label", "numeric"]);
        cpu_sub.set_xalign(1.0);
        cpu_right.append(&cpu_value);
        cpu_right.append(&cpu_sub);
        cpu_head.append(&cpu_right);
        let cpu_chart = Chart::new(150, Some(100.0), pct);
        cpu_card.append(&cpu_chart.area);
        let cpu_detail = label("", &["dim-label"]);
        cpu_card.append(&cpu_detail);
        ov.append(&cpu_card);

        let grid = gtk::Grid::new();
        grid.set_column_spacing(14);
        grid.set_row_spacing(14);
        grid.set_column_homogeneous(true);

        let (mem_card, mem_head) = card("Memory");
        let mem_pct = label("—", &["mid-value"]);
        mem_head.append(&mem_pct);
        let mem_value = label("—", &["mid-value", "numeric"]);
        mem_card.append(&mem_value);
        let mem_mini = Chart::mini(70, Some(100.0), pct);
        mem_card.append(&mem_mini.area);
        let mem_detail = label("", &["dim-label"]);
        mem_card.append(&mem_detail);
        grid.attach(&mem_card, 0, 0, 1, 1);

        let (net_card, net_head) = card("Network (throughput)");
        let net_rx = legend(&net_head, "Download", "c-net-rx");
        let net_tx = legend(&net_head, "Upload", "c-net-tx");
        let net_ov_chart = Chart::new(120, None, net_fmt);
        net_card.append(&net_ov_chart.area);
        grid.attach(&net_card, 1, 0, 1, 1);

        let (fs_card, fs_head) = card("Storage");
        let fs_pct = label("—", &["mid-value"]);
        fs_head.append(&fs_pct);
        let fs_value = label("—", &["mid-value", "numeric"]);
        fs_card.append(&fs_value);
        let fs_bar = gtk::LevelBar::for_interval(0.0, 100.0);
        fs_bar.add_css_class("capacity");
        fs_card.append(&fs_bar);
        let fs_detail = label("", &["dim-label"]);
        fs_card.append(&fs_detail);
        grid.attach(&fs_card, 0, 1, 1, 1);

        let (disk_card, disk_head) = card("Disk activity");
        let disk_r = legend(&disk_head, "Read", "c-disk-r");
        let disk_w = legend(&disk_head, "Write", "c-disk-w");
        let disk_ov_chart = Chart::new(120, None, bytes_rate);
        disk_card.append(&disk_ov_chart.area);
        grid.attach(&disk_card, 1, 1, 1, 1);
        ov.append(&grid);

        let (top_card, _) = card("Top processes (CPU, % of all cores)");
        let top = gtk::ListBox::new();
        top.set_selection_mode(gtk::SelectionMode::None);
        top.add_css_class("flat-list");
        top_card.append(&top);
        ov.append(&top_card);
        let source_l = label(&source, &["dim-label", "caption"]);
        ov.append(&source_l);

        // ---------------- Processes
        let pp = page(stack, "processes", "Processes");
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let proc_search = gtk::SearchEntry::new();
        proc_search.set_placeholder_text(Some("Filter by name, PID or user"));
        proc_search.set_hexpand(true);
        let proc_sort = gtk::DropDown::from_strings(&[
            "Sort: CPU",
            "Sort: memory",
            "Sort: disk I/O",
            "Sort: name",
            "Sort: PID",
        ]);
        let proc_count = label("", &["dim-label"]);
        proc_count.set_wrap(false);
        proc_count.set_ellipsize(gtk::pango::EllipsizeMode::End);
        proc_count.set_max_width_chars(28);
        let watch_btn = gtk::Button::with_label("Watch");
        watch_btn.set_sensitive(false);
        watch_btn.set_tooltip_text(Some(
            "Keep CPU and memory history for the selected processes (Ctrl/Shift-click to select several; up to 8)",
        ));
        bar.append(&proc_search);
        bar.append(&proc_sort);
        bar.append(&watch_btn);
        bar.append(&proc_count);
        pp.append(&bar);
        let (watch_card, watch_header) = card("");
        let watch_title = label("", &["card-title"]);
        watch_title.set_hexpand(true);
        watch_header.prepend(&watch_title);
        watch_card.prepend(&watch_header);
        let watch_box = gtk::FlowBox::new();
        watch_box.set_selection_mode(gtk::SelectionMode::None);
        watch_box.set_homogeneous(true);
        watch_box.set_max_children_per_line(4);
        watch_box.set_row_spacing(8);
        watch_box.set_column_spacing(8);
        watch_card.append(&watch_box);
        watch_card.set_visible(false);
        pp.append(&watch_card);
        let (list_card, _) = card("");
        // Table | details, with a draggable divider. The details side starts
        // at DETAILS_W and keeps that width when the window is resized.
        const DETAILS_W: i32 = 340;
        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_vexpand(true);
        split.set_wide_handle(true);
        split.set_resize_start_child(true);
        split.set_resize_end_child(false);
        split.set_shrink_start_child(false);
        split.set_shrink_end_child(false);
        let proc_table = ProcTable::new();
        // Its own scroller keeps the view virtualized (only visible rows
        // get widgets) and fills the page height.
        let proc_scroll = gtk::ScrolledWindow::new();
        proc_scroll.set_child(Some(&proc_table.view));
        proc_scroll.set_hexpand(true);
        proc_scroll.set_vexpand(true);
        proc_scroll.set_min_content_height(360);
        // Ask for the columns' full width so the details panel cannot take it.
        proc_scroll.set_propagate_natural_width(true);
        list_card.append(&proc_scroll);
        split.set_start_child(Some(&list_card));
        let (details_card, _) = card("Details");
        details_card.set_hexpand(false);
        details_card.set_valign(gtk::Align::Start);
        let proc_details = label("Select a process to see its live details.", &["dim-label"]);
        proc_details.set_selectable(true);
        // Fixed width so long paths wrap instead of squeezing the table.
        proc_details.set_max_width_chars(34);
        proc_details.set_width_chars(34);
        proc_details.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        // Cap the panel's width: long paths wrap inside it.
        let details_scroll = gtk::ScrolledWindow::new();
        details_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        details_scroll.set_propagate_natural_width(true);
        details_scroll.set_propagate_natural_height(true);
        details_scroll.set_max_content_width(320);
        details_scroll.set_child(Some(&proc_details));
        details_card.append(&details_scroll);
        details_card.set_margin_start(8);
        split.set_end_child(Some(&details_card));
        {
            // Place the divider once the page has a width; afterwards the
            // user's position is kept (end child does not resize).
            let placed = Cell::new(false);
            split.connect_max_position_notify(move |p| {
                if !placed.get() && p.max_position() > DETAILS_W * 2 {
                    placed.set(true);
                    p.set_position(p.max_position() - DETAILS_W);
                }
            });
        }
        pp.append(&split);
        // Typing anywhere on the page starts filtering; focus moves into
        // the table when the page opens so key presses reach the page.
        proc_search.set_key_capture_widget(Some(&pp));
        {
            let view = proc_table.view.clone();
            stack.connect_visible_child_name_notify(move |st| {
                if st.visible_child_name().is_some_and(|n| n == "processes") {
                    view.grab_focus();
                }
            });
        }

        // ---------------- CPU
        let cp = page(stack, "cpu", "CPU");
        let (c1, _) = card("Usage, % of all logical cores");
        let (cpu_grid, cpu_stats) = stats::grid(&[
            ("Usage", "Share of all logical cores that were busy"),
            ("User", "Time running application code"),
            ("System", "Time running kernel code for applications"),
            ("Nice", "Time running low-priority (niced) processes"),
            ("Interrupts", "Hardware and software interrupt handling"),
            (
                "I/O wait",
                "Idle time while disk I/O was pending. Not CPU work, and not counted as busy",
            ),
            ("Steal", "Time a hypervisor gave to other virtual machines"),
            (
                "Load average",
                "Run-queue length (running + waiting tasks), not a percentage. Compare with the number of cores",
            ),
            (
                "Pressure",
                "Share of time at least one task waited for a CPU (PSI \"some\")",
            ),
        ]);
        c1.append(&cpu_grid);
        let cpu_page_chart = Chart::new(170, Some(100.0), pct);
        c1.append(&cpu_page_chart.area);
        cp.append(&c1);
        let (c2, _) = card("Cores");
        c2.append(&label(
            "Logical CPUs as the kernel schedules them: usage, current frequency and the last 2 minutes. \
             With hyper-threading, two logical CPUs share one physical core.",
            &["dim-label", "caption"],
        ));
        let cpu_cores = gtk::FlowBox::new();
        cpu_cores.set_selection_mode(gtk::SelectionMode::None);
        cpu_cores.set_min_children_per_line(2);
        cpu_cores.set_max_children_per_line(8);
        cpu_cores.set_homogeneous(true);
        cpu_cores.set_row_spacing(8);
        cpu_cores.set_column_spacing(8);
        c2.append(&cpu_cores);
        cp.append(&c2);

        // ---------------- Memory
        let mp = page(stack, "memory", "Memory");
        let (m1, _) = card("Used memory (total − available), %");
        let (mem_grid, mem_stats) = stats::grid(&[
            ("Used", "Total minus available"),
            (
                "Available",
                "Kernel estimate of memory usable without swapping, including reclaimable cache",
            ),
            (
                "Cache",
                "File contents cached in memory; reclaimed when needed",
            ),
            (
                "Reclaimable slab",
                "Kernel caches that can be freed under pressure",
            ),
            ("Shared", "Shared memory and tmpfs"),
            ("Dirty", "Modified file data not yet written to disk"),
            ("Swap used", "Memory moved to swap"),
            ("Swapping", "Pages read from / written to swap per second"),
            (
                "Pressure",
                "Share of time at least one task stalled waiting for memory (PSI \"some\")",
            ),
        ]);
        m1.append(&mem_grid);
        let mem_chart = Chart::new(150, Some(100.0), pct);
        m1.append(&mem_chart.area);
        mp.append(&m1);
        let (m2, _) = card("Memory pressure: time tasks stalled waiting for memory (PSI some)");
        let mem_psi_chart = Chart::new(110, Some(100.0), pct);
        m2.append(&mem_psi_chart.area);
        mp.append(&m2);

        // ---------------- Network
        let np = page(stack, "network", "Network");
        let (n1, _) =
            card("Throughput — physical interfaces (excludes loopback, bridges, veth, tunnels)");
        let net_chart = Chart::new(170, None, net_fmt);
        n1.append(&net_chart.area);
        n1.append(&label(
            "Includes LAN traffic; this is not internet speed or link capacity.",
            &["dim-label"],
        ));
        np.append(&n1);
        let (n2, _) = card("Interfaces");
        let net_list = gtk::ListBox::new();
        net_list.set_selection_mode(gtk::SelectionMode::None);
        net_list.add_css_class("flat-list");
        n2.append(&net_list);
        np.append(&n2);

        // ---------------- Storage
        let sp = page(stack, "storage", "Storage");
        let (s1, _) = card("Disk activity — whole disks");
        let disk_info = label("", &["dim-label"]);
        s1.append(&disk_info);
        let disk_chart = Chart::new(150, None, bytes_rate);
        s1.append(&disk_chart.area);
        sp.append(&s1);
        let (s2, _) =
            card("Volumes (capacity per filesystem; available = what a normal user can allocate)");
        let fs_list = gtk::ListBox::new();
        fs_list.set_selection_mode(gtk::SelectionMode::None);
        fs_list.add_css_class("flat-list");
        s2.append(&fs_list);
        sp.append(&s2);
        let (s3, _) = card(
            "Block devices (activity per device; in-flight is not saturation on parallel devices)",
        );
        let disk_list = gtk::ListBox::new();
        disk_list.set_selection_mode(gtk::SelectionMode::None);
        disk_list.add_css_class("flat-list");
        s3.append(&disk_list);
        sp.append(&s3);

        // ---------------- Containers & services
        let gp = page(stack, "groups", "Containers & services");
        let gbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let groups_kind =
            gtk::DropDown::from_strings(&["All groups", "Containers", "Services", "Apps"]);
        let groups_sort = gtk::DropDown::from_strings(&[
            "Sort: CPU",
            "Sort: memory",
            "Sort: disk I/O",
            "Sort: name",
        ]);
        let groups_count = label("", &["dim-label"]);
        groups_count.set_hexpand(true);
        groups_count.set_wrap(false);
        gbar.append(&groups_kind);
        gbar.append(&groups_sort);
        gbar.append(&groups_count);
        let names_switch = gtk::Switch::new();
        names_switch.set_active(container_names);
        names_switch.set_valign(gtk::Align::Center);
        let names_l = label("Container names", &[]);
        names_l.set_wrap(false);
        let tip = "Ask Docker/Podman for container names (one read-only request every 30 s while this page is open). \
                   Off by default: access to that socket is equivalent to root on most systems.";
        names_switch.set_tooltip_text(Some(tip));
        names_l.set_tooltip_text(Some(tip));
        gbar.append(&names_l);
        gbar.append(&names_switch);
        gp.append(&gbar);
        let (gcard, _) = card("");
        gcard.append(&label(
            "From cgroup v2 accounting, read as a normal user. Memory includes page cache charged to the group. \
             Without \"Container names\" containers are shown as runtime + id.",
            &["dim-label"],
        ));
        let groups_list = gtk::ListBox::new();
        groups_list.set_selection_mode(gtk::SelectionMode::None);
        groups_list.add_css_class("flat-list");
        gcard.append(&groups_list);
        gp.append(&gcard);

        let pages = Pages {
            rate,
            stack: stack.clone(),
            window_s: Cell::new(300.0),
            source: source_l,
            alert_banner,
            cpu_value,
            cpu_sub,
            cpu_detail,
            cpu_chart,
            mem_value,
            mem_pct,
            mem_detail,
            mem_mini,
            net_rx,
            net_tx,
            net_ov_chart,
            fs_value,
            fs_pct,
            fs_bar,
            fs_detail,
            disk_r,
            disk_w,
            disk_ov_chart,
            top,
            proc_search,
            proc_sort,
            proc_count,
            proc_table,
            proc_details,
            watch_btn,
            watch_card,
            watch_title,
            watch_box,
            watch_tiles: RefCell::new(Vec::new()),
            watch_cb: Rc::default(),
            watched_count: Rc::default(),
            proc_state: Rc::new(RefCell::new(ProcState {
                last: None,
                rows: Vec::new(),
                rendered_ts: None,
                details_for: None,
                details_at: None,
            })),
            details_cb: Rc::new(RefCell::new(None)),
            cpu_stats,
            cpu_page_chart,
            cpu_cores,
            core_tiles: RefCell::new(Vec::new()),
            core_seq: std::cell::Cell::new(0),
            mem_stats,
            mem_chart,
            mem_psi_chart,
            net_chart,
            net_list,
            disk_chart,
            disk_info,
            disk_list,
            fs_list,
            groups_kind,
            groups_sort,
            names: ContainerNames::new(names_switch),
            groups_count,
            groups_list,
        };
        pages.wire_process_controls();
        pages
    }

    fn wire_process_controls(&self) {
        let rerender = {
            let (st, list, count, search, sort) = (
                self.proc_state.clone(),
                self.proc_table.clone(),
                self.proc_count.clone(),
                self.proc_search.clone(),
                self.proc_sort.clone(),
            );
            Rc::new(move || {
                let snap = st.borrow().last.clone();
                if let Some(s) = snap {
                    let prev = std::mem::take(&mut st.borrow_mut().rows);
                    let shown = st.borrow().details_for;
                    let rows = render_processes(
                        &s,
                        &list,
                        &count,
                        &search.text(),
                        sort_key(sort.selected()),
                        &prev,
                        shown,
                    );
                    st.borrow_mut().rows = rows;
                }
            })
        };
        let r = rerender.clone();
        self.proc_search.connect_search_changed(move |_| r());
        let r = rerender.clone();
        self.proc_sort.connect_selected_notify(move |_| r());
        let (st, cb, details) = (
            self.proc_state.clone(),
            self.details_cb.clone(),
            self.proc_details.clone(),
        );
        self.proc_table.connect_activate(move |idx| {
            let Some((pid, start)) = st.borrow().rows.get(idx).map(|r| (r.0, r.1)) else {
                return;
            };
            details.set_text(&format!("Loading details for PID {pid}…"));
            {
                let mut s = st.borrow_mut();
                s.details_for = Some((pid, start));
                s.details_at = Some(std::time::Instant::now());
            }
            if let Some(f) = cb.borrow().as_ref() {
                f(pid, start);
            }
        });
        let (btn, st, table, cb, details) = (
            self.watch_btn.clone(),
            self.proc_state.clone(),
            self.proc_table.clone(),
            self.details_cb.clone(),
            self.proc_details.clone(),
        );
        self.proc_table.connect_selection_changed(move |n| {
            btn.set_sensitive(n > 0);
            // A single selected process is shown in the details panel.
            let rows = table.selected_rows();
            if rows.len() != 1 {
                return;
            }
            let Some((pid, start)) = st.borrow().rows.get(rows[0]).map(|r| (r.0, r.1)) else {
                return;
            };
            if st.borrow().details_for == Some((pid, start)) {
                return; // same process (selection restored after a refresh)
            }
            details.set_text(&format!("Loading details for PID {pid}…"));
            {
                let mut s = st.borrow_mut();
                s.details_for = Some((pid, start));
                s.details_at = Some(std::time::Instant::now());
            }
            if let Some(f) = cb.borrow().as_ref() {
                f(pid, start);
            }
        });
        let (st, table, cb) = (
            self.proc_state.clone(),
            self.proc_table.clone(),
            self.watch_cb.clone(),
        );
        let watched = self.watch_tiles_count();
        self.watch_btn.connect_clicked(move |_| {
            let rows = st.borrow().rows.clone();
            let free = MAX_WATCHED.saturating_sub(watched.get());
            for i in table.selected_rows().into_iter().take(free) {
                if let Some((pid, start, name)) = rows.get(i)
                    && let Some(f) = cb.borrow().as_ref()
                {
                    f(
                        nysm_core::raw::ProcessId {
                            pid: *pid,
                            start_ticks: *start,
                        },
                        Some(name.clone()),
                    );
                }
            }
            table.unselect_all();
        });
    }

    /// Shared count of watched processes, for the Watch button's limit.
    fn watch_tiles_count(&self) -> Rc<Cell<usize>> {
        self.watched_count.clone()
    }

    pub fn on_watch_changed(
        &self,
        f: impl Fn(nysm_core::raw::ProcessId, Option<String>) + 'static,
    ) {
        *self.watch_cb.borrow_mut() = Some(Box::new(f));
    }

    /// Demo captures: filter to `name`, watch the live processes whose
    /// name contains it, and show the first in the details panel.
    pub fn demo_watch(&self, name: &str) {
        self.proc_search.set_text(name);
        let snap = self.proc_state.borrow().last.clone();
        let Some(t) = snap.as_ref().and_then(|s| s.processes.as_ref()) else {
            return;
        };
        let mut live: Vec<_> = t
            .entries
            .iter()
            .filter(|p| p.name.contains(name) && p.state != 'Z' && p.rss_bytes > 0)
            .collect();
        live.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.pid.cmp(&b.id.pid)));
        live.dedup_by(|a, b| a.name == b.name);
        for p in &live {
            if let Some(f) = self.watch_cb.borrow().as_ref() {
                f(p.id.clone(), Some(p.name.clone()));
            }
        }
        if let Some(p) = live.first() {
            {
                let mut st = self.proc_state.borrow_mut();
                st.details_for = Some((p.id.pid, p.id.start_ticks));
                st.details_at = Some(std::time::Instant::now());
            }
            if let Some(f) = self.details_cb.borrow().as_ref() {
                f(p.id.pid, p.id.start_ticks);
            }
        }
    }

    /// The process whose details should be refreshed now (every second
    /// while the Processes page is shown), if any.
    pub fn details_due(&self) -> Option<(u32, u64)> {
        if self.visible_page() != "processes" {
            return None;
        }
        let mut st = self.proc_state.borrow_mut();
        let id = st.details_for?;
        if st
            .details_at
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1))
        {
            return None;
        }
        st.details_at = Some(std::time::Instant::now());
        Some(id)
    }

    /// Show the watched (pinned) processes; tiles are rebuilt only when the
    /// set changes.
    fn update_watch(&self, pinned: &[nysm_core::history::PinnedProcess]) {
        self.watched_count.set(pinned.len());
        self.watch_card.set_visible(!pinned.is_empty());
        self.watch_title.set_text(&format!(
            "Watching {} of {MAX_WATCHED} — CPU and memory since watching started",
            pinned.len()
        ));
        let mut tiles = self.watch_tiles.borrow_mut();
        let same =
            tiles.len() == pinned.len() && tiles.iter().zip(pinned).all(|((id, _), p)| *id == p.id);
        if !same {
            while let Some(c) = self.watch_box.first_child() {
                self.watch_box.remove(&c);
            }
            tiles.clear();
            for p in pinned {
                let t = WatchTile::new();
                let (cb, id) = (self.watch_cb.clone(), p.id.clone());
                t.stop.connect_clicked(move |_| {
                    if let Some(f) = cb.borrow().as_ref() {
                        f(id.clone(), None);
                    }
                });
                self.watch_box.insert(&t.root, -1);
                tiles.push((p.id.clone(), t));
            }
        }
        for ((_, t), p) in tiles.iter().zip(pinned) {
            t.show(p);
        }
    }

    pub fn on_details_requested(&self, f: impl Fn(u32, u64) + 'static) {
        *self.details_cb.borrow_mut() = Some(Box::new(f));
    }

    pub fn show_details(&self, r: nysm_collect::CResult<nysm_collect::ProcessDetails>) {
        use gtk::glib::markup_escape_text as esc;
        let reason = |e: &nysm_collect::CollectError| format!("— ({})", e.status().label());
        let show = |x: &nysm_collect::CResult<String>| match x {
            Ok(v) => safe(v),
            Err(e) => reason(e),
        };
        // Live values for the same process from the latest table.
        let (proc_line, live_lines) = {
            let st = self.proc_state.borrow();
            let p = st.details_for.and_then(|(pid, start)| {
                st.last
                    .as_ref()?
                    .processes
                    .as_ref()?
                    .entries
                    .iter()
                    .find(|p| p.id.pid == pid && p.id.start_ticks == start)
                    .cloned()
            });
            let parent = p.as_ref().and_then(|p| {
                st.last
                    .as_ref()?
                    .processes
                    .as_ref()?
                    .entries
                    .iter()
                    .find(|q| q.id.pid == p.ppid)
                    .map(|q| format!("{} ({})", safe(&q.name), q.id.pid))
            });
            match p {
                Some(p) => {
                    let io = match p.disk_io.live() {
                        Some(d) => format!(
                            "{} read · {} written",
                            bytes_rate(d.read_bytes_per_s),
                            bytes_rate(d.write_bytes_per_s)
                        ),
                        None => format!("— ({})", p.disk_io.status.label()),
                    };
                    (
                        format!("<b>{}</b>  PID {}", esc(&safe(&p.name)), p.id.pid),
                        vec![
                            (
                                "State",
                                format!(
                                    "{} · user {}",
                                    state_name(p.state),
                                    safe(&p.user.clone().unwrap_or_else(|| {
                                        p.uid.map_or("?".into(), |u| u.to_string())
                                    }))
                                ),
                            ),
                            ("Parent", parent.unwrap_or_else(|| p.ppid.to_string())),
                            (
                                "CPU",
                                format!(
                                    "{} now · {} total CPU time",
                                    p.cpu_pct.live().map_or("—".into(), |c| format!("{c:.1}%")),
                                    units::duration_s(p.cpu_time_s)
                                ),
                            ),
                            (
                                "Memory",
                                format!(
                                    "{} resident · {} virtual",
                                    units::bytes(p.rss_bytes as f64),
                                    units::bytes(p.virtual_bytes as f64)
                                ),
                            ),
                            ("Threads", p.threads.to_string()),
                            ("Disk", io),
                        ],
                    )
                }
                None => (String::new(), Vec::new()),
            }
        };
        let mut rows: Vec<(&str, String)> = live_lines;
        let failed = match &r {
            Ok(d) => {
                rows.push((
                    "Open file descriptors (files, sockets, pipes)",
                    match (&d.open_fds, &d.fd_limit) {
                        (Ok(n), Ok(u64::MAX)) => format!("{n} (no limit)"),
                        (Ok(n), Ok(l)) => format!("{n} of {l} allowed"),
                        (Ok(n), Err(_)) => n.to_string(),
                        (Err(e), _) => reason(e),
                    },
                ));
                rows.push((
                    "Swap used",
                    match &d.swap_bytes {
                        Ok(b) => units::bytes(*b as f64),
                        Err(e) => reason(e),
                    },
                ));
                rows.push((
                    "Belongs to",
                    d.cgroup
                        .as_ref()
                        .ok()
                        .filter(|c| !c.is_empty() && c.as_str() != "/")
                        .map_or("—".to_string(), |c| {
                            let (k, n) = nysm_collect::linux::parse::cgroup_classify(
                                c.trim_start_matches('/'),
                            );
                            format!("{} {}", k.label(), safe(&n))
                        }),
                ));
                rows.push(("Executable", show(&d.exe)));
                rows.push(("Working directory", show(&d.cwd)));
                rows.push(("Cgroup", show(&d.cgroup)));
                None
            }
            Err(e) => {
                // Gone or unreadable: stop refreshing.
                self.proc_state.borrow_mut().details_for = None;
                Some(format!("Details unavailable: {e}"))
            }
        };
        let mut markup = String::new();
        if !proc_line.is_empty() {
            markup.push_str(&proc_line);
            markup.push_str("\n\n");
        }
        if let Some(f) = failed {
            markup.push_str(&esc(&f));
            markup.push_str("\n\n");
        }
        for (k, v) in &rows {
            markup.push_str(&format!(
                "<span size=\"small\" alpha=\"60%\">{}</span>\n{}\n\n",
                esc(k),
                esc(v)
            ));
        }
        markup.push_str("<span size=\"small\" alpha=\"60%\">Command line is hidden by default (nysm inspect --show-args).</span>");
        if self.proc_state.borrow().details_for.is_some() {
            let now = gtk::glib::DateTime::now_local()
                .ok()
                .and_then(|t| t.format("%H:%M:%S").ok())
                .map_or(String::new(), |t| format!(" · updated {t}"));
            markup.push_str(&format!(
                "\n<span size=\"small\" alpha=\"60%\">Live{} (every second while this page is open)</span>",
                esc(&now)
            ));
        }
        self.proc_details.set_markup(&markup);
    }

    pub fn visible_page(&self) -> String {
        self.stack
            .visible_child_name()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// Show the Processes page and focus its filter.
    pub fn focus_process_search(&self) {
        self.stack.set_visible_child_name("processes");
        self.proc_search.grab_focus();
    }

    pub fn set_source(&self, text: &str) {
        self.source.set_text(text);
    }

    /// Re-render the groups page after a filter/sort change.
    pub fn groups_controls(&self) -> (gtk::DropDown, gtk::DropDown) {
        (self.groups_kind.clone(), self.groups_sort.clone())
    }

    pub fn set_window_s(&self, s: f64) {
        self.window_s.set(s);
    }

    pub fn update(
        &self,
        s: &Arc<Snapshot>,
        history: &[HistoryPoint],
        alerts: &[ActiveAlert],
        pinned: &[nysm_core::history::PinnedProcess],
        visible: &str,
    ) {
        self.record_cores(s);
        // Restrict charts to the selected time window.
        let newest = history.last().map_or(0, |p| p.timestamp_ms);
        let cutoff = newest - (self.window_s.get() * 1000.0) as i64;
        let start = history
            .iter()
            .position(|p| p.timestamp_ms >= cutoff)
            .unwrap_or(0);
        let h = &history[start..];
        let g = gaps(h);
        let span = span_s(h);
        let rate = |v: f64| units::rate(v, self.rate);
        let (c, m) = (&s.cpu, &s.memory);
        let cores = c.logical_cores.value.unwrap_or(0);
        let cpu_series = || {
            vec![Series {
                color: chart::CPU,
                values: series(h, |p| p.cpu_pct),
            }]
        };
        let mem_series = || {
            vec![Series {
                color: chart::MEM,
                values: series(h, |p| p.mem_used_pct),
            }]
        };
        let net_series = || {
            vec![
                Series {
                    color: chart::NET_RX,
                    values: series(h, |p| p.net_rx_bytes_per_s),
                },
                Series {
                    color: chart::NET_TX,
                    values: series(h, |p| p.net_tx_bytes_per_s),
                },
            ]
        };
        let disk_series = || {
            vec![
                Series {
                    color: chart::DISK_R,
                    values: series(h, |p| p.disk_read_bytes_per_s),
                },
                Series {
                    color: chart::DISK_W,
                    values: series(h, |p| p.disk_write_bytes_per_s),
                },
            ]
        };
        let span_text = format!("last {}", units::duration_s(span));

        if visible == "overview" {
            let firing: Vec<String> = alerts
                .iter()
                .filter(|a| matches!(a.state, AlertState::Firing { .. }))
                .map(|a| {
                    let t = a
                        .target
                        .as_deref()
                        .map(|t| format!(" {}", safe(t)))
                        .unwrap_or_default();
                    match a.value {
                        Some(v) => format!("{}{t} {v:.1}{}", a.metric.label(), a.metric.unit()),
                        None => format!("{}{t}", a.metric.label()),
                    }
                })
                .collect();
            self.alert_banner.set_visible(!firing.is_empty());
            self.alert_banner
                .set_text(&format!("Alert: {}", firing.join(" · ")));

            // CPU card
            self.cpu_value.set_text(
                &c.usage
                    .live()
                    .map_or_else(|| missing(&c.usage), |u| format!("{:.0}%", u.total_pct)),
            );
            let freqs: Vec<f64> = c
                .per_core
                .iter()
                .filter_map(|k| k.frequency_mhz.live().copied())
                .collect();
            let freq = if freqs.is_empty() {
                String::new()
            } else {
                format!(
                    "{:.1} GHz avg",
                    freqs.iter().sum::<f64>() / freqs.len() as f64 / 1000.0
                )
            };
            let temp = s
                .sensors
                .value
                .as_ref()
                .and_then(|v| v.cpu_temperature())
                .map(|t| format!("{:.0} °C", t.celsius));
            let sub: Vec<String> = [Some(freq), temp]
                .into_iter()
                .flatten()
                .filter(|x| !x.is_empty())
                .collect();
            self.cpu_sub.set_text(&sub.join(" · "));
            self.cpu_detail.set_text(&format!(
                "{cores} logical cores · load {} (queue length) · {}",
                c.load
                    .live()
                    .map_or("—".into(), |l| format!("{:.2}", l.one)),
                psi_text(&c.pressure)
            ));
            self.cpu_chart.set(cpu_series(), g.clone(), span);
            self.cpu_chart.set_summary(&format!(
                "CPU usage {} now, {span_text}",
                self.cpu_value.text()
            ));

            // Memory card
            match m.usage.live() {
                Some(u) => {
                    self.mem_value.set_text(&format!(
                        "{} / {}",
                        units::bytes(u.used_bytes as f64),
                        units::bytes(u.total_bytes as f64)
                    ));
                    self.mem_pct.set_text(&format!("{:.0}%", u.used_pct));
                    self.mem_detail.set_text(&format!(
                        "{} available · swap {} · {}",
                        units::bytes(u.available_bytes as f64),
                        m.swap
                            .live()
                            .map_or("—".into(), |w| units::bytes(w.used_bytes as f64)),
                        psi_text(&m.pressure)
                    ));
                }
                None => {
                    self.mem_value.set_text(&missing(&m.usage));
                    self.mem_pct.set_text("");
                }
            }
            self.mem_mini.set(mem_series(), g.clone(), span);
            self.mem_mini
                .set_summary(&format!("Memory used {}, {span_text}", self.mem_pct.text()));

            // Network card
            match s.network.total.live() {
                Some(n) => {
                    self.net_rx.set_text(&rate(n.rx_bytes_per_s));
                    self.net_tx.set_text(&rate(n.tx_bytes_per_s));
                }
                None => {
                    self.net_rx.set_text(&missing(&s.network.total));
                    self.net_tx.set_text("");
                }
            }
            self.net_ov_chart.set(net_series(), g.clone(), span);
            self.net_ov_chart.set_summary(&format!(
                "Network download {}, upload {}, {span_text}",
                self.net_rx.text(),
                self.net_tx.text()
            ));

            // Storage card: the root filesystem (or the largest one).
            let fs = s.storage.filesystems.value.as_ref().and_then(|v| {
                v.iter()
                    .find(|f| f.mount_point == "/")
                    .or_else(|| v.iter().max_by_key(|f| f.total_bytes))
            });
            match fs {
                Some(f) => {
                    self.fs_value.set_text(&format!(
                        "{} / {}",
                        units::bytes(f.used_bytes as f64),
                        units::bytes(f.total_bytes as f64)
                    ));
                    self.fs_pct.set_text(&format!("{:.0}%", f.used_pct));
                    self.fs_bar.set_value(f.used_pct);
                    let others = s
                        .storage
                        .filesystems
                        .value
                        .as_ref()
                        .map_or(0, |v| v.len().saturating_sub(1));
                    let stale = if s.storage.filesystems.status == Status::Stale {
                        " · stale"
                    } else {
                        ""
                    };
                    self.fs_detail.set_text(&format!(
                        "{} ({}) · {} available{}{stale}",
                        safe(&f.mount_point),
                        safe(&f.fs_type),
                        units::bytes(f.available_bytes as f64),
                        if others > 0 {
                            format!(" · {others} more in Storage")
                        } else {
                            String::new()
                        }
                    ));
                }
                None => {
                    self.fs_value.set_text(&missing(&s.storage.filesystems));
                    self.fs_pct.set_text("");
                    self.fs_bar.set_value(0.0);
                }
            }

            // Disk card
            match s.storage.total_io.live() {
                Some(d) => {
                    self.disk_r.set_text(&bytes_rate(d.read_bytes_per_s));
                    self.disk_w.set_text(&bytes_rate(d.write_bytes_per_s));
                }
                None => {
                    self.disk_r.set_text(&missing(&s.storage.total_io));
                    self.disk_w.set_text("");
                }
            }
            self.disk_ov_chart.set(disk_series(), g.clone(), span);
            self.disk_ov_chart.set_summary(&format!(
                "Disk read {}, write {}, {span_text}",
                self.disk_r.text(),
                self.disk_w.text()
            ));

            // Top processes
            if let Some(t) = &s.processes {
                let order = query::view(&t.entries, ProcessSort::Cpu, "");
                let mut n = 0;
                for (k, i) in order.into_iter().take(TOP_ROWS).enumerate() {
                    let p = &t.entries[i];
                    let (r, w) = match p.disk_io.live() {
                        Some(d) => (
                            bytes_rate(d.read_bytes_per_s),
                            bytes_rate(d.write_bytes_per_s),
                        ),
                        None => ("—".into(), "—".into()),
                    };
                    set_row(
                        &self.top,
                        k,
                        &[
                            (safe(&p.name), 0, 0.0),
                            (
                                p.cpu_pct.live().map_or("—".into(), |c| format!("{c:.1}%")),
                                7,
                                1.0,
                            ),
                            (units::bytes(p.rss_bytes as f64), 9, 1.0),
                            (r, 10, 1.0),
                            (w, 10, 1.0),
                        ],
                    );
                    n += 1;
                }
                hide_rows_from(&self.top, n);
            }
        }

        if visible == "processes" {
            self.update_watch(pinned);
            let ts = s.processes.as_ref().map(|t| t.timestamp_ms);
            let changed = self.proc_state.borrow().rendered_ts != ts;
            self.proc_state.borrow_mut().last = Some(s.clone());
            if changed {
                let prev = std::mem::take(&mut self.proc_state.borrow_mut().rows);
                let shown = self.proc_state.borrow().details_for;
                let rows = render_processes(
                    s,
                    &self.proc_table,
                    &self.proc_count,
                    &self.proc_search.text(),
                    sort_key(self.proc_sort.selected()),
                    &prev,
                    shown,
                );
                let mut st = self.proc_state.borrow_mut();
                st.rows = rows;
                st.rendered_ts = ts;
            }
        }

        if visible == "cpu" {
            let t = &self.cpu_stats;
            match c.usage.live() {
                Some(u) => {
                    let p = |v: f64| format!("{v:.1}%");
                    t[0].set(&p(u.total_pct), &format!("of {cores} logical cores"));
                    t[1].set(&p(u.user_pct), "");
                    t[2].set(&p(u.system_pct), "");
                    t[3].set(&p(u.nice_pct), "");
                    t[4].set(&p(u.irq_pct), "");
                    t[5].set(&p(u.iowait_pct), "not busy");
                    t[6].set(&p(u.steal_pct), "");
                }
                None => {
                    for x in &t[..7] {
                        x.set("—", c.usage.status.label());
                    }
                }
            }
            match c.load.live() {
                Some(l) => t[7].set(
                    &format!("{:.2}", l.one),
                    &format!("5 min {:.2} · 15 min {:.2}", l.five, l.fifteen),
                ),
                None => t[7].set("—", c.load.status.label()),
            }
            let (pv, ph) = psi_tile(&c.pressure);
            t[8].set(&pv, &ph);
            self.cpu_page_chart.set(cpu_series(), g.clone(), span);
            self.cpu_page_chart
                .set_summary(&format!("CPU usage history, {span_text}"));
            self.update_cores(s);
        }

        if visible == "memory" {
            let t = &self.mem_stats;
            let b = |v: u64| units::bytes(v as f64);
            let o = |v: Option<u64>| v.map_or("—".into(), b);
            match m.usage.live() {
                Some(u) => {
                    t[0].set(
                        &b(u.used_bytes),
                        &format!("{:.1}% of {}", u.used_pct, b(u.total_bytes)),
                    );
                    t[1].set(&b(u.available_bytes), "incl. reclaimable cache");
                    t[2].set(&o(u.cache_bytes), "");
                    t[3].set(&o(u.reclaimable_slab_bytes), "");
                    t[4].set(&o(u.shared_bytes), "");
                    t[5].set(&o(u.dirty_bytes), "");
                }
                None => {
                    for x in &t[..6] {
                        x.set("—", m.usage.status.label());
                    }
                }
            }
            match m.swap.live() {
                Some(w) => t[6].set(&b(w.used_bytes), &format!("of {}", b(w.total_bytes))),
                None => t[6].set("—", m.swap.status.label()),
            }
            match m.swap_activity.live() {
                Some(a) => t[7].set(
                    &format!("{} in", bytes_rate(a.in_bytes_per_s)),
                    &format!("{} out", bytes_rate(a.out_bytes_per_s)),
                ),
                None => t[7].set("—", m.swap_activity.status.label()),
            }
            let (pv, ph) = psi_tile(&m.pressure);
            t[8].set(&pv, &ph);
            self.mem_chart.set(mem_series(), g.clone(), span);
            self.mem_chart
                .set_summary(&format!("Memory used history, {span_text}"));
            self.mem_psi_chart.set(
                vec![Series {
                    color: chart::MEM,
                    values: series(h, |p| p.mem_pressure_some_pct),
                }],
                g.clone(),
                span,
            );
            self.mem_psi_chart.set_summary(&format!(
                "Memory pressure history, {}",
                psi_text(&m.pressure)
            ));
        }

        if visible == "network" {
            self.net_chart.set(net_series(), g.clone(), span);
            self.net_chart
                .set_summary(&format!("Network throughput history, {span_text}"));
            let mut ifs: Vec<_> = s
                .network
                .interfaces
                .live()
                .map(|v| v.iter().collect())
                .unwrap_or_default();
            ifs.sort_by_key(|i| (!i.counted_in_total, i.up != Some(true), i.name.clone()));
            set_row(
                &self.net_list,
                0,
                &[
                    ("INTERFACE".into(), 16, 0.0),
                    ("KIND".into(), 11, 0.0),
                    ("STATE".into(), 6, 0.0),
                    ("IN TOTAL".into(), 8, 0.0),
                    ("DOWNLOAD".into(), 12, 1.0),
                    ("UPLOAD".into(), 12, 1.0),
                    ("".into(), 0, 0.0),
                ],
            );
            for (k, i) in ifs.iter().enumerate() {
                let (r, t) = i.rates.live().map_or(("—".into(), "—".into()), |x| {
                    (rate(x.rx_bytes_per_s), rate(x.tx_bytes_per_s))
                });
                let state = match i.up {
                    Some(true) => "up",
                    Some(false) => "down",
                    None => "?",
                };
                set_row(
                    &self.net_list,
                    k + 1,
                    &[
                        (safe(&i.name), 16, 0.0),
                        (i.kind.label().into(), 11, 0.0),
                        (state.into(), 6, 0.0),
                        ((if i.counted_in_total { "yes" } else { "" }).into(), 8, 0.0),
                        (r, 12, 1.0),
                        (t, 12, 1.0),
                        (String::new(), 0, 0.0),
                    ],
                );
            }
            hide_rows_from(&self.net_list, ifs.len() + 1);
        }

        if visible == "groups" {
            self.update_groups(s);
        }

        if visible == "storage" {
            self.disk_info.set_text(&format!(
                "read {} · write {} · I/O {}",
                s.storage
                    .total_io
                    .live()
                    .map_or("—".into(), |d| bytes_rate(d.read_bytes_per_s)),
                s.storage
                    .total_io
                    .live()
                    .map_or("—".into(), |d| bytes_rate(d.write_bytes_per_s)),
                psi_text(&s.storage.io_pressure)
            ));
            self.disk_chart.set(disk_series(), g, span);
            self.disk_chart
                .set_summary(&format!("Disk throughput history, {span_text}"));
            self.update_volumes(s);
            self.update_devices(s);
        }
    }

    fn update_groups(&self, s: &Snapshot) {
        use nysm_core::raw::CgroupKind as K;
        let Some(t) = &s.cgroups else {
            self.groups_count.set_text("Collecting cgroup accounting…");
            hide_rows_from(&self.groups_list, 0);
            return;
        };
        let want = match self.groups_kind.selected() {
            1 => Some(K::Container),
            2 => Some(K::Service),
            3 => Some(K::UserApp),
            _ => None,
        };
        let mut rows: Vec<&nysm_core::snapshot::CgroupSnapshot> = t
            .groups
            .iter()
            .filter(|g| want.is_none_or(|k| g.kind == k))
            .collect();
        let key = match self.groups_sort.selected() {
            1 => ProcessSort::Memory,
            2 => ProcessSort::DiskIo,
            3 => ProcessSort::Name,
            _ => ProcessSort::Cpu,
        };
        query::sort_groups(&mut rows, key);
        let names = self.names.poll();
        self.groups_count.set_text(&format!(
            "{} groups · CPU % of all {} cores{}",
            rows.len(),
            t.logical_cores,
            self.names.note()
        ));
        set_row(
            &self.groups_list,
            0,
            &[
                ("KIND".into(), 9, 0.0),
                ("NAME".into(), 0, 0.0),
                ("CPU %".into(), 6, 1.0),
                ("LIMIT".into(), 6, 1.0),
                ("MEMORY / LIMIT".into(), 16, 1.0),
                ("PIDS".into(), 5, 1.0),
                ("READ/s".into(), 9, 1.0),
                ("WRITE/s".into(), 9, 1.0),
                ("MEM PSI".into(), 7, 1.0),
                ("MAIN".into(), 12, 0.0),
            ],
        );
        let n = rows.len().min(MAX_LIST_ROWS);
        for (k, g) in rows.iter().take(n).enumerate() {
            let mem = match (g.memory_bytes, g.memory_max_bytes) {
                (Some(v), Some(m)) => {
                    format!("{} / {}", units::bytes(v as f64), units::bytes(m as f64))
                }
                (Some(v), None) => units::bytes(v as f64),
                _ => "—".into(),
            };
            let (r, w) = g.disk_io.live().map_or(("—".into(), "—".into()), |d| {
                (
                    bytes_rate(d.read_bytes_per_s),
                    bytes_rate(d.write_bytes_per_s),
                )
            });
            set_row(
                &self.groups_list,
                k + 1,
                &[
                    (g.kind.label().into(), 9, 0.0),
                    (
                        safe(&match nysm_collect::runtime::container_id(&g.path)
                            .and_then(|id| names.get(id).map(|c| (id, c)))
                        {
                            Some((id, c)) => nysm_collect::runtime::label(&c.name, id),
                            None => g.name.clone(),
                        }),
                        0,
                        0.0,
                    ),
                    (
                        g.cpu_pct.live().map_or("—".into(), |c| format!("{c:.1}")),
                        6,
                        1.0,
                    ),
                    (
                        g.cpu_limit_cores
                            .map_or(String::new(), |c| format!("{c:.2}c")),
                        6,
                        1.0,
                    ),
                    (mem, 16, 1.0),
                    (g.pids.map_or("—".into(), |p| p.to_string()), 5, 1.0),
                    (r, 10, 1.0),
                    (w, 10, 1.0),
                    (
                        g.memory_pressure_pct
                            .live()
                            .map_or("—".into(), |v| format!("{v:.1}%")),
                        7,
                        1.0,
                    ),
                    (safe(g.main_process.as_deref().unwrap_or("")), 12, 0.0),
                ],
            );
        }
        hide_rows_from(&self.groups_list, n + 1);
    }

    /// Per-core history is kept here for every new snapshot (the shared
    /// history only has totals); tiles are drawn when the page is visible.
    fn record_cores(&self, s: &Snapshot) {
        if s.seq == self.core_seq.get() {
            return;
        }
        self.core_seq.set(s.seq);
        let per_core = &s.cpu.per_core;
        let mut tiles = self.core_tiles.borrow_mut();
        if tiles.len() != per_core.len() {
            while let Some(ch) = self.cpu_cores.first_child() {
                self.cpu_cores.remove(&ch);
            }
            tiles.clear();
            for _ in per_core {
                let (w, t) = CoreTile::new();
                self.cpu_cores.insert(&w, -1);
                tiles.push(t);
            }
        }
        for (t, core) in tiles.iter_mut().zip(per_core) {
            t.push(core.usage_pct.live().copied());
        }
    }

    fn update_cores(&self, s: &Snapshot) {
        let span =
            stats::CORE_HISTORY as f64 * s.interval_ms.unwrap_or(1000).max(1) as f64 / 1000.0;
        for (t, core) in self.core_tiles.borrow().iter().zip(&s.cpu.per_core) {
            t.show(
                core.id,
                core.usage_pct.live().copied(),
                core.frequency_mhz.live().copied(),
                span,
            );
        }
    }

    fn update_volumes(&self, s: &Snapshot) {
        clear(&self.fs_list);
        let Some(fss) = s.storage.filesystems.value.as_ref() else {
            self.fs_list
                .append(&label(&missing(&s.storage.filesystems), &["dim-label"]));
            return;
        };
        for f in fss {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            b.set_margin_top(6);
            b.set_margin_bottom(6);
            b.set_margin_start(6);
            let name = label(&safe(&f.mount_point), &[]);
            name.set_width_chars(18);
            name.set_wrap(false);
            name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            b.append(&name);
            let lb = gtk::LevelBar::for_interval(0.0, 100.0);
            lb.add_css_class("capacity");
            lb.set_value(f.used_pct);
            lb.set_hexpand(true);
            lb.set_valign(gtk::Align::Center);
            b.append(&lb);
            let detail = label(
                &format!(
                    "{} / {} ({:.0}%) · {} free · {}{}{}",
                    units::bytes(f.used_bytes as f64),
                    units::bytes(f.total_bytes as f64),
                    f.used_pct,
                    units::bytes(f.available_bytes as f64),
                    safe(&f.fs_type),
                    if f.read_only { " · read-only" } else { "" },
                    match (f.growth_bytes_per_hour, f.full_in_hours) {
                        (Some(g), Some(h)) if g > 1024.0 * 1024.0 && h < 24.0 * 14.0 => {
                            format!(
                                " · +{}/h, full in ~{} at this rate",
                                units::bytes(g),
                                units::duration_s(h * 3600.0)
                            )
                        }
                        (Some(g), _) if g.abs() >= 1024.0 * 1024.0 => {
                            format!(
                                " · {}{}/h",
                                if g > 0.0 { "+" } else { "-" },
                                units::bytes(g.abs())
                            )
                        }
                        _ => String::new(),
                    }
                ),
                &["numeric"],
            );
            detail.set_wrap(false);
            detail.set_xalign(1.0);
            detail.set_width_chars(46);
            b.append(&detail);
            self.fs_list.append(&b);
        }
    }

    fn update_devices(&self, s: &Snapshot) {
        set_row(
            &self.disk_list,
            0,
            &[
                ("DEVICE".into(), 12, 0.0),
                ("KIND".into(), 9, 0.0),
                ("READ/s".into(), 11, 1.0),
                ("WRITE/s".into(), 11, 1.0),
                ("LATENCY r/w ms".into(), 15, 1.0),
                ("IN-FLIGHT".into(), 9, 1.0),
                ("".into(), 0, 0.0),
            ],
        );
        let mut k = 1;
        if let Some(devs) = s.storage.devices.live() {
            for d in devs.iter().filter(|d| {
                !matches!(d.kind, DiskKind::Loop | DiskKind::Memory)
                    || d.io
                        .live()
                        .is_some_and(|io| io.read_ops_per_s + io.write_ops_per_s > 0.0)
            }) {
                let ms = |v: Option<f64>| v.map_or("—".into(), |v| format!("{v:.2}"));
                let (r, w, lat, busy) = match d.io.live() {
                    Some(io) => (
                        bytes_rate(io.read_bytes_per_s),
                        bytes_rate(io.write_bytes_per_s),
                        format!("{}/{}", ms(io.read_latency_ms), ms(io.write_latency_ms)),
                        io.busy_pct.map_or("—".into(), |b| format!("{b:.0}%")),
                    ),
                    None => (missing(&d.io), String::new(), String::new(), String::new()),
                };
                set_row(
                    &self.disk_list,
                    k,
                    &[
                        (safe(&d.name), 12, 0.0),
                        (format!("{:?}", d.kind).to_lowercase(), 9, 0.0),
                        (r, 11, 1.0),
                        (w, 11, 1.0),
                        (lat, 15, 1.0),
                        (busy, 9, 1.0),
                        (String::new(), 0, 0.0),
                    ],
                );
                k += 1;
            }
        }
        hide_rows_from(&self.disk_list, k);
    }
}

fn sort_key(i: u32) -> ProcessSort {
    match i {
        1 => ProcessSort::Memory,
        2 => ProcessSort::DiskIo,
        3 => ProcessSort::Name,
        4 => ProcessSort::Pid,
        _ => ProcessSort::Cpu,
    }
}

fn render_processes(
    s: &Snapshot,
    table: &ProcTable,
    count: &gtk::Label,
    filter: &str,
    key: ProcessSort,
    prev: &[(u32, u64, String)],
    shown: Option<(u32, u64)>,
) -> RowIds {
    let Some(t) = &s.processes else {
        count.set_text("process list not available");
        table.set_rows(Vec::new());
        return Vec::new();
    };
    let order = query::view(&t.entries, key, filter);
    count.set_text(&format!("{} of {} shown", order.len(), t.entries.len()));
    let mut ids = Vec::with_capacity(order.len());
    let mut rows: Vec<proctable::Cells> = Vec::with_capacity(order.len());
    for i in order {
        let p = &t.entries[i];
        let (r, w) = match p.disk_io.live() {
            Some(d) => (
                bytes_rate(d.read_bytes_per_s),
                bytes_rate(d.write_bytes_per_s),
            ),
            None if p.disk_io.status == Status::PermissionDenied => {
                ("denied".into(), "denied".into())
            }
            None => ("—".into(), "—".into()),
        };
        rows.push([
            safe(&p.name),
            p.id.pid.to_string(),
            safe(
                &p.user
                    .clone()
                    .unwrap_or_else(|| p.uid.map_or("?".into(), |u| u.to_string())),
            ),
            p.cpu_pct.live().map_or("—".into(), |c| format!("{c:.1}")),
            units::bytes(p.rss_bytes as f64),
            r,
            w,
        ]);
        ids.push((p.id.pid, p.id.start_ticks, p.name.clone()));
    }
    // Rows are re-sorted on every refresh; keep the selection on the same
    // processes rather than the same positions.
    let mut selected: Vec<(u32, u64)> = table
        .selected_rows()
        .into_iter()
        .filter_map(|i| prev.get(i).map(|r| (r.0, r.1)))
        .collect();
    // With no other selection, highlight the process shown in the details
    // panel whenever it is in the list (e.g. after a filter is cleared).
    if selected.is_empty()
        && let Some(id) = shown
    {
        selected.push(id);
    }
    table.set_rows(rows);
    if !selected.is_empty() {
        let keep: Vec<usize> = ids
            .iter()
            .enumerate()
            .filter(|(_, r)| selected.contains(&(r.0, r.1)))
            .map(|(i, _)| i)
            .collect();
        table.select_rows(&keep);
    }
    ids
}

type NameMap = std::collections::HashMap<String, nysm_collect::runtime::ContainerInfo>;

/// Opt-in container names from the runtime API, fetched off the UI thread
/// at most every 30 s while the groups page is shown.
struct ContainerNames {
    switch: gtk::Switch,
    map: RefCell<Rc<NameMap>>,
    error: RefCell<Option<String>>,
    result: Arc<std::sync::Mutex<Option<nysm_collect::CResult<NameMap>>>>,
    in_flight: std::cell::Cell<bool>,
    fetched: std::cell::Cell<Option<std::time::Instant>>,
}

impl ContainerNames {
    fn new(switch: gtk::Switch) -> Self {
        let n = ContainerNames {
            switch: switch.clone(),
            map: RefCell::new(Rc::new(NameMap::new())),
            error: RefCell::new(None),
            result: Arc::default(),
            in_flight: std::cell::Cell::new(false),
            fetched: std::cell::Cell::new(None),
        };
        switch.connect_active_notify(|sw| {
            let v = if sw.is_active() { "true" } else { "false" };
            if let Some(p) = nysm_config::default_path()
                && let Err(e) = nysm_config::set_many(&p, &[("display.container_names", v)])
            {
                eprintln!("nysm-desktop: setting not saved: {e}");
            }
        });
        n
    }

    /// Current names (empty when off); starts a refresh when due.
    fn poll(&self) -> Rc<NameMap> {
        if !self.switch.is_active() {
            self.fetched.set(None);
            *self.map.borrow_mut() = Rc::new(NameMap::new());
            *self.error.borrow_mut() = None;
            return self.map.borrow().clone();
        }
        if let Some(r) = self.result.lock().ok().and_then(|mut g| g.take()) {
            self.in_flight.set(false);
            match r {
                Ok(m) => {
                    *self.map.borrow_mut() = Rc::new(m);
                    *self.error.borrow_mut() = None;
                }
                Err(e) => *self.error.borrow_mut() = Some(e.reason()),
            }
        }
        let due = self
            .fetched
            .get()
            .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(30));
        if due && !self.in_flight.get() {
            self.in_flight.set(true);
            self.fetched.set(Some(std::time::Instant::now()));
            let slot = self.result.clone();
            std::thread::spawn(move || {
                let r = nysm_collect::runtime::container_names(std::time::Duration::from_secs(3));
                if let Ok(mut g) = slot.lock() {
                    *g = Some(r);
                }
            });
        }
        self.map.borrow().clone()
    }

    fn note(&self) -> String {
        match &*self.error.borrow() {
            Some(e) if self.switch.is_active() => format!(" · names unavailable: {e}"),
            _ => String::new(),
        }
    }
}
