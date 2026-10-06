//! Area/line trend chart drawn with cairo, styled after the reference
//! design: soft filled areas, y-axis scale labels, and a time axis that
//! ends at "now". Missing samples and gaps break the line and the fill;
//! nothing is interpolated across them. Every chart carries a text
//! description for screen readers (`set_summary`).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

#[derive(Clone, Copy)]
pub struct Rgb(pub f64, pub f64, pub f64);

// Palette from the reference collage. Series are named in the card
// legends next to their values, so colour is never the only cue.
pub const CPU: Rgb = Rgb(0.18, 0.77, 0.71);
pub const MEM: Rgb = Rgb(0.65, 0.42, 1.00);
pub const NET_RX: Rgb = Rgb(0.31, 0.55, 1.00);
pub const NET_TX: Rgb = Rgb(0.65, 0.42, 1.00);
pub const DISK_R: Rgb = Rgb(0.96, 0.65, 0.14);
pub const DISK_W: Rgb = Rgb(0.93, 0.45, 0.13);

#[derive(Clone)]
pub struct Series {
    pub color: Rgb,
    /// Oldest first. `None` = no data for that sample.
    pub values: Vec<Option<f64>>,
}

#[derive(Default)]
struct Data {
    series: Vec<Series>,
    /// Indices where a gap (suspend/stall) precedes the sample.
    gaps: Vec<usize>,
    /// Fixed maximum (e.g. 100 for percentages) or auto when None.
    fixed_max: Option<f64>,
    format: Option<fn(f64) -> String>,
    /// Seconds covered by the data (for the time axis).
    span_s: f64,
    fill: bool,
    axes: bool,
}

#[derive(Clone)]
pub struct Chart {
    pub area: gtk::DrawingArea,
    data: Rc<RefCell<Data>>,
}

impl Chart {
    pub fn new(height: i32, fixed_max: Option<f64>, format: fn(f64) -> String) -> Self {
        Self::build(height, fixed_max, format, true, true)
    }

    /// Small chart without axes (sparkline style), still filled.
    pub fn mini(height: i32, fixed_max: Option<f64>, format: fn(f64) -> String) -> Self {
        Self::build(height, fixed_max, format, true, false)
    }

    fn build(
        height: i32,
        fixed_max: Option<f64>,
        format: fn(f64) -> String,
        fill: bool,
        axes: bool,
    ) -> Self {
        let area = gtk::DrawingArea::new();
        area.set_content_height(height);
        area.set_hexpand(true);
        let data = Rc::new(RefCell::new(Data {
            fixed_max,
            format: Some(format),
            fill,
            axes,
            ..Default::default()
        }));
        let d = data.clone();
        let widget = area.clone();
        area.set_draw_func(move |_, cr, w, h| draw(cr, w as f64, h as f64, &d.borrow(), &widget));
        Chart { area, data }
    }

    pub fn set(&self, series: Vec<Series>, gaps: Vec<usize>, span_s: f64) {
        {
            let mut d = self.data.borrow_mut();
            d.series = series;
            d.gaps = gaps;
            d.span_s = span_s;
        }
        self.area.queue_draw();
    }

    /// Text alternative for assistive technologies and tooltips.
    pub fn set_summary(&self, text: &str) {
        self.area
            .update_property(&[gtk::accessible::Property::Label(text)]);
        self.area.set_tooltip_text(Some(text));
    }
}

fn ago(s: f64) -> String {
    if s < 1.0 {
        "now".into()
    } else if s >= 60.0 {
        let m = s / 60.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{:.0} min", m)
        } else {
            format!("{m:.1} min")
        }
    } else {
        format!("{s:.0} s")
    }
}

