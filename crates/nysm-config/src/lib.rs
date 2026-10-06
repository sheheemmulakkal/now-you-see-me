//! Configuration file: TOML, versioned, validated, written atomically.
//!
//! Location: `$NYSM_CONFIG`, else the platform config directory:
//! Linux/BSD `$XDG_CONFIG_HOME/nysm/config.toml` (`~/.config/...`),
//! macOS `~/Library/Application Support/nysm/config.toml`,
//! Windows `%APPDATA%\nysm\config.toml`.
//!
//! A missing file means defaults. An invalid file is an error that names
//! the file and the problem; callers fall back to defaults with a warning.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use nysm_core::alerts::{self, Rule};
use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;
pub const MAX_CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub sampling: Sampling,
    #[serde(default)]
    pub display: Display,
    #[serde(default)]
    pub alerts: Alerts,
    #[serde(default)]
    pub incidents: Incidents,
}

/// Opt-in capture of metrics around alert events (needs `nysm service`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Incidents {
    pub enabled: bool,
    /// Window kept before the event, e.g. "5m".
    pub pre: String,
    /// Window recorded after the event, e.g. "2m".
    pub post: String,
    pub max_files: usize,
    /// Total size cap for all incident files, MiB.
    pub max_total_mib: u64,
    /// Directory; default `$XDG_STATE_HOME/nysm/incidents`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

