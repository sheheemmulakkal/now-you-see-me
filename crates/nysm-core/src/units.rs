//! Unit formatting. Bytes use binary prefixes (KiB, MiB) for sizes and
//! decimal-free labels are always explicit; network rates may be shown
//! as bytes/s or bits/s, and the label always says which.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RateUnit {
    /// Bytes per second with binary prefixes (KiB/s).
    #[default]
    Bytes,
    /// Bits per second with decimal prefixes (kb/s, Mb/s), as links are rated.
    Bits,
}

/// `1536 -> "1.5 KiB"`.
pub fn bytes(n: f64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if !n.is_finite() {
        return "?".into();
    }
    let mut v = n;
    let mut i = 0;
    while v.abs() >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v.abs() < 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.0} {}", UNITS[i])
    }
}

pub fn rate(bytes_per_s: f64, unit: RateUnit) -> String {
    match unit {
        RateUnit::Bytes => format!("{}/s", bytes(bytes_per_s)),
        RateUnit::Bits => {
            const UNITS: [&str; 5] = ["b/s", "kb/s", "Mb/s", "Gb/s", "Tb/s"];
            let mut v = bytes_per_s * 8.0;
            let mut i = 0;
            while v.abs() >= 1000.0 && i < UNITS.len() - 1 {
                v /= 1000.0;
                i += 1;
            }
            if i == 0 || v.abs() >= 10.0 {
                format!("{v:.0} {}", UNITS[i])
            } else {
                format!("{v:.1} {}", UNITS[i])
            }
        }
    }
}

pub fn percent(p: f64) -> String {
    format!("{p:.1}%")
}

pub fn duration_s(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (d, h, m, s) = (s / 86400, s / 3600 % 24, s / 60 % 60, s % 60);
    if d > 0 {
        format!("{d}d {h}h {m:02}m")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Parse "500ms", "1s", "1.5s", "2m" (humantime-compatible subset kept
/// dependency-free in core).
pub fn parse_duration(s: &str) -> Option<std::time::Duration> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let secs = match unit.trim() {
        "ms" => n / 1000.0,
        "s" | "sec" => n,
        "m" | "min" => n * 60.0,
        "h" => n * 3600.0,
        _ => return None,
    };
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    Some(std::time::Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatting() {
        assert_eq!(bytes(0.0), "0 B");
        assert_eq!(bytes(1536.0), "1.5 KiB");
        assert_eq!(bytes(15.0 * 1024.0 * 1024.0), "15 MiB");
        assert_eq!(bytes(f64::NAN), "?");
    }

    #[test]
    fn rate_units_are_explicit() {
        assert_eq!(rate(1024.0, RateUnit::Bytes), "1.0 KiB/s");
        assert_eq!(rate(125_000.0, RateUnit::Bits), "1.0 Mb/s");
        assert_eq!(rate(12.0, RateUnit::Bits), "96 b/s");
    }

    #[test]
    fn durations() {
        assert_eq!(
            parse_duration("500ms"),
            Some(std::time::Duration::from_millis(500))
        );
        assert_eq!(
            parse_duration("1.5s"),
            Some(std::time::Duration::from_millis(1500))
        );
        assert_eq!(
            parse_duration("2m"),
            Some(std::time::Duration::from_secs(120))
        );
        assert_eq!(parse_duration("2 parsecs"), None);
        assert_eq!(parse_duration("-1s"), None);
        assert_eq!(duration_s(3725.0), "1h 02m");
    }
}
