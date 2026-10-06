//! Live meter icon: two vertical bars (CPU, memory) drawn into an ARGB32
//! pixmap. Works on every StatusNotifier host because it is just an icon;
//! exact numbers live in the tooltip and menu.

/// What the icon should show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Meter {
    /// 0–100, `None` when unavailable (only the track is drawn).
    pub cpu: Option<f64>,
    pub mem: Option<f64>,
    /// Data is not live: bars are drawn grey.
    pub stale: bool,
    /// An alert is firing: a red marker is drawn.
    pub alert: bool,
}

type Rgba = [u8; 4];

const CPU: Rgba = [0x1c, 0x87, 0xde, 0xff];
const MEM: Rgba = [0x91, 0x57, 0xcc, 0xff];
const STALE: Rgba = [0x9a, 0x9a, 0x9a, 0xff];
const TRACK: Rgba = [0x80, 0x80, 0x80, 0x60];
const ALERT: Rgba = [0xe0, 0x1b, 0x24, 0xff];

/// Square ARGB32 (network byte order: A, R, G, B) icon of `size` pixels.
pub fn render(size: i32, m: Meter) -> ksni::Icon {
    let s = size.max(8) as usize;
    let mut px = vec![[0u8; 4]; s * s]; // RGBA, transparent
    let margin = (s / 8).max(1);
    let gap = (s / 8).max(1);
    let bar_w = (s - 2 * margin - gap) / 2;
    let top = margin;
    let bottom = s - margin; // exclusive
    let height = bottom - top;
    let mut fill_bar = |x0: usize, value: Option<f64>, color: Rgba| {
        for y in top..bottom {
            for x in x0..x0 + bar_w {
                px[y * s + x] = TRACK;
            }
        }
        if let Some(v) = value.filter(|v| v.is_finite()) {
            let h = ((v.clamp(0.0, 100.0) / 100.0) * height as f64).round() as usize;
            // Any non-zero load shows at least one pixel.
            let h = if v > 0.0 { h.max(1) } else { 0 };
            for y in bottom - h..bottom {
                for x in x0..x0 + bar_w {
                    px[y * s + x] = color;
                }
            }
        }
    };
    let (c1, c2) = if m.stale { (STALE, STALE) } else { (CPU, MEM) };
    fill_bar(margin, m.cpu, c1);
    fill_bar(margin + bar_w + gap, m.mem, c2);
    if m.alert {
        let d = (s / 4).max(2);
        for y in 0..d {
            for x in s - d..s {
                px[y * s + x] = ALERT;
            }
        }
    }
    let data = px
        .iter()
        .flat_map(|[r, g, b, a]| [*a, *r, *g, *b])
        .collect();
    ksni::Icon {
        width: s as i32,
        height: s as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(i: &ksni::Icon, x: usize, y: usize) -> [u8; 4] {
        let o = (y * i.width as usize + x) * 4;
        [i.data[o], i.data[o + 1], i.data[o + 2], i.data[o + 3]]
    }

    fn argb(c: Rgba) -> [u8; 4] {
        [c[3], c[0], c[1], c[2]]
    }

    #[test]
    fn bars_scale_with_values() {
        let i = render(
            22,
            Meter {
                cpu: Some(100.0),
                mem: Some(0.0),
                stale: false,
                alert: false,
            },
        );
        assert_eq!(i.data.len(), 22 * 22 * 4);
        // Full CPU bar: top of left bar is CPU colour.
        assert_eq!(pixel(&i, 3, 3), argb(CPU));
        // Empty memory bar: only the track at the bottom.
        assert_eq!(pixel(&i, 14, 18), argb(TRACK));
        // Outside the bars is transparent.
        assert_eq!(pixel(&i, 0, 0)[0], 0);
    }

    #[test]
    fn half_full_and_tiny_values() {
        let i = render(
            44,
            Meter {
                cpu: Some(50.0),
                mem: Some(0.4),
                stale: false,
                alert: false,
            },
        );
        // Left bar: bottom filled, top is track.
        assert_eq!(pixel(&i, 10, 38), argb(CPU));
        assert_eq!(pixel(&i, 10, 8), argb(TRACK));
        // A tiny but non-zero memory value still shows one pixel row.
        assert_eq!(pixel(&i, 30, 38), argb(MEM));
    }

    #[test]
    fn missing_stale_and_alert_states() {
        let i = render(
            22,
            Meter {
                cpu: None,
                mem: Some(80.0),
                stale: true,
                alert: true,
            },
        );
        assert_eq!(
            pixel(&i, 4, 18),
            argb(TRACK),
            "missing value is never drawn as a level"
        );
        assert_eq!(pixel(&i, 14, 18), argb(STALE));
        assert_eq!(pixel(&i, 21, 0), argb(ALERT));
    }
}
