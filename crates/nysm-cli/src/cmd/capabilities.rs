use std::io::{self, Write};
use std::time::Duration;

use nysm_core::Status;
use nysm_core::capabilities::report;

use crate::{Ctx, fmt};

pub fn run(ctx: &Ctx, json: bool) -> io::Result<u8> {
    let (engine, snap) = fmt::one_shot_with(Duration::from_millis(500), true, true);
    let rep = report(&snap, &|id| engine.source(id));
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &rep)?;
        writeln!(out)?;
        return Ok(crate::exit::OK);
    }
    let st = &ctx.style;
    writeln!(out, "{}", fmt::host_line(st, &snap))?;
    writeln!(out)?;
    for c in &rep.capabilities {
        let label = format!("{:<18}", c.status.label());
        let status = match c.status {
            Status::Available => label,
            Status::WarmingUp | Status::Stale => st.warn(&label),
            _ => st.dim(&label),
        };
        writeln!(
            out,
            "  {} {} {}",
            st.bold(&format!("{:<26}", c.id)),
            status,
            st.dim(c.source)
        )?;
        if let Some(r) = &c.reason {
            writeln!(out, "  {:<26} {}", "", st.dim(&fmt::safe(r)))?;
        }
    }
    Ok(crate::exit::OK)
}
