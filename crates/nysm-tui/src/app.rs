//! TUI state and input handling, independent of the terminal so it can be
//! tested headlessly.

use std::sync::Arc;

use nysm_collect::{CResult, ProcessDetails};
use nysm_core::history::HistoryPoint;
use nysm_core::query::{self, ProcessSort};
use nysm_core::raw::ProcessId;
use nysm_core::snapshot::Snapshot;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Processes,
    Cpu,
    Memory,
    Network,
    Disk,
    Groups,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Overview,
        Tab::Processes,
        Tab::Cpu,
        Tab::Memory,
        Tab::Network,
        Tab::Disk,
        Tab::Groups,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Processes => "Processes",
            Tab::Cpu => "CPU",
            Tab::Memory => "Memory",
            Tab::Network => "Network",
            Tab::Disk => "Disk",
            Tab::Groups => "Groups",
        }
    }
}

/// Side effects the run loop must perform for the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    RequestDetails(ProcessId),
    Pin(ProcessId, String),
    Unpin(ProcessId),
}

pub struct Details {
    pub id: ProcessId,
    pub result: Option<CResult<ProcessDetails>>,
}

/// Container id → runtime info, for naming containers in the Groups view.
pub type NameMap = std::collections::HashMap<String, nysm_collect::runtime::ContainerInfo>;

pub struct App {
    pub tab: Tab,
    /// Runtime names for containers (empty unless enabled in the config).
    pub container_names: NameMap,
    /// Latest snapshot from the collector.
    pub live: Option<Arc<Snapshot>>,
    pub live_history: Vec<HistoryPoint>,
    /// Frozen view while paused; collection continues underneath.
    pub paused: Option<(Arc<Snapshot>, Vec<HistoryPoint>)>,
    pub help: bool,
    pub sort: ProcessSort,
    pub tree: bool,
    pub filter: String,
    pub editing_filter: bool,
    /// Selected process, tracked by identity so refreshes don't move it.
    pub selected: Option<ProcessId>,
    pub scroll: usize,
    pub details: Option<Details>,
    /// Timeline cursor: number of samples back from the newest (None = live edge).
    pub cursor: Option<usize>,
    /// Rows visible in the process table at last render (for paging).
    pub page_rows: usize,
    pub ascii: bool,
    pub color: bool,
    pub rate: nysm_core::units::RateUnit,
    /// Pending/firing alerts from the collector (live, not frozen by pause).
    pub alerts: Vec<nysm_core::alerts::ActiveAlert>,
    pub alert_events: Vec<nysm_core::alerts::AlertEvent>,
    pub pinned: Vec<nysm_core::history::PinnedProcess>,
    /// Where data comes from (embedded collector or attached service).
    pub source: String,
    /// One-off message shown in the footer (e.g. a fallback).
    pub notice: Option<String>,
    /// Groups view: kind filter (None = all) and scroll offset.
    pub group_kind: Option<nysm_core::raw::CgroupKind>,
    pub group_scroll: usize,
}

impl App {
    pub fn new(ascii: bool, color: bool, rate: nysm_core::units::RateUnit) -> Self {
        App {
            tab: Tab::Overview,
            container_names: NameMap::new(),
            live: None,
            live_history: Vec::new(),
            paused: None,
            help: false,
            sort: ProcessSort::Cpu,
            tree: false,
            filter: String::new(),
            editing_filter: false,
            selected: None,
            scroll: 0,
            details: None,
            cursor: None,
            page_rows: 10,
            ascii,
            color,
            rate,
            alerts: Vec::new(),
            alert_events: Vec::new(),
            pinned: Vec::new(),
            source: String::new(),
            notice: None,
            group_kind: None,
            group_scroll: 0,
        }
    }

    /// The snapshot being displayed (frozen when paused).
    pub fn shown(&self) -> Option<&Arc<Snapshot>> {
        self.paused.as_ref().map(|p| &p.0).or(self.live.as_ref())
    }

    pub fn history(&self) -> &[HistoryPoint] {
        self.paused
            .as_ref()
            .map(|p| p.1.as_slice())
            .unwrap_or(&self.live_history)
    }

    /// Process rows in display order: (entry index, tree depth).
    pub fn rows(&self) -> Vec<(usize, usize)> {
        let Some(t) = self.shown().and_then(|s| s.processes.clone()) else {
            return Vec::new();
        };
        if self.tree && self.filter.is_empty() {
            query::tree(&t.entries, self.sort)
                .into_iter()
                .map(|r| (r.index, r.depth))
                .collect()
        } else {
            query::view(&t.entries, self.sort, &self.filter)
                .into_iter()
                .map(|i| (i, 0))
                .collect()
        }
    }

