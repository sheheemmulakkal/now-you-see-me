//! Terminal UI. Data comes from a `Source`: an attached collector service
//! or an embedded collector thread. This thread only handles input and
//! renders, at a capped frame rate independent of the collection interval.

pub mod app;
pub mod ui;

use std::io::{self, IsTerminal};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nysm_core::units::RateUnit;
use nysm_engine::EngineConfig;
use nysm_ipc::source::{Attach, Source};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

use app::{Action, App};

pub struct Options {
    pub engine: EngineConfig,
    /// Alert rules evaluated by an embedded collector (an attached service
    /// uses its own configuration).
    pub rules: Vec<nysm_core::alerts::Rule>,
    pub ascii: bool,
    pub max_fps: u32,
    pub rate: RateUnit,
    pub attach: Attach,
    /// Monitor a remote machine through this command (e.g. ssh … stdio).
    pub remote: Option<std::process::Command>,
    /// Ask Docker/Podman for container names while the Groups view is
    /// open (`display.container_names`; the socket is root-equivalent).
    pub container_names: bool,
}

pub fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
}

pub fn run(mut opts: Options) -> io::Result<()> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err(io::Error::other(
            "the TUI needs an interactive terminal; use `nysm watch` or `nysm summary` for redirected output",
        ));
    }
    let mut engine = opts.engine.clone();
    engine.processes = true;
    let client = concat!("nysm-tui/", env!("CARGO_PKG_VERSION"));
    let remote_mode = opts.remote.is_some();
    let source = match opts.remote.take() {
        Some(cmd) => Source::command(cmd, client, true, false)
            .map_err(|e| io::Error::other(format!("cannot reach the remote collector: {e}")))?,
        None => Source::open(opts.attach, client, engine, opts.rules.clone()).map_err(|e| {
            io::Error::other(format!("cannot attach to the collector service: {e}"))
        })?,
    };
    let mut app = App::new(opts.ascii, color_enabled(), opts.rate);
    app.source = source.describe();
    // try_init installs a panic hook that restores the terminal first.
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, source, &mut app, &opts, remote_mode);
    ratatui::restore();
    result
}

type DetailsRx = mpsc::Receiver<nysm_collect::CResult<nysm_collect::ProcessDetails>>;

fn event_loop(
    terminal: &mut DefaultTerminal,
    mut source: Source,
    app: &mut App,
    opts: &Options,
    remote_mode: bool,
) -> io::Result<()> {
    let frame_min = Duration::from_secs_f64(1.0 / opts.max_fps.max(1) as f64);
    let mut last_seq = 0;
    let mut last_snapshot_at = Instant::now();
    let interval = opts.engine.interval;
    let mut dirty = true;
    let mut last_draw = Instant::now() - frame_min;
    let mut pending: Option<DetailsRx> = None;
    let mut cgroups_on = false;
    // Container names: one read-only request every 30 s while Groups is open.
    let mut names_rx: Option<mpsc::Receiver<nysm_collect::CResult<app::NameMap>>> = None;
    let mut names_at: Option<Instant> = None;
    loop {
        if opts.container_names && !remote_mode && app.tab == app::Tab::Groups {
            if let Some(rx) = &names_rx
                && let Ok(r) = rx.try_recv()
            {
                if let Ok(m) = r {
                    app.container_names = m;
                    dirty = true;
                }
                names_rx = None;
            }
            if names_rx.is_none() && names_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
            {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(nysm_collect::runtime::container_names(Duration::from_secs(
                        3,
                    )));
                });
                names_rx = Some(rx);
                names_at = Some(Instant::now());
            }
        }
        // Only pay for cgroup scans while the Groups view is open.
        let want_cg = app.tab == app::Tab::Groups;
        if want_cg != cgroups_on {
            source.set_cgroups(want_cg);
            cgroups_on = want_cg;
        }
        if remote_mode && !source.is_connected() {
            // Never substitute local data for a remote machine.
            if app.notice.is_none() {
                app.notice =
                    Some("remote connection lost — data shown is stale (q to quit)".into());
                dirty = true;
            }
        } else if !source.is_connected() {
            // The service went away: keep working with a local collector
            // rather than freezing stale values as if they were live.
            let mut engine = opts.engine.clone();
            engine.processes = true;
            source = Source::local(engine, opts.rules.clone());
            app.source = source.describe();
            app.notice = Some("collector service disconnected; now collecting locally".into());
            last_seq = 0;
            pending = None;
            cgroups_on = false;
            dirty = true;
        }
        if let Some(s) = source.latest().filter(|s| s.seq != last_seq) {
            last_seq = s.seq;
            last_snapshot_at = Instant::now();
            app.live = Some(s);
            source.with_state(|st| {
                app.live_history = st.history.iter().copied().collect();
                app.alerts = st.alerts.clone();
                app.alert_events = st.alert_events.iter().cloned().collect();
                app.pinned = st.pinned.clone();
            });
            // While paused the display is frozen; no redraw needed.
            if app.paused.is_none() {
                dirty = true;
            }
        }
        if let Some(rx) = &pending {
            match rx.try_recv() {
                Ok(r) => {
                    if let Some(d) = app.details.as_mut() {
                        d.result = Some(r);
                    }
                    pending = None;
                    dirty = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if dirty && last_draw.elapsed() >= frame_min {
            terminal.draw(|f| ui::draw(f, app))?;
            last_draw = Instant::now();
            dirty = false;
        }
        // Idle: sleep until the next snapshot is due (input still wakes us
        // immediately), instead of polling at a fixed short period.
        let wait = if dirty {
            frame_min.saturating_sub(last_draw.elapsed())
        } else if pending.is_some() {
            Duration::from_millis(20)
        } else {
            let due =
                interval.saturating_sub(last_snapshot_at.elapsed()) + Duration::from_millis(15);
            due.clamp(Duration::from_millis(15), Duration::from_secs(1))
        };
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(k) => {
                    match app.on_key(k) {
                        Action::Quit => return Ok(()),
                        Action::RequestDetails(id) => {
                            pending = Some(source.request_details(id.pid, Some(id.start_ticks)))
                        }
                        Action::Pin(id, name) => source.pin(id, name),
                        Action::Unpin(id) => source.unpin(id),
                        Action::None => {}
                    }
                    dirty = true;
                }
                Event::Resize(..) => dirty = true,
                _ => {}
            }
        }
    }
}
