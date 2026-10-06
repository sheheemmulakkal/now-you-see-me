//! `nysm`: Now You See Me command-line interface.

mod cmd;
mod fmt;
mod style;

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use nysm_core::units::{RateUnit, parse_duration};

use style::{ColorChoice, Style};

/// Exit codes (documented in README and `--help`).
pub mod exit {
    pub const OK: u8 = 0;
    /// Runtime failure (e.g. output could not be written, TUI failed).
    pub const FAILURE: u8 = 1;
    /// Invalid invocation (clap also uses 2).
    pub const USAGE: u8 = 2;
    /// Output produced, but a core metric (CPU or memory) failed to collect.
    pub const PARTIAL: u8 = 3;
    /// The requested target (e.g. a PID) does not exist.
    pub const NOT_FOUND: u8 = 4;
}

const AFTER_HELP: &str = "\
Examples:
  nysm summary                     one-shot overview (samples for 1s to compute rates)
  nysm summary --json              versioned machine-readable snapshot
  nysm watch --interval 1s --format jsonl --count 10
  nysm processes --sort mem --limit 15
  nysm inspect --pid 1234
  nysm ports --port 3000           who is listening on / connected to port 3000
  nysm net check example.com       DNS time + TCP connect latency (on demand)
  nysm disk usage ~/projects       largest entries (bounded scan, Ctrl-C safe)
  nysm containers                  CPU/memory/IO per container vs its limits
  nysm services --sort mem         systemd services by memory
  nysm record -o before.nysm -- cargo build --release
  nysm compare before.nysm after.nysm
    nysm tui                         interactive terminal UI
  nysm service run &               optional shared collector (TUI/desktop attach to it)
  nysm alerts                      watch sustained alert rules (see `nysm config show`)
  nysm capabilities --json         what this machine can report, and why not
  nysm doctor                      environment and permission diagnostics

Exit codes: 0 ok, 1 runtime failure, 2 invalid usage,
            3 partial result (a core metric failed), 4 target not found.
Colour: honours NO_COLOR; override with --color.";

#[derive(Parser)]
#[command(name = "nysm", version, about = "Now You See Me: a precise, lightweight resource monitor for engineers", after_help = AFTER_HELP)]
struct Cli {
    /// When to use ANSI colour in text output.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto, global = true)]
    color: ColorChoice,
    /// Show network rates in bytes/s (binary prefixes) or bits/s (decimal).
    /// Default from the config file, else bytes.
    #[arg(long, value_enum, global = true)]
    rate_unit: Option<RateArg>,
    /// Configuration file (default: $NYSM_CONFIG or the platform config dir).
    #[arg(long, global = true)]
    config: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, ValueEnum)]
