//! "Strip" layout: all chosen values drawn as one wide SVG image that the
//! tray item shows instead of separate icon+label items.
//!
//! Ubuntu's AppIndicator host shows an icon *file* that is at least 1.5×
//! wider than tall at its own width (its "indicator-multiload" path), so a
//! single image can hold icons, names and values with no per-item padding
//! and no truncation. Every value has a fixed slot sized for its widest
//! possible text, so the image width never changes while values do. The
//! host renders the SVG text with the system font; slot widths come from
//! Ubuntu Sans metrics (digits are tabular) scaled by the font size.

use std::path::{Path, PathBuf};

/// Logical height of the image (px). Glyphs are 16 px, text 11 pt.
const HEIGHT: f64 = 24.0;
/// 11 pt at 96 dpi.
const BASE_FONT_PX: f64 = 14.667;

/// Advance widths in Ubuntu Sans at 11 pt (px), for layout.
fn char_px(c: char) -> f64 {
    match c {
        '0'..='9' | '\u{2007}' => 8.0,
        '.' | '\u{2008}' | 'i' | 'I' | '·' => 4.0,
        '%' => 12.0,
        'B' | 'K' | 'C' | 'P' | 'R' | 'b' => 9.0,
        'M' => 13.0,
        'G' | 'U' | 'A' | 'D' => 10.0,
        'T' => 9.0,
        'k' => 7.0,
        'W' => 14.0,
        'O' => 11.0,
        '↓' | '↑' | 's' => 6.0,
        '/' => 5.0,
        ' ' => 3.0,
        '—' => 15.0,
        '⚠' => 14.0,
        _ => 9.0,
    }
}

/// Text width at the given scale (1.0 = 11 pt).
fn text_px(s: &str, scale: f64) -> f64 {
    s.chars().map(char_px).sum::<f64>() * scale
}

/// One drawn part: optional icon (16×16 symbolic SVG body), optional
/// name, then fields each with a fixed slot width.
pub struct Part {
    pub icon: Option<&'static str>,
    pub name: Option<&'static str>,
    /// (prefix like "↓", value text, widest value text for the slot)
    pub fields: Vec<(&'static str, String, &'static str)>,
}

pub struct Style {
    pub icons: bool,
    pub names: bool,
    pub stale: bool,
    pub alert: bool,
    /// Text scaling (1.0 = 11 pt), e.g. GNOME's text-scaling-factor.
    pub scale: f64,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render the strip as SVG.
pub fn svg(parts: &[Part], st: &Style) -> String {
    let s = st.scale;
    let font = BASE_FONT_PX * s;
    let fg = if st.stale { "#9a9a9a" } else { "#ffffff" };
    let baseline = (HEIGHT / 2.0 + font * 0.36).round();
    let mut x = 2.0;
    let mut body = String::new();
    let text = |body: &mut String, x: f64, t: &str, color: &str| {
        body.push_str(&format!(
            r#"<text x="{x:.1}" y="{baseline}" fill="{color}">{}</text>"#,
            esc(t)
        ));
    };
    if st.alert {
        text(&mut body, x, "⚠", "#f6d32d");
        x += text_px("⚠ ", s);
    }
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            x += 14.0 * s;
        }
        if st.icons
            && let Some(icon) = p.icon
        {
            let k = 16.0 * s / 16.0;
            let y = (HEIGHT - 16.0 * s) / 2.0;
            body.push_str(&format!(
                r#"<g transform="translate({x:.1},{y:.1}) scale({k:.3})" fill="{fg}">{icon}</g>"#
            ));
            x += 16.0 * s + 5.0 * s;
        }
        if st.names
            && let Some(name) = p.name
        {
            text(&mut body, x, name, fg);
            x += text_px(name, s) + text_px(" ", s);
        }
        for (j, (prefix, value, widest)) in p.fields.iter().enumerate() {
            if j > 0 {
                x += text_px("  ", s);
            }
            if !prefix.is_empty() {
                text(&mut body, x, prefix, fg);
                x += text_px(prefix, s) + 1.0;
            }
            text(&mut body, x, value, fg);
            // Fixed slot: the widest text this field can show.
            x += text_px(widest, s).max(text_px(value, s));
        }
    }
    let width = (x + 2.0).ceil().max(HEIGHT * 1.5 + 1.0);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{HEIGHT}" viewBox="0 0 {width} {HEIGHT}"><g font-family="Ubuntu Sans, Ubuntu, Cantarell, sans-serif" font-size="{font:.2}px" style="font-variant-numeric: tabular-nums">{body}</g></svg>"#
    )
}

/// Writes strip images, alternating between two file names so the host
/// reloads the image on every change.
pub struct Writer {
    dir: PathBuf,
    flip: bool,
    last: String,
    pub path: String,
}

impl Writer {
    pub fn new(dir: &Path) -> Self {
        Writer {
            dir: dir.to_path_buf(),
            flip: false,
            last: String::new(),
            path: String::new(),
        }
    }

    /// Write `svg` if it changed; `path` then names the new file.
    pub fn write(&mut self, svg: &str) {
        if svg == self.last {
            return;
        }
        self.flip = !self.flip;
        let p = self
            .dir
            .join(format!("nysm-strip-{}.svg", u8::from(self.flip)));
        let tmp = p.with_extension("svg.tmp");
        if std::fs::write(&tmp, svg).is_ok() && std::fs::rename(&tmp, &p).is_ok() {
            self.path = p.to_string_lossy().into_owned();
            self.last = svg.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(v: &str) -> Part {
        Part {
            icon: None,
            name: Some("CPU"),
            fields: vec![("", v.into(), "100%")],
        }
    }

    fn width_of(svg: &str) -> String {
        let i = svg.find("width=\"").unwrap() + 7;
        svg[i..].split('"').next().unwrap().to_string()
    }

    #[test]
    fn width_does_not_depend_on_values() {
        let st = Style {
            icons: true,
            names: true,
            stale: false,
            alert: false,
            scale: 1.0,
        };
        let a = svg(&[part("5%"), part("9.9G")], &st);
        let b = svg(&[part("100%"), part("10G")], &st);
        assert_eq!(width_of(&a), width_of(&b));
        assert!(a.contains(">CPU<") && a.contains(">5%<"));
        // Wide enough for the host's wide-image path (>= 1.5 × height).
        assert!(width_of(&a).parse::<f64>().unwrap() >= HEIGHT * 1.5);
    }
}