    pub fn selected_index(&self, rows: &[(usize, usize)]) -> Option<usize> {
        let t = self.shown()?.processes.clone()?;
        let sel = self.selected.as_ref()?;
        rows.iter().position(|(i, _)| &t.entries[*i].id == sel)
    }

    fn move_selection(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            self.selected = None;
            return;
        }
        let Some(t) = self.shown().and_then(|s| s.processes.clone()) else {
            return;
        };
        let cur = self.selected_index(&rows).map(|i| i as isize).unwrap_or(-1);
        let next = (cur + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.selected = Some(t.entries[rows[next].0].id.clone());
    }

    fn select_edge(&mut self, last: bool) {
        let rows = self.rows();
        let Some(t) = self.shown().and_then(|s| s.processes.clone()) else {
            return;
        };
        let r = if last { rows.last() } else { rows.first() };
        self.selected = r.map(|(i, _)| t.entries[*i].id.clone());
    }

    pub fn toggle_pause(&mut self) {
        self.paused = match self.paused.take() {
            Some(_) => None,
            None => self.live.clone().map(|s| (s, self.live_history.clone())),
        };
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Action {
        if k.kind == KeyEventKind::Release {
            return Action::None;
        }
        self.notice = None;
        if k.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            return Action::Quit;
        }
        if self.editing_filter {
            match k.code {
                KeyCode::Esc => {
                    self.editing_filter = false;
                    self.filter.clear();
                }
                KeyCode::Enter => self.editing_filter = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) if !c.is_control() && self.filter.chars().count() < 64 => {
                    self.filter.push(c)
                }
                _ => {}
            }
            return Action::None;
        }
        if self.help {
            // Any key closes help; q still quits.
            self.help = false;
            return if k.code == KeyCode::Char('q') {
                Action::Quit
            } else {
                Action::None
            };
        }
        match k.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            KeyCode::Char(' ') | KeyCode::Char('p') => self.toggle_pause(),
            KeyCode::Tab => self.tab = Tab::ALL[(self.tab_index() + 1) % Tab::ALL.len()],
            KeyCode::BackTab => {
                self.tab = Tab::ALL[(self.tab_index() + Tab::ALL.len() - 1) % Tab::ALL.len()]
            }
            KeyCode::Char(c @ '1'..='7') => self.tab = Tab::ALL[(c as u8 - b'1') as usize],
            KeyCode::Esc => {
                if self.details.is_some() {
                    self.details = None;
                } else if self.cursor.is_some() {
                    self.cursor = None;
                } else if !self.filter.is_empty() {
                    self.filter.clear();
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.move_cursor(1),
            KeyCode::Right | KeyCode::Char('l') => self.move_cursor(-1),
            KeyCode::End => self.cursor = None,
            _ => {}
        }
        if self.tab == Tab::Groups {
            use nysm_core::raw::CgroupKind as K;
            match k.code {
                KeyCode::Char('k') => {
                    self.group_kind = match self.group_kind {
                        None => Some(K::Container),
                        Some(K::Container) => Some(K::Service),
                        Some(K::Service) => Some(K::UserApp),
                        _ => None,
                    };
                    self.group_scroll = 0;
                }
                KeyCode::Char('c') => self.sort = ProcessSort::Cpu,
                KeyCode::Char('m') => self.sort = ProcessSort::Memory,
                KeyCode::Char('d') => self.sort = ProcessSort::DiskIo,
                KeyCode::Char('n') => self.sort = ProcessSort::Name,
                KeyCode::Down | KeyCode::Char('j') => self.group_scroll += 1,
                KeyCode::Up => self.group_scroll = self.group_scroll.saturating_sub(1),
                KeyCode::PageDown => self.group_scroll += self.page_rows.max(1),
                KeyCode::PageUp => {
                    self.group_scroll = self.group_scroll.saturating_sub(self.page_rows.max(1))
                }
                KeyCode::Home | KeyCode::Char('g') => self.group_scroll = 0,
                _ => {}
            }
            return Action::None;
        }
        if matches!(self.tab, Tab::Processes | Tab::Overview) {
            match k.code {
                KeyCode::Char('/') => {
                    self.tab = Tab::Processes;
                    self.editing_filter = true;
                }
                KeyCode::Char('c') => self.sort = ProcessSort::Cpu,
                KeyCode::Char('m') => self.sort = ProcessSort::Memory,
                KeyCode::Char('d') => self.sort = ProcessSort::DiskIo,
                KeyCode::Char('n') => self.sort = ProcessSort::Name,
                KeyCode::Char('P') => self.sort = ProcessSort::Pid,
                KeyCode::Char('t') => {
                    self.tree = !self.tree;
                    self.tab = Tab::Processes;
                }
                KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
                KeyCode::PageDown => self.move_selection(self.page_rows.max(1) as isize),
                KeyCode::PageUp => self.move_selection(-(self.page_rows.max(1) as isize)),
                KeyCode::Home | KeyCode::Char('g') => self.select_edge(false),
                KeyCode::Char('G') => self.select_edge(true),
                KeyCode::Char('*') => {
                    if let Some(id) = self.selected.clone() {
                        if self.is_pinned(&id) {
                            return Action::Unpin(id);
                        }
                        let name = self
                            .shown()
                            .and_then(|s| s.processes.clone())
                            .and_then(|t| {
                                t.entries
                                    .iter()
                                    .find(|p| p.id == id)
                                    .map(|p| p.name.clone())
                            })
                            .unwrap_or_default();
                        return Action::Pin(id, name);
                    }
                }
                KeyCode::Enter => {
                    if let Some(id) = self.selected.clone() {
                        self.tab = Tab::Processes;
                        self.details = Some(Details {
                            id: id.clone(),
                            result: None,
                        });
                        return Action::RequestDetails(id);
                    }
                }
                _ => {}
            }
        }
        Action::None
    }