enum RateArg {
    Bytes,
    Bits,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WatchFormat {
    Text,
    Jsonl,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GroupKindArg {
    Container,
    Service,
    App,
    Machine,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProtoArg {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortKey {
    Cpu,
    Mem,
    Io,
    Pid,
    Name,
}

fn duration_arg(s: &str) -> Result<Duration, String> {
    parse_duration(s).ok_or_else(|| format!("invalid duration {s:?} (examples: 500ms, 1s, 2m)"))
}

#[derive(Subcommand)]
enum Command {
    /// One-shot overview of CPU, memory, network, storage and top processes.
    Summary {
        #[arg(long)]
        json: bool,
        /// Sampling window used to compute rates (minimum 100ms).
        #[arg(long, default_value = "1s", value_parser = duration_arg)]
        warmup: Duration,
        /// Number of top processes to include (0 disables process collection).
        #[arg(long, default_value_t = 5)]
        top: usize,
    },
    /// Stream samples at a fixed interval.
    Watch {
        /// Sampling interval (default: config, else 1s).
        #[arg(long, value_parser = duration_arg)]
        interval: Option<Duration>,
        #[arg(long, value_enum, default_value_t = WatchFormat::Text)]
        format: WatchFormat,
        /// Stop after this many samples (default: run until interrupted).
        #[arg(long)]
        count: Option<u64>,
        /// Include the process table in JSONL output.
        #[arg(long)]
        processes: bool,
        /// Include per-container/service/app (cgroup) tables in JSONL output.
        #[arg(long)]
        cgroups: bool,
    },
    /// List processes, sorted by a resource.
    Processes {
        #[arg(long, value_enum, default_value_t = SortKey::Cpu)]
        sort: SortKey,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Only processes whose name contains this text (case-insensitive).
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "1s", value_parser = duration_arg)]
        warmup: Duration,
        /// Show CPU on a one-core scale (100% = one full core) instead of machine share.
        #[arg(long)]
        per_core: bool,
    },
    /// Details for one process: identity, resources, executable, working directory.
    Inspect {
        #[arg(long)]
        pid: u32,
        /// Also show the full command line (may contain secrets; off by default).
        #[arg(long)]
        show_args: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "1s", value_parser = duration_arg)]
        warmup: Duration,
    },
    /// Resource use per container, systemd service and desktop app (cgroup v2).
    ///
    /// Reads cgroup accounting as a normal user; no container runtime
    /// socket is used, so containers are named by runtime + short id.
    Groups {
        #[arg(long, value_enum)]
        kind: Option<GroupKindArg>,
        #[arg(long, value_enum, default_value_t = SortKey::Cpu)]
        sort: SortKey,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "1s", value_parser = duration_arg)]
        warmup: Duration,
        /// Resolve container names via the Docker/Podman API socket
        /// (opt-in: that socket grants broad privileges; read-only request).
        #[arg(long)]
        names: bool,
    },
    /// Shortcut for `groups --kind container`.
    Containers {
        #[arg(long)]
        json: bool,
        /// Resolve container names via the Docker/Podman API socket (opt-in).
        #[arg(long)]
        names: bool,
    },
    /// Shortcut for `groups --kind service`, with systemd unit state.
    Services {
        #[arg(long, value_enum, default_value_t = SortKey::Cpu)]
        sort: SortKey,
        /// List failed systemd units instead (exit 0 either way).
        #[arg(long)]
        failed: bool,
        #[arg(long)]
        json: bool,
    },
    /// Storage analysis (on demand).
    #[cfg(unix)]
    Disk {
        #[command(subcommand)]
        action: DiskAction,
    },
    /// Active network diagnostics against an explicit endpoint (on demand).
    Net {
        #[command(subcommand)]
        action: NetAction,
    },
    /// Find sockets by port/protocol and the processes that own them.
    ///
    /// Lists listening sockets by default; `--port` matches local or remote
    /// port in any state. Exits 4 when `--port` matches nothing.
    Ports {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long, value_enum)]
        proto: Option<ProtoArg>,
        /// Include connected (non-listening) sockets.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Record system metrics over a window, or an explicitly launched command.
    ///
    /// With `-- CMD ARGS`, records until the command exits and adds exact
    /// CPU time and peak RSS from the OS; nysm then exits with the
    /// command's status. Without a command, records for --duration
    /// (default 60s). Ctrl-C stops early. Files are created with 0600
    /// permissions and are never overwritten without --force.
    Record {
        #[arg(long, short)]
        output: std::path::PathBuf,
        #[arg(long, value_parser = duration_arg)]
        duration: Option<Duration>,
        #[arg(long, value_parser = duration_arg)]
        interval: Option<Duration>,
        /// Stop writing samples beyond this size (MiB).
        #[arg(long, default_value_t = 64)]
        max_size_mib: u64,
        #[arg(long)]
        force: bool,
        /// Free-text label stored in the header.
        #[arg(long)]
        label: Option<String>,
        /// Store the command's arguments (may contain secrets; off by default).
        #[arg(long)]
        store_args: bool,
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Summarise one recording, or compare two (first = baseline).
    Compare {
        first: std::path::PathBuf,
        second: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Watch sustained-threshold alert rules and print events as they happen.
    ///
    /// Rules come from the config file (built-in defaults: filesystem
    /// nearly full, memory pressure, I/O pressure). Runs until interrupted.
    Alerts {
        /// Print the active rules and exit.
        #[arg(long)]
        list: bool,
        #[arg(long, value_enum, default_value_t = WatchFormat::Text)]
        format: WatchFormat,
        #[arg(long, value_parser = duration_arg)]
        interval: Option<Duration>,
        /// Stop after this many samples.
        #[arg(long)]
        count: Option<u64>,
    },
    /// List (or delete) incident snapshots captured around alerts.
    ///
    /// Captures are opt-in (`[incidents] enabled = true`) and are written by
    /// `nysm service run` when an alert fires.
    Incidents {
        /// Delete all incident files.
        #[arg(long)]
        delete_all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show, create or validate the configuration file.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Report which metrics are available on this machine, with reasons.
    Capabilities {
        #[arg(long)]
        json: bool,
    },
    /// Diagnose environment, adapter and permission problems.
    Doctor,
    /// Interactive terminal UI.
    #[cfg(feature = "tui")]
    Tui {
        #[arg(long, value_parser = duration_arg)]
        interval: Option<Duration>,
        /// Use only ASCII characters for charts and borders.
        #[arg(long)]
        ascii: bool,
        /// Maximum redraws per second (independent of the collection rate).
        #[arg(long, default_value_t = 10)]
        max_fps: u32,
        /// Use the per-user collector service: auto (if running), never, require.
        #[arg(long, value_enum, default_value_t = AttachArg::Auto)]
        attach: AttachArg,
        /// Monitor another machine over SSH: runs `ssh DEST nysm service stdio`.
        /// Uses your SSH configuration, keys and host-key checking.
        #[arg(long, value_name = "[USER@]HOST")]
        remote: Option<String>,
        /// Path of nysm on the remote machine (non-interactive SSH often
        /// lacks ~/.local/bin in PATH).
        #[arg(long, default_value = "nysm", requires = "remote")]
        remote_nysm: String,
    },
    /// Optional per-user collector shared by the TUI, desktop and panel.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AttachArg {
    Auto,
    Never,
    Require,
}

#[cfg(unix)]
#[derive(Subcommand)]
pub enum DiskAction {
    /// Largest entries directly under PATH (bounded, cancellable scan).
    Usage {
        path: std::path::PathBuf,
        /// Number of entries to show.
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// Stop after visiting this many entries.
        #[arg(long, default_value_t = 2_000_000)]
        max_entries: u64,
        #[arg(long, default_value = "60s", value_parser = duration_arg)]
        timeout: Duration,
        /// Also descend into other mounted filesystems.
        #[arg(long)]
        cross_filesystems: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum NetAction {
    /// DNS lookup time and TCP connect latency to HOST[:PORT].
    ///
    /// Nothing runs in the background; only the given endpoint is
    /// contacted. Exits 1 if it cannot be resolved or reached.
    Check {
        /// e.g. example.com, example.com:443, 10.0.0.5:22, [::1]:8080
        target: String,
        /// Port when TARGET has none.
        #[arg(long, default_value_t = 443)]
        port: u16,
        /// Number of connection attempts.
        #[arg(long, default_value_t = 4)]
        count: u32,
        #[arg(long, default_value = "3s", value_parser = duration_arg)]
        timeout: Duration,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ServiceAction {
    /// Run the collector in the foreground (use systemd --user or `&` to background it).
    Run {
        /// Exit after this long without clients (e.g. 10m). Ignored when alert rules are enabled.
        #[arg(long, value_parser = duration_arg)]
        idle_exit: Option<Duration>,
        #[arg(long, value_parser = duration_arg)]
        interval: Option<Duration>,
    },
    /// Show whether a collector is running and how to reach it.
    Status,
    /// Ask a running collector to stop (SIGTERM, after verifying it is ours).
    Stop,
    /// Print a systemd user unit for this binary (does not install it).
    Unit,
    /// Serve one client over stdin/stdout (used by `nysm tui --remote`).
    ///
    /// Runs an embedded collector; no socket or listener is created and it
    /// exits when the client disconnects. Do not run it interactively.
    Stdio,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print the config file path.
    Path,
    /// Print the effective configuration (file or defaults) as TOML.
    Show,
    /// Write a default config file (0600, atomic). Refuses to overwrite without --force.
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Validate the config file; exit 2 if invalid.
    Check,
    /// Set one value, e.g. `sampling.interval 2s` or `display.theme dark`.
    /// Keeps the file's comments; refuses invalid values and invalid files.
    Set { key: String, value: String },
}

pub struct Ctx {
    pub style: Style,
    pub rate: RateUnit,
    pub settings: nysm_config::Settings,
    pub config_path: Option<std::path::PathBuf>,
}

impl Ctx {
    /// Engine configuration from settings, with an optional interval override.
    pub fn engine_config(
        &self,
        interval: Option<Duration>,
        processes: bool,
    ) -> nysm_engine::EngineConfig {
        let s = &self.settings;
        let interval = interval.unwrap_or(s.interval);
        let history_samples = (s.history.as_secs_f64() / interval.as_secs_f64())
            .ceil()
            .max(1.0) as usize;
        nysm_engine::EngineConfig {
            interval,
            processes,
            process_interval: s.process_interval,
            cgroups: false,
            filesystems: true,
            filesystem_interval: s.filesystem_interval,
            frequency: s.cpu_frequency,
            sensors: true,
            sensor_interval: std::time::Duration::from_secs(5),
            history_samples,
            history_bytes: nysm_core::history::history_bytes(history_samples),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config_path = cli.config.clone().or_else(nysm_config::default_path);
    let (settings, config_error) = nysm_config::load_or_default(config_path.as_deref());
    let is_config_cmd = matches!(cli.command, Some(Command::Config { .. }));
    if let Some(e) = &config_error
        && !is_config_cmd
    {
        eprintln!("nysm: warning: ignoring invalid configuration, using defaults: {e}");
        eprintln!("nysm: fix it, or run `nysm config check` / `nysm config init --force`");
    }
    let ctx = Ctx {
        style: Style::new(cli.color),
        rate: match cli.rate_unit {
            Some(RateArg::Bytes) => RateUnit::Bytes,
            Some(RateArg::Bits) => RateUnit::Bits,
            None => settings.rate_unit,
        },
        settings,
        config_path,
    };
    let min = Duration::from_millis(100);
    let too_short = |d: Duration, what: &str| {
        if d < min {
            eprintln!("error: {what} must be at least 100ms");
            true
        } else {
            false
        }
    };
    let result = match cli.command.unwrap_or(Command::Summary {
        json: false,
        warmup: Duration::from_secs(1),
        top: 5,
    }) {
        Command::Summary { json, warmup, top } => {
            if too_short(warmup, "--warmup") {
                return ExitCode::from(exit::USAGE);
            }
            cmd::summary::run(&ctx, json, warmup, top)
        }
        Command::Watch {
            interval,
            format,
            count,
            processes,
            cgroups,
        } => {
            if interval.is_some_and(|i| too_short(i, "--interval")) {
                return ExitCode::from(exit::USAGE);
            }
            cmd::watch::run(&ctx, interval, format, count, processes, cgroups)
        }
        Command::Processes {
            sort,
            limit,
            filter,
            json,
            warmup,
            per_core,
        } => {
            if too_short(warmup, "--warmup") {
                return ExitCode::from(exit::USAGE);
            }
            cmd::processes::run(&ctx, sort, limit, filter, json, warmup, per_core)
        }
        Command::Inspect {
            pid,
            show_args,
            json,
            warmup,
        } => cmd::inspect::run(&ctx, pid, show_args, json, warmup),
        Command::Record {
            output,
            duration,
            interval,
            max_size_mib,
            force,
            label,
            store_args,
            command,
        } => {
            if interval.is_some_and(|i| too_short(i, "--interval")) {
                return ExitCode::from(exit::USAGE);
            }
            let interval = interval.unwrap_or(ctx.settings.interval);
            cmd::record::run(
                &ctx,
                cmd::record::Args {
                    output: &output,
                    duration,
                    interval,
                    max_bytes: max_size_mib.max(1) * 1024 * 1024,
                    force,
                    label,
                    store_args,
                    command,
                },
            )
        }
        Command::Compare {
            first,
            second,
            json,
        } => cmd::compare::run(&ctx, &first, second.as_deref(), json),
        Command::Ports {
            port,
            proto,
            all,
            json,
        } => cmd::ports::run(&ctx, port, proto, all, json),
        Command::Alerts {
            list,
            format,
            interval,
            count,
        } => {
            if interval.is_some_and(|i| too_short(i, "--interval")) {
                return ExitCode::from(exit::USAGE);
            }
            cmd::alerts::run(&ctx, list, format, interval, count)
        }
        Command::Config { action } => cmd::config::run(&ctx, action, config_error),
        #[cfg(unix)]
        Command::Disk {
            action:
                DiskAction::Usage {
                    path,
                    top,
                    max_entries,
                    timeout,
                    cross_filesystems,
                    json,
                },
        } => cmd::diskusage::run(
            &ctx,
            &path,
            top,
            cmd::diskusage::Limits {
                max_entries,
                timeout,
                one_file_system: !cross_filesystems,
            },
            json,
        ),
        Command::Net {
            action:
                NetAction::Check {
                    target,
                    port,
                    count,
                    timeout,
                    json,
                },
        } => cmd::netcheck::run(&ctx, &target, port, count.clamp(1, 100), timeout, json),
        Command::Incidents { delete_all, json } => cmd::incidents::run(&ctx, delete_all, json),
        Command::Service { action } => cmd::service::run(&ctx, action),
        Command::Groups {
            kind,
            sort,
            limit,
            json,
            warmup,
            names,
        } => {
            if too_short(warmup, "--warmup") {
                return ExitCode::from(exit::USAGE);
            }
            cmd::groups::run(&ctx, kind, sort, limit, json, warmup, names)
        }
        Command::Containers { json, names } => cmd::groups::run(
            &ctx,
            Some(GroupKindArg::Container),
            SortKey::Cpu,
            usize::MAX,
            json,
            Duration::from_secs(1),
            names,
        ),
        Command::Services {
            failed: true, json, ..
        } => cmd::groups::failed_units(&ctx, json),
        Command::Services { sort, json, .. } => cmd::groups::run(
            &ctx,
            Some(GroupKindArg::Service),
            sort,
            40,
            json,
            Duration::from_secs(1),
            false,
        ),
        Command::Capabilities { json } => cmd::capabilities::run(&ctx, json),
        Command::Doctor => cmd::doctor::run(&ctx),
        #[cfg(feature = "tui")]
        Command::Tui {
            interval,
            ascii,
            max_fps,
            attach,
            remote,
            remote_nysm,
        } => {
            if interval.is_some_and(|i| too_short(i, "--interval")) {
                return ExitCode::from(exit::USAGE);
            }
            nysm_tui::run(nysm_tui::Options {
                engine: ctx.engine_config(interval, true),
                rules: ctx.settings.rules.clone(),
                ascii: ascii || ctx.settings.ascii,
                max_fps: max_fps.clamp(1, 60),
                rate: ctx.rate,
                remote: remote.map(|dest| {
                    let mut c = std::process::Command::new("ssh");
                    // -T: no remote tty (binary protocol on stdout).
                    c.args(["-T", "--", &dest, &remote_nysm, "service", "stdio"]);
                    c
                }),
                attach: match attach {
                    AttachArg::Auto => nysm_ipc::source::Attach::Auto,
                    AttachArg::Never => nysm_ipc::source::Attach::Never,
                    AttachArg::Require => nysm_ipc::source::Attach::Require,
                },
            })
            .map(|_| exit::OK)
            .map_err(|e| io::Error::other(e.to_string()))
        }
    };
    match result {
        Ok(code) => ExitCode::from(code),
        // A closed pipe (e.g. `nysm watch | head`) is a normal way to stop.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::from(exit::OK),
        Err(e) => {
            let _ = io::stdout().flush();
            eprintln!("nysm: {e}");
            ExitCode::from(exit::FAILURE)
        }
    }
}