impl Default for Incidents {
    fn default() -> Self {
        Incidents {
            enabled: false,
            pre: "5m".into(),
            post: "2m".into(),
            max_files: 20,
            max_total_mib: 200,
            dir: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Sampling {
    /// e.g. "1s", "500ms".
    pub interval: String,
    pub process_interval: String,
    pub filesystem_interval: String,
    /// Retained in-memory history, e.g. "10m".
    pub history: String,
    pub cpu_frequency: bool,
}

impl Default for Sampling {
    fn default() -> Self {
        Sampling {
            interval: "1s".into(),
            process_interval: "2s".into(),
            filesystem_interval: "15s".into(),
            history: "10m".into(),
            cpu_frequency: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RateUnitCfg {
    #[default]
    Bytes,
    Bits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the desktop's light/dark preference.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Display {
    pub rate_unit: RateUnitCfg,
    pub ascii: bool,
    /// Desktop app theme.
    pub theme: Theme,
    /// Desktop app: resolve container names through the Docker/Podman API
    /// socket (opt-in; that socket grants broad privileges).
    pub container_names: bool,
    /// Tray items in the top bar, comma separated, in order:
    /// cpu, mem, net, storage (root filesystem used %), disk (activity %),
    /// diskio (read/write).
    pub tray_items: String,
}

impl Default for Display {
    fn default() -> Self {
        Display {
            rate_unit: RateUnitCfg::default(),
            ascii: false,
            theme: Theme::default(),
            container_names: false,
            tray_items: "cpu,mem,net,storage,disk".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Alerts {
    /// Include the built-in rules (`alerts::default_rules`). Rules in
    /// `rules` with the same id replace a default.
    pub use_defaults: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

impl Default for Alerts {
    fn default() -> Self {
        Alerts {
            use_defaults: true,
            rules: Vec::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CONFIG_VERSION,
            sampling: Sampling::default(),
            display: Display::default(),
            alerts: Alerts::default(),
            incidents: Incidents::default(),
        }
    }
}

/// Validated, typed view used by the application.
#[derive(Debug, Clone)]
pub struct Settings {
    pub interval: Duration,
    pub process_interval: Duration,
    pub filesystem_interval: Duration,
    pub history: Duration,
    pub cpu_frequency: bool,
    pub rate_unit: nysm_core::units::RateUnit,
    pub ascii: bool,
    pub theme: Theme,
    pub container_names: bool,
    pub tray_items: String,
    pub rules: Vec<Rule>,
    pub incidents: IncidentSettings,
}

#[derive(Debug, Clone)]
pub struct IncidentSettings {
    pub enabled: bool,
    pub pre: Duration,
    pub post: Duration,
    pub max_files: usize,
    pub max_total_bytes: u64,
    pub dir: Option<PathBuf>,
}

/// Default incident directory: `$XDG_STATE_HOME/nysm/incidents`
/// (`~/.local/state/nysm/incidents`).
pub fn default_incident_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".local/state"))
        })?;
    Some(base.join(nysm_core::brand::COMMAND_NAME).join("incidents"))
}

#[derive(Debug)]
pub enum ConfigError {
    Io(PathBuf, io::Error),
    TooLarge(PathBuf),
    Parse(PathBuf, String),
    Invalid(PathBuf, String),
    NewerVersion(PathBuf, u32),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            ConfigError::TooLarge(p) => {
                write!(f, "{}: larger than {MAX_CONFIG_BYTES} bytes", p.display())
            }
            ConfigError::Parse(p, e) => write!(f, "{}: {e}", p.display()),
            ConfigError::Invalid(p, e) => write!(f, "{}: {e}", p.display()),
            ConfigError::NewerVersion(p, v) => {
                write!(
                    f,
                    "{}: config version {v} is newer than supported ({CONFIG_VERSION}); upgrade nysm",
                    p.display()
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

fn dur(field: &str, s: &str, min: Duration, max: Duration) -> Result<Duration, String> {
    let d = nysm_core::units::parse_duration(s)
        .ok_or_else(|| format!("{field}: invalid duration {s:?} (e.g. 500ms, 1s, 2m)"))?;
    if d < min || d > max {
        return Err(format!(
            "{field}: {s} is outside {}..{}",
            fmt_d(min),
            fmt_d(max)
        ));
    }
    Ok(d)
}

fn fmt_d(d: Duration) -> String {
    if d.as_secs() >= 60 && d.as_secs().is_multiple_of(60) {
        format!("{}m", d.as_secs() / 60)
    } else {
        format!("{}ms", d.as_millis())
    }
}

impl Config {
    pub fn settings(&self) -> Result<Settings, String> {
        if self.version == 0 {
            return Err("version must be 1".into());
        }
        let ms = Duration::from_millis;
        let s = &self.sampling;
        let interval = dur(
            "sampling.interval",
            &s.interval,
            ms(100),
            Duration::from_secs(3600),
        )?;
        let process_interval = dur(
            "sampling.process_interval",
            &s.process_interval,
            ms(100),
            Duration::from_secs(3600),
        )?;
        let filesystem_interval = dur(
            "sampling.filesystem_interval",
            &s.filesystem_interval,
            Duration::from_secs(1),
            Duration::from_secs(3600),
        )?;
        let history = dur(
            "sampling.history",
            &s.history,
            Duration::from_secs(10),
            Duration::from_secs(24 * 3600),
        )?;
        let mut rules = if self.alerts.use_defaults {
            alerts::default_rules()
        } else {
            Vec::new()
        };
        for r in &self.alerts.rules {
            r.validate()?;
            if r.id.trim().is_empty() {
                return Err("alerts.rules: id must not be empty".into());
            }
            rules.retain(|d| d.id != r.id);
            rules.push(r.clone());
        }
        let mut ids: Vec<&str> = self.alerts.rules.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err("alerts.rules: duplicate rule id".into());
        }
        Ok(Settings {
            interval,
            process_interval,
            filesystem_interval,
            history,
            cpu_frequency: s.cpu_frequency,
            rate_unit: match self.display.rate_unit {
                RateUnitCfg::Bytes => nysm_core::units::RateUnit::Bytes,
                RateUnitCfg::Bits => nysm_core::units::RateUnit::Bits,
            },
            ascii: self.display.ascii,
            theme: self.display.theme,
            container_names: self.display.container_names,
            tray_items: self.display.tray_items.clone(),
            rules,
            incidents: IncidentSettings {
                enabled: self.incidents.enabled,
                pre: dur(
                    "incidents.pre",
                    &self.incidents.pre,
                    Duration::from_secs(0),
                    Duration::from_secs(3600),
                )?,
                post: dur(
                    "incidents.post",
                    &self.incidents.post,
                    Duration::from_secs(0),
                    Duration::from_secs(3600),
                )?,
                max_files: self.incidents.max_files.clamp(1, 1000),
                max_total_bytes: self.incidents.max_total_mib.clamp(1, 10 * 1024) * 1024 * 1024,
                dir: self
                    .incidents
                    .dir
                    .as_ref()
                    .map(PathBuf::from)
                    .or_else(default_incident_dir),
            },
        })
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serialises")
    }
}

/// Default config file path for this platform, honouring `$NYSM_CONFIG`.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NYSM_CONFIG").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let home = || {
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
    }?;
    Some(
        base.join(nysm_core::brand::COMMAND_NAME)
            .join("config.toml"),
    )
}

pub fn parse(path: &Path, text: &str) -> Result<(Config, Settings), ConfigError> {
    // Check the version before strict parsing so newer files get a clear message.
    let raw: toml::Table =
        toml::from_str(text).map_err(|e| ConfigError::Parse(path.into(), e.to_string()))?;
    match raw.get("version").and_then(|v| v.as_integer()) {
        Some(v) if v > CONFIG_VERSION as i64 => {
            return Err(ConfigError::NewerVersion(path.into(), v as u32));
        }
        Some(v) if v >= 1 => {}
        _ => {
            return Err(ConfigError::Invalid(
                path.into(),
                format!("missing or invalid `version` (expected {CONFIG_VERSION})"),
            ));
        }
    }
    let cfg: Config =
        toml::from_str(text).map_err(|e| ConfigError::Parse(path.into(), e.to_string()))?;
    let settings = cfg
        .settings()
        .map_err(|e| ConfigError::Invalid(path.into(), e))?;
    Ok((cfg, settings))
}

/// Load from `path`. `Ok(None)` when the file does not exist.
pub fn load(path: &Path) -> Result<Option<(Config, Settings)>, ConfigError> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ConfigError::Io(path.into(), e)),
    };
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge(path.into()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.into(), e))?;
    parse(path, &text).map(Some)
}

/// Defaults, or the loaded file; on error, defaults plus the error so the
/// caller can warn. Never panics on a bad file.
pub fn load_or_default(path: Option<&Path>) -> (Settings, Option<ConfigError>) {
    let defaults = || Config::default().settings().expect("defaults are valid");
    match path.map(load) {
        Some(Ok(Some((_, s)))) => (s, None),
        Some(Err(e)) => (defaults(), Some(e)),
        _ => (defaults(), None),
    }
}

const EXAMPLE_RULE: &str = r#"
# Add or override alert rules (same id replaces a built-in rule). Metrics:
# cpu_pct, memory_used_pct, swap_used_pct, cpu_pressure_pct,
# memory_pressure_pct, io_pressure_pct, filesystem_used_pct, cpu_temperature_c.
#
# [[alerts.rules]]
# id = "cpu-saturated"
# metric = "cpu_pressure_pct"
# threshold = 50.0   # breach above this
# clear = 25.0       # resolve only below this (hysteresis)
# for_s = 120.0      # must hold this long before firing
# cooldown_s = 600.0 # no re-notification within this after resolving
"#;

/// Atomically write `cfg` to `path` (temp file + rename), mode 0600.
pub fn save(path: &Path, cfg: &Config) -> io::Result<()> {
    let text = format!(
        "# Now You See Me configuration. See docs/configuration.md.\n{}{EXAMPLE_RULE}",
        cfg.to_toml()
    );
    write_atomic(path, text.as_bytes())
}

/// Set individual `table.key` values in the config file, keeping the
/// file's comments and layout. Values that parse as TOML booleans or
/// numbers are stored as such, anything else as a string. The existing
/// file must be valid (it is never overwritten otherwise) and the result is
/// validated before the atomic write. Creates the file if it is missing.
pub fn set_many(path: &Path, values: &[(&str, &str)]) -> Result<Settings, ConfigError> {
    let text = match load(path)? {
        Some(_) => std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.into(), e))?,
        None => format!("version = {CONFIG_VERSION}\n"),
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigError::Parse(path.into(), e.to_string()))?;
    for (key, raw) in values {
        let invalid = |m: String| ConfigError::Invalid(path.into(), m);
        let (table, field) = key
            .split_once('.')
            .filter(|(t, f)| !t.is_empty() && !f.is_empty() && !f.contains('.'))
            .ok_or_else(|| invalid(format!("key {key:?} must look like table.key")))?;
        let value = match raw.parse::<toml_edit::Value>() {
            Ok(
                v @ (toml_edit::Value::Boolean(_)
                | toml_edit::Value::Integer(_)
                | toml_edit::Value::Float(_)),
            ) => v,
            _ => toml_edit::Value::from(*raw),
        };
        let t = doc
            .entry(table)
            .or_insert_with(toml_edit::table)
            .as_table_like_mut()
            .ok_or_else(|| invalid(format!("{table} is not a table")))?;
        match t.get_mut(field).and_then(|i| i.as_value_mut()) {
            // Replace only the value so comments attached to the key stay.
            Some(v) => {
                let decor = v.decor().clone();
                *v = value;
                *v.decor_mut() = decor;
            }
            None => {
                t.insert(field, toml_edit::Item::Value(value));
            }
        }
    }
    let new_text = doc.to_string();
    let (_, settings) = parse(path, &new_text)?;
    write_atomic(path, new_text.as_bytes()).map_err(|e| ConfigError::Io(path.into(), e))?;
    Ok(settings)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("toml.tmp-{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> PathBuf {
        PathBuf::from("/x/config.toml")
    }

    #[test]
    fn defaults_round_trip_and_validate() {
        let c = Config::default();
        let (back, s) = parse(&p(), &c.to_toml()).unwrap();
        assert_eq!(back, c);
        assert_eq!(s.interval, Duration::from_secs(1));
        assert_eq!(s.rules.len(), alerts::default_rules().len());
    }

    #[test]
    fn set_keeps_comments_and_validates() {
        let dir = std::env::temp_dir().join(format!("nysm-cfg-set-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        // Missing file: created with just the new values.
        let s = set_many(&path, &[("display.theme", "dark")]).unwrap();
        assert_eq!(s.theme, Theme::Dark);
        std::fs::write(
            &path,
            "# my notes\nversion = 1\n\n[sampling]\n# keep this\ninterval = \"1s\"\n",
        )
        .unwrap();
        let s = set_many(
            &path,
            &[("sampling.interval", "2s"), ("incidents.enabled", "true")],
        )
        .unwrap();
        assert_eq!(s.interval, Duration::from_secs(2));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# my notes") && text.contains("# keep this"),
            "{text}"
        );
        assert!(text.contains("enabled = true"), "{text}");
        // Invalid result: rejected, file unchanged.
        assert!(set_many(&path, &[("sampling.interval", "fast")]).is_err());
        assert!(set_many(&path, &[("nonsense", "1")]).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        // Invalid existing file: never overwritten.
        std::fs::write(&path, "version = 1\n[sampling]\nbogus = 1\n").unwrap();
        assert!(set_many(&path, &[("display.theme", "light")]).is_err());
        assert!(std::fs::read_to_string(&path).unwrap().contains("bogus"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn minimal_file_uses_defaults() {
        let (_, s) = parse(&p(), "version = 1\n").unwrap();
        assert_eq!(s.process_interval, Duration::from_secs(2));
    }

    #[test]
    fn custom_rules_replace_defaults_by_id() {
        let t = r#"
version = 1
[alerts]
[[alerts.rules]]
id = "memory-pressure"
metric = "memory_pressure_pct"
threshold = 20.0
clear = 10.0
for_s = 10.0
[[alerts.rules]]
id = "cpu-hot"
metric = "cpu_pct"
threshold = 95.0
clear = 80.0
for_s = 300.0
cooldown_s = 600.0
"#;
        let (_, s) = parse(&p(), t).unwrap();
        let mem: Vec<_> = s
            .rules
            .iter()
            .filter(|r| r.id == "memory-pressure")
            .collect();
        assert_eq!(mem.len(), 1);
        assert_eq!(mem[0].threshold, 20.0);
        assert!(s.rules.iter().any(|r| r.id == "cpu-hot"));
    }

    #[test]
    fn errors_are_actionable() {
        let e = |t: &str| parse(&p(), t).unwrap_err().to_string();
        assert!(e("version = 1\n[sampling]\nintervall = \"1s\"\n").contains("unknown field"));
        assert!(e("version = 1\n[sampling]\ninterval = \"10ms\"\n").contains("sampling.interval"));
        assert!(e("version = 1\n[sampling]\ninterval = \"soon\"\n").contains("invalid duration"));
        assert!(e("version = 9\n").contains("newer than supported"));
        assert!(e("[sampling]\n").contains("version"));
        assert!(e("this is = = not toml").contains("/x/config.toml"));
        let bad_rule = "version = 1\n[[alerts.rules]]\nid = \"r\"\nmetric = \"cpu_pct\"\nthreshold = 50.0\nclear = 60.0\nfor_s = 1.0\n";
        assert!(e(bad_rule).contains("healthy side"));
        let dup = "version = 1\n[[alerts.rules]]\nid = \"r\"\nmetric = \"cpu_pct\"\nthreshold = 50.0\nclear = 40.0\nfor_s = 1.0\n[[alerts.rules]]\nid = \"r\"\nmetric = \"cpu_pct\"\nthreshold = 50.0\nclear = 40.0\nfor_s = 1.0\n";
        assert!(e(dup).contains("duplicate"));
    }

    #[test]
    fn save_is_atomic_private_and_loadable() {
        let dir = std::env::temp_dir().join(format!("nysm-cfg-{}", std::process::id()));
        let path = dir.join("sub/config.toml");
        save(&path, &Config::default()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(load(&path).unwrap().is_some());
        // Appending a rule to a generated file must work.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("\n[[alerts.rules]]\nid = \"x\"\nmetric = \"cpu_pct\"\nthreshold = 90.0\nclear = 80.0\nfor_s = 5.0\n");
        assert!(
            parse(&path, &text)
                .unwrap()
                .1
                .rules
                .iter()
                .any(|r| r.id == "x")
        );
        // Corrupt file: load_or_default returns defaults plus an error.
        std::fs::write(&path, "garbage = [").unwrap();
        let (s, err) = load_or_default(Some(&path));
        assert!(err.is_some());
        assert_eq!(s.interval, Duration::from_secs(1));
        // Missing file: defaults, no error.
        let (_, err) = load_or_default(Some(&dir.join("nope.toml")));
        assert!(err.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