    pub fn is_pinned(&self, id: &ProcessId) -> bool {
        self.pinned.iter().any(|p| &p.id == id)
    }

    fn tab_index(&self) -> usize {
        Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0)
    }

    fn move_cursor(&mut self, back: isize) {
        let len = self.history().len();
        if len == 0 {
            return;
        }
        let cur = self.cursor.map(|c| c as isize).unwrap_or(0);
        let next = (cur + back).clamp(0, len as isize - 1);
        self.cursor = if next == 0 { None } else { Some(next as usize) };
    }

    /// History point under the timeline cursor.
    pub fn cursor_point(&self) -> Option<&HistoryPoint> {
        let h = self.history();
        let back = self.cursor?;
        h.len().checked_sub(1 + back).and_then(|i| h.get(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn quit_and_help() {
        let mut a = App::new(false, true, Default::default());
        assert_eq!(a.on_key(key(KeyCode::Char('?'))), Action::None);
        assert!(a.help);
        assert_eq!(a.on_key(key(KeyCode::Char('x'))), Action::None);
        assert!(!a.help);
        assert_eq!(
            a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
        assert_eq!(a.on_key(key(KeyCode::Char('q'))), Action::Quit);
    }

    #[test]
    fn tabs_cycle_and_jump() {
        let mut a = App::new(false, true, Default::default());
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.tab, Tab::Processes);
        a.on_key(key(KeyCode::BackTab));
        a.on_key(key(KeyCode::BackTab));
        assert_eq!(a.tab, Tab::Groups);
        a.on_key(key(KeyCode::Char('3')));
        assert_eq!(a.tab, Tab::Cpu);
        a.on_key(key(KeyCode::Char('7')));
        assert_eq!(a.tab, Tab::Groups);
        // k cycles the kind filter in the Groups view.
        a.on_key(key(KeyCode::Char('k')));
        assert_eq!(a.group_kind, Some(nysm_core::raw::CgroupKind::Container));
    }

    #[test]
    fn filter_editing_captures_keys() {
        let mut a = App::new(false, true, Default::default());
        a.on_key(key(KeyCode::Char('/')));
        assert!(a.editing_filter);
        for c in "q1".chars() {
            assert_eq!(a.on_key(key(KeyCode::Char(c))), Action::None);
        }
        assert_eq!(a.filter, "q1");
        assert_eq!(a.tab, Tab::Processes);
        a.on_key(key(KeyCode::Esc));
        assert!(!a.editing_filter && a.filter.is_empty());
    }

    #[test]
    fn pause_freezes_without_live_data_is_noop() {
        let mut a = App::new(false, true, Default::default());
        a.toggle_pause();
        assert!(a.paused.is_none());
    }
}
