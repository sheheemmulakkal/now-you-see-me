//! `nysm incidents`: list or delete incident snapshots.

use std::io::{self, Write};

use nysm_core::units;
use nysm_record::{Target, incident, reader};
use serde::Serialize;

use crate::{Ctx, exit, fmt};

#[derive(Serialize)]
struct Item {
    path: String,
    created_ms: i64,
    bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    samples: u64,
    truncated: bool,
}

pub fn run(ctx: &Ctx, delete_all: bool, json: bool) -> io::Result<u8> {
    let i = &ctx.settings.incidents;
    let Some(dir) = i.dir.clone() else {
        eprintln!("nysm: no incident directory (set incidents.dir in the config)");
        return Ok(exit::FAILURE);
    };
    let files = incident::list(&dir)?;
    let mut out = io::stdout().lock();
    if delete_all {
        for f in &files {
            std::fs::remove_file(&f.path)?;
        }
        writeln!(
            out,
            "deleted {} incident files from {}",
            files.len(),
            dir.display()
        )?;
        return Ok(exit::OK);
    }
    let items: Vec<Item> = files
        .iter()
        .map(|f| {
            let mut samples = 0u64;
            let c = reader::read(&f.path, reader::DEFAULT_READ_LIMIT, |_| samples += 1).ok();
            let (rule, message) = match c.as_ref().map(|c| &c.header.target) {
                Some(Target::Incident { rule, message, .. }) => {
                    (Some(rule.clone()), Some(message.clone()))
                }
                _ => (None, None),
            };
            Item {
                path: f.path.display().to_string(),
                created_ms: f.created_ms,
                bytes: f.bytes,
                rule,
                message,
                samples,
                truncated: c
                    .as_ref()
                    .is_none_or(|c| c.end.as_ref().is_none_or(|e| e.truncated)),
            }
        })
        .collect();
    if json {
        serde_json::to_writer_pretty(
            &mut out,
            &serde_json::json!({
                "schema_version": nysm_core::SCHEMA_VERSION,
                "enabled": i.enabled,
                "dir": dir.display().to_string(),
                "incidents": items,
            }),
        )?;
        writeln!(out)?;
        return Ok(exit::OK);
    }
    let st = &ctx.style;
    if !i.enabled {
        writeln!(out, "{}", st.dim("incident snapshots are off; enable [incidents] in the config and run `nysm service run`"))?;
    }
    if items.is_empty() {
        writeln!(out, "no incidents in {}", dir.display())?;
        return Ok(exit::OK);
    }
    for it in &items {
        writeln!(
            out,
            "{}  {}  {:>4} samples  {:>8}{}",
            fmt::clock(it.created_ms),
            st.bold(&fmt::safe(it.rule.as_deref().unwrap_or("?"))),
            it.samples,
            units::bytes(it.bytes as f64),
            if it.truncated {
                st.warn("  truncated")
            } else {
                String::new()
            }
        )?;
        if let Some(m) = &it.message {
            writeln!(out, "    {}", fmt::safe(m))?;
        }
        writeln!(out, "    {}", st.dim(&format!("nysm compare {}", it.path)))?;
    }
    Ok(exit::OK)
}
