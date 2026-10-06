//! `nysm config path|show|init|check`.

use std::io::{self, Write};

use nysm_config::{Config, ConfigError};

use crate::{ConfigAction, Ctx, exit};

pub fn run(ctx: &Ctx, action: ConfigAction, load_error: Option<ConfigError>) -> io::Result<u8> {
    let mut out = io::stdout().lock();
    let Some(path) = ctx.config_path.clone() else {
        eprintln!("nysm: no config location (set $NYSM_CONFIG, $XDG_CONFIG_HOME or $HOME)");
        return Ok(exit::FAILURE);
    };
    match action {
        ConfigAction::Path => {
            writeln!(out, "{}", path.display())?;
            Ok(exit::OK)
        }
        ConfigAction::Check => match (load_error, path.exists()) {
            (Some(e), _) => {
                eprintln!("nysm: invalid: {e}");
                Ok(exit::USAGE)
            }
            (None, true) => {
                writeln!(out, "{}: valid", path.display())?;
                Ok(exit::OK)
            }
            (None, false) => {
                writeln!(
                    out,
                    "{}: not present; built-in defaults are in use",
                    path.display()
                )?;
                Ok(exit::OK)
            }
        },
        ConfigAction::Show => {
            if let Some(e) = load_error {
                eprintln!("nysm: invalid: {e}");
                return Ok(exit::USAGE);
            }
            match nysm_config::load(&path) {
                Ok(Some((cfg, _))) => {
                    writeln!(out, "# {}", path.display())?;
                    write!(out, "{}", cfg.to_toml())?;
                }
                _ => {
                    writeln!(out, "# defaults (no file at {})", path.display())?;
                    write!(out, "{}", Config::default().to_toml())?;
                }
            }
            Ok(exit::OK)
        }
        ConfigAction::Init { force } => {
            if path.exists() && !force {
                eprintln!(
                    "nysm: {} already exists (use --force to replace it with defaults)",
                    path.display()
                );
                return Ok(exit::FAILURE);
            }
            nysm_config::save(&path, &Config::default())?;
            writeln!(out, "wrote {}", path.display())?;
            Ok(exit::OK)
        }
    }
}
