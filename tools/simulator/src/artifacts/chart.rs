//! Stacked time-series line charts as self-contained SVG or rasterized Canvas.

use super::canvas::Canvas;
use std::fmt::Write;

/// A time series for charts; `color` is a CSS colour (hex is required for `chart_png`).
pub struct Series {
    pub name: String,
    pub unit: String,
    pub color: String,
    pub points: Vec<(u64, Option<f64>)>,
}

const LEFT: f64 = 64.0;
const RIGHT: f64 = 16.0;
/// Room for three staggered rows of marker labels.
const TOP: f64 = 44.0;
const LABEL_ROWS: usize = 3;
const PANEL: f64 = 110.0;
const AXIS: f64 = 24.0;
const BACKGROUND: [u8; 3] = [0x14, 0x18, 0x1d];
const GRID: [u8; 3] = [0x2b, 0x32, 0x3a];
const INK: [u8; 3] = [0xc9, 0xd1, 0xd9];
const MARKER: [u8; 3] = [0xd2, 0x99, 0x22];

struct Layout {
    width: f64,
    t0: u64,
    span: f64,
    panels: usize,
}

impl Layout {
    fn new(series: &[Series], markers: &[(u64, String)], width: f64) -> Self {
        let times = series
            .iter()
            .flat_map(|s| s.points.iter().map(|p| p.0))
            .chain(markers.iter().map(|m| m.0));
        let t0 = times.clone().min().unwrap_or(0);
        let t1 = times.max().unwrap_or(0);
        Self {
            width,
            t0,
            span: (t1 - t0).max(1) as f64,
            panels: series.len(),
        }
    }

    fn height(&self) -> f64 {
        TOP + self.panels as f64 * PANEL + AXIS
    }

    fn x(&self, t: u64) -> f64 {
        LEFT + (t - self.t0) as f64 / self.span * (self.width - LEFT - RIGHT)
    }

    fn panel_top(&self, panel: usize) -> f64 {
        TOP + panel as f64 * PANEL
    }

    fn plot_top(&self, panel: usize) -> f64 {
        self.panel_top(panel) + 22.0
    }

    fn plot_bottom(&self, panel: usize) -> f64 {
        self.panel_top(panel) + PANEL - 10.0
    }

    fn axis_y(&self) -> f64 {
        TOP + self.panels as f64 * PANEL
    }

    fn y(&self, panel: usize, range: (f64, f64), v: f64) -> f64 {
        let (top, bottom) = (self.plot_top(panel), self.plot_bottom(panel));
        bottom - (v - range.0) / (range.1 - range.0) * (bottom - top)
    }

    fn ticks(&self) -> Vec<u64> {
        let target = ((self.width - LEFT - RIGHT) / 80.0).max(1.0);
        let seconds = self.span / 1000.0;
        let step = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600]
            .into_iter()
            .find(|&s| seconds / s as f64 <= target)
            .unwrap_or(7200)
            * 1000;
        (self.t0.div_ceil(step)..)
            .map(|n| n * step)
            .take_while(|&t| t as f64 <= self.t0 as f64 + self.span)
            .collect()
    }
}

fn value_range(series: &Series) -> Option<(f64, f64)> {
    let mut values = series.points.iter().filter_map(|p| p.1);
    let first = values.next()?;
    Some(values.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
}

fn plot_range(range: Option<(f64, f64)>) -> (f64, f64) {
    match range {
        Some((lo, hi)) if hi > lo => (lo, hi),
        Some((lo, _)) => (lo - 1.0, lo + 1.0),
        None => (0.0, 1.0),
    }
}

fn segments(series: &Series) -> Vec<Vec<(u64, f64)>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    for &(t, v) in &series.points {
        match v {
            Some(v) => current.push((t, v)),
            None => out.push(std::mem::take(&mut current)),
        }
    }
    out.push(current);
    out.retain(|s| !s.is_empty());
    out
}

fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn number(v: f64) -> String {
    if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

fn legend(series: &Series) -> String {
    if series.unit.is_empty() {
        series.name.clone()
    } else {
        format!("{} ({})", series.name, series.unit)
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn hex(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

fn parse_css_hex(css: &str) -> [u8; 3] {
    let digits = css.trim().trim_start_matches('#');
    let parse = |s: &str| u8::from_str_radix(s, 16).ok();
    match digits.len() {
        6 => (|| {
            Some([
                parse(&digits[0..2])?,
                parse(&digits[2..4])?,
                parse(&digits[4..6])?,
            ])
        })(),
        3 => (|| {
            Some([
                parse(&digits[0..1])? * 17,
                parse(&digits[1..2])? * 17,
                parse(&digits[2..3])? * 17,
            ])
        })(),
        _ => None,
    }
    .unwrap_or([0x4c, 0xc2, 0xff])
}

pub fn chart_svg(series: &[Series], markers: &[(u64, String)], width: u32) -> String {
    let layout = Layout::new(series, markers, f64::from(width));
    let height = layout.height();
    let (grid, ink) = (hex(GRID), hex(INK));
    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" font-family=\"monospace\" font-size=\"11\">\n<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>\n",
        hex(BACKGROUND)
    );
    let right = layout.width - RIGHT;
    for tick in layout.ticks() {
        let x = layout.x(tick);
        let _ = writeln!(
            svg,
            "<line x1=\"{x:.1}\" y1=\"{TOP}\" x2=\"{x:.1}\" y2=\"{:.1}\" stroke=\"{grid}\"/><text x=\"{x:.1}\" y=\"{:.1}\" fill=\"{ink}\" text-anchor=\"middle\">{}</text>",
            layout.axis_y(),
            layout.axis_y() + 15.0,
            mmss(tick)
        );
    }
    for (i, s) in series.iter().enumerate() {
        let range = value_range(s);
        let (lo, hi) = plot_range(range);
        let _ = writeln!(
            svg,
            "<rect x=\"{LEFT}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"none\" stroke=\"{grid}\"/>\n<text x=\"{LEFT}\" y=\"{:.1}\" fill=\"{}\">{}</text>",
            layout.plot_top(i),
            right - LEFT,
            layout.plot_bottom(i) - layout.plot_top(i),
            layout.panel_top(i) + 14.0,
            escape(&s.color),
            escape(&legend(s))
        );
        if range.is_some() {
            let _ = writeln!(
                svg,
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{ink}\" text-anchor=\"end\">{}</text><text x=\"{:.1}\" y=\"{:.1}\" fill=\"{ink}\" text-anchor=\"end\">{}</text>",
                LEFT - 6.0,
                layout.plot_top(i) + 4.0,
                number(range.map_or(hi, |r| r.1)),
                LEFT - 6.0,
                layout.plot_bottom(i) + 4.0,
                number(range.map_or(lo, |r| r.0)),
            );
        }
        for segment in segments(s) {
            let points: Vec<String> = segment
                .iter()
                .map(|&(t, v)| format!("{:.1},{:.1}", layout.x(t), layout.y(i, (lo, hi), v)))
                .collect();
            if let [only] = points.as_slice() {
                let (cx, cy) = only.split_once(',').unwrap_or(("0", "0"));
                let _ = writeln!(
                    svg,
                    "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"2\" fill=\"{}\"/>",
                    escape(&s.color)
                );
            } else {
                let _ = writeln!(
                    svg,
                    "<polyline points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2\" stroke-linejoin=\"round\"/>",
                    points.join(" "),
                    escape(&s.color)
                );
            }
        }
    }
    let marker = hex(MARKER);
    for ((t, label), row) in markers.iter().zip(label_rows(&layout, markers, 7.0)) {
        let x = layout.x(*t);
        let _ = writeln!(
            svg,
            "<line x1=\"{x:.1}\" y1=\"{TOP}\" x2=\"{x:.1}\" y2=\"{:.1}\" stroke=\"{marker}\" stroke-dasharray=\"4 3\"/>",
            layout.axis_y(),
        );
        if let Some(row) = row {
            let _ = writeln!(
                svg,
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{marker}\">{}</text>",
                x + 3.0,
                12.0 + row as f64 * 11.0,
                escape(label)
            );
        }
    }
    svg.push_str("</svg>");
    svg
}

pub fn chart_png(series: &[Series], markers: &[(u64, String)], width: usize) -> Canvas {
    let layout = Layout::new(series, markers, width as f64);
    let mut canvas = Canvas::new(width, layout.height() as usize, BACKGROUND);
    let right = layout.width - RIGHT;
    for tick in layout.ticks() {
        let x = layout.x(tick) as usize;
        canvas.fill_rect(x, TOP as usize, 1, (layout.axis_y() - TOP) as usize, GRID);
        let label = mmss(tick);
        let label_x = x.saturating_sub(label.len() * 3);
        canvas.text(label_x, layout.axis_y() as usize + 8, 1, INK, &label);
    }
    for (i, s) in series.iter().enumerate() {
        let range = value_range(s);
        let (lo, hi) = plot_range(range);
        let color = parse_css_hex(&s.color);
        let (top, bottom) = (layout.plot_top(i) as usize, layout.plot_bottom(i) as usize);
        let frame = (LEFT as usize, top, (right - LEFT) as usize, bottom - top);
        canvas.fill_rect(frame.0, frame.1, frame.2, 1, GRID);
        canvas.fill_rect(frame.0, frame.1 + frame.3, frame.2, 1, GRID);
        canvas.fill_rect(frame.0, frame.1, 1, frame.3, GRID);
        canvas.fill_rect(frame.0 + frame.2, frame.1, 1, frame.3 + 1, GRID);
        canvas.text(
            LEFT as usize,
            layout.panel_top(i) as usize + 8,
            1,
            color,
            &legend(s),
        );
        if let Some((min, max)) = range {
            for (v, y) in [
                (max, top.saturating_sub(3)),
                (min, bottom.saturating_sub(3)),
            ] {
                let label = number(v);
                let x = (LEFT as usize).saturating_sub(6 + label.len() * 6);
                canvas.text(x, y, 1, INK, &label);
            }
        }
        for segment in segments(s) {
            let pixels: Vec<(i64, i64)> = segment
                .iter()
                .map(|&(t, v)| {
                    (
                        layout.x(t).round() as i64,
                        layout.y(i, (lo, hi), v).round() as i64,
                    )
                })
                .collect();
            if let [(x, y)] = pixels[..] {
                dot(&mut canvas, x, y, color);
            }
            for pair in pixels.windows(2) {
                line(&mut canvas, pair[0], pair[1], color);
            }
        }
    }
    for ((t, label), row) in markers.iter().zip(label_rows(&layout, markers, 6.0)) {
        let x = layout.x(*t) as usize;
        let mut y = TOP as usize;
        while y < layout.axis_y() as usize {
            canvas.fill_rect(x, y, 1, 4, MARKER);
            y += 7;
        }
        if let Some(row) = row {
            canvas.text(x + 3, 6 + row * 11, 1, MARKER, label);
        }
    }
    canvas
}

/// The label row for each marker, or none when every row is taken there.
fn label_rows(layout: &Layout, markers: &[(u64, String)], char_width: f64) -> Vec<Option<usize>> {
    let mut ends = [f64::MIN; LABEL_ROWS];
    markers
        .iter()
        .map(|(t, label)| {
            let x = layout.x(*t) + 3.0;
            let row = ends.iter().position(|&end| x > end + 6.0)?;
            ends[row] = x + label.len() as f64 * char_width;
            Some(row)
        })
        .collect()
}

fn dot(canvas: &mut Canvas, x: i64, y: i64, color: [u8; 3]) {
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        if let (Ok(px), Ok(py)) = (usize::try_from(x + dx), usize::try_from(y + dy)) {
            canvas.set(px, py, color);
        }
    }
}

/// Bresenham line drawn with a 2x2 brush.
fn line(canvas: &mut Canvas, from: (i64, i64), to: (i64, i64), color: [u8; 3]) {
    let (mut x, mut y) = from;
    let (dx, dy) = ((to.0 - x).abs(), -(to.1 - y).abs());
    let (sx, sy) = (
        (to.0 - x).signum().max(0) * 2 - 1,
        (to.1 - y).signum().max(0) * 2 - 1,
    );
    let mut error = dx + dy;
    loop {
        dot(canvas, x, y, color);
        if (x, y) == to {
            return;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Series> {
        vec![
            Series {
                name: "Power".into(),
                unit: "W".into(),
                color: "#ff8800".into(),
                points: vec![
                    (0, Some(100.0)),
                    (1000, Some(250.0)),
                    (2000, None),
                    (3000, Some(180.0)),
                    (4000, Some(190.0)),
                ],
            },
            Series {
                name: "Cadence <rpm>".into(),
                unit: "rpm".into(),
                color: "#4cc2ff".into(),
                points: vec![(0, Some(80.0)), (4000, Some(90.0))],
            },
        ]
    }

    #[test]
    fn svg_lists_series_markers_and_is_well_formed() {
        let svg = chart_svg(&sample(), &[(2000, "Start".into())], 640);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("Power (W)"));
        assert!(svg.contains("Cadence &lt;rpm&gt; (rpm)"));
        assert!(svg.contains("Start"));
        assert!(svg.contains("00:02"));
        assert_eq!(svg.matches("<polyline").count(), 3);
    }

    #[test]
    fn png_chart_has_expected_size_and_draws_lines() {
        let canvas = chart_png(&sample(), &[(2000, "Start".into())], 640);
        assert_eq!(canvas.width, 640);
        assert_eq!(canvas.height, (TOP + 2.0 * PANEL + AXIS) as usize);
        assert_eq!(canvas.rgb.len(), canvas.width * canvas.height * 3);
        assert!(canvas.rgb.as_chunks::<3>().0.contains(&[0xff, 0x88, 0x00]));
        assert!(canvas.rgb.as_chunks::<3>().0.contains(&[0x4c, 0xc2, 0xff]));
    }

    #[test]
    fn empty_input_still_renders() {
        assert!(chart_svg(&[], &[], 320).ends_with("</svg>"));
        assert_eq!(chart_png(&[], &[], 320).width, 320);
    }
}