fn draw(cr: &gtk::cairo::Context, w: f64, h: f64, d: &Data, widget: &gtk::DrawingArea) {
    let fg = widget.color();
    let (fr, fgc, fb) = (fg.red() as f64, fg.green() as f64, fg.blue() as f64);
    let fmt = d.format.unwrap_or(|v| format!("{v:.0}"));
    let n = d.series.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let max = d.fixed_max.unwrap_or_else(|| {
        let m = d
            .series
            .iter()
            .flat_map(|s| s.values.iter().flatten())
            .fold(0.0f64, |a, b| a.max(*b));
        // Round the auto scale up so the top label is a readable number
        // (in binary units for byte rates: 1 MiB/s, not 977 KiB/s).
        nice_binary(m.max(1.0))
    });
    // Follow the user's text size (large-text settings) instead of a fixed
    // pixel size: ~85 % of the widget font, as GTK's caption style does.
    let fpx = label_px();
    cr.set_font_size(fpx);
    let (left, bottom) = if d.axes {
        let widest = [max, max / 2.0, 0.0]
            .iter()
            .map(|v| cr.text_extents(&fmt(*v)).map(|e| e.width()).unwrap_or(30.0))
            .fold(0.0, f64::max);
        (widest + 8.0, fpx + 6.0)
    } else {
        (0.0, 0.0)
    };
    let top = if d.axes { fpx * 0.6 } else { 2.0 };
    let pw = (w - left).max(1.0);
    let ph = (h - top - bottom).max(1.0);
    let y_of = |v: f64| top + ph * (1.0 - (v / max).clamp(0.0, 1.0));

    if d.axes {
        for (frac, v) in [(0.0, max), (0.5, max / 2.0), (1.0, 0.0)] {
            let y = top + ph * frac;
            cr.set_source_rgba(fr, fgc, fb, if frac == 1.0 { 0.25 } else { 0.08 });
            cr.set_line_width(1.0);
            cr.move_to(left, y.round() + 0.5);
            cr.line_to(w, y.round() + 0.5);
            let _ = cr.stroke();
            cr.set_source_rgba(fr, fgc, fb, 0.6);
            let label = fmt(v);
            let tw = cr.text_extents(&label).map(|e| e.width()).unwrap_or(0.0);
            cr.move_to(left - 6.0 - tw, y + 3.5);
            let _ = cr.show_text(&label);
        }
        // Time axis: round ticks (whole seconds/minutes) ending at "now".
        if d.span_s > 0.0 {
            cr.set_source_rgba(fr, fgc, fb, 0.6);
            let step = tick_step(d.span_s);
            let mut t = 0.0;
            while t <= d.span_s + 0.001 {
                let label = ago(t);
                let tw = cr.text_extents(&label).map(|e| e.width()).unwrap_or(0.0);
                let x = left + pw * (1.0 - t / d.span_s);
                cr.move_to((x - tw / 2.0).clamp(left, w - tw), h - 3.0);
                let _ = cr.show_text(&label);
                t += step;
            }
        }
    }
    if n < 2 {
        cr.set_source_rgba(fr, fgc, fb, 0.5);
        cr.move_to(left + pw / 2.0 - 30.0, top + ph / 2.0);
        let _ = cr.show_text("collecting…");
        return;
    }
    let x_of = |i: usize| left + pw * i as f64 / (n - 1) as f64;
    for &g in &d.gaps {
        if g < n {
            cr.set_source_rgba(fr, fgc, fb, 0.25);
            cr.set_dash(&[2.0, 3.0], 0.0);
            cr.move_to(x_of(g), top);
            cr.line_to(x_of(g), top + ph);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
    }
    for s in &d.series {
        let off = n - s.values.len();
        // Split into contiguous runs so gaps break both fill and line.
        let mut runs: Vec<Vec<(f64, f64)>> = vec![Vec::new()];
        for (i, v) in s.values.iter().enumerate() {
            let idx = i + off;
            if d.gaps.contains(&idx) && !runs.last().unwrap().is_empty() {
                runs.push(Vec::new());
            }
            match v {
                Some(v) => runs.last_mut().unwrap().push((x_of(idx), y_of(*v))),
                None if !runs.last().unwrap().is_empty() => runs.push(Vec::new()),
                None => {}
            }
        }
        for run in runs.iter().filter(|r| !r.is_empty()) {
            if d.fill && run.len() > 1 {
                let grad = gtk::cairo::LinearGradient::new(0.0, top, 0.0, top + ph);
                grad.add_color_stop_rgba(0.0, s.color.0, s.color.1, s.color.2, 0.35);
                grad.add_color_stop_rgba(1.0, s.color.0, s.color.1, s.color.2, 0.02);
                cr.move_to(run[0].0, top + ph);
                for (x, y) in run {
                    cr.line_to(*x, *y);
                }
                cr.line_to(run.last().unwrap().0, top + ph);
                cr.close_path();
                let _ = cr.set_source(&grad);
                let _ = cr.fill();
            }
            cr.set_source_rgb(s.color.0, s.color.1, s.color.2);
            cr.set_line_width(1.6);
            cr.set_line_join(gtk::cairo::LineJoin::Round);
            cr.move_to(run[0].0, run[0].1);
            for (x, y) in &run[1..] {
                cr.line_to(*x, *y);
            }
            if run.len() == 1 {
                cr.arc(run[0].0, run[0].1, 1.2, 0.0, std::f64::consts::TAU);
            }
            let _ = cr.stroke();
        }
    }
}

/// Tick spacing giving at most ~6 labels over `span` seconds.
fn tick_step(span: f64) -> f64 {
    [5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0]
        .into_iter()
        .find(|s| span / s <= 6.0)
        .unwrap_or(1200.0)
}

/// Like `nice_ceiling` but on a 1024-based scale.
fn nice_binary(v: f64) -> f64 {
    let k = (v.log2() / 10.0).floor().max(0.0);
    let unit = 1024f64.powf(k);
    let r = nice_ceiling(v / unit);
    // 1000 of a unit reads better as 1 of the next unit.
    if r >= 1000.0 { 1024.0 * unit } else { r * unit }
}

/// Axis label size in pixels: 10 px at the default 96 DPI, scaled by the
/// display DPI (gtk-xft-dpi), so large-text settings scale chart labels.
fn label_px() -> f64 {
    let dpi = gtk::Settings::default()
        .map(|s| s.gtk_xft_dpi())
        .filter(|d| *d > 0)
        .map_or(96.0, |d| d as f64 / 1024.0);
    (10.0 * dpi / 96.0).clamp(8.0, 30.0)
}

/// 1, 2, 5 × 10^k at or above `v`.
fn nice_ceiling(v: f64) -> f64 {
    let exp = 10f64.powf(v.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * exp >= v {
            return m * exp;
        }
    }
    10.0 * exp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_scales() {
        assert_eq!(nice_ceiling(1.0), 1.0);
        assert_eq!(nice_ceiling(3.2), 5.0);
        assert_eq!(nice_ceiling(130_000.0), 200_000.0);
        assert_eq!(nice_ceiling(999.0), 1000.0);
    }

    #[test]
    fn binary_scales_and_ticks() {
        assert_eq!(nice_binary(1_000_000.0), 1024.0 * 1024.0);
        assert_eq!(nice_binary(300.0), 500.0);
        assert_eq!(tick_step(300.0), 60.0);
        assert_eq!(tick_step(25.0), 5.0);
        assert_eq!(tick_step(600.0), 120.0);
    }

    #[test]
    fn time_labels() {
        assert_eq!(ago(0.0), "now");
        assert_eq!(ago(45.0), "45 s");
        assert_eq!(ago(300.0), "5 min");
        assert_eq!(ago(90.0), "1.5 min");
    }
}
