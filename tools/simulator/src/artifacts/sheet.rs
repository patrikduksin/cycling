use super::Canvas;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct Spec {
    pub title: String,
    /// Panels as `<w>x<h>`, in column order.
    pub panels: Vec<String>,
    pub rows: Vec<Row>,
}

#[derive(Deserialize)]
pub struct Row {
    pub label: String,
    pub cells: BTreeMap<String, Cell>,
}

#[derive(Deserialize)]
pub struct Cell {
    pub state: String,
    pub before: Option<PathBuf>,
    pub after: Option<PathBuf>,
}

const PAD: usize = 12;
const GAP: usize = 4;
const SCALE: usize = 2;
const LINE: usize = 7 * SCALE + 8;
const FRAME: usize = 3;
const BACKGROUND: [u8; 3] = [24, 24, 28];
const INK: [u8; 3] = [230, 230, 230];
const DIM: [u8; 3] = [140, 140, 140];
const EMPTY: [u8; 3] = [48, 48, 52];

fn frame(state: &str) -> [u8; 3] {
    match state {
        "changed" => [235, 64, 52],
        "unchanged" => [70, 70, 76],
        _ => [240, 170, 40],
    }
}

fn size(panel: &str) -> Result<(usize, usize), String> {
    panel
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .ok_or_else(|| format!("panel {panel:?} is not <w>x<h>"))
}

fn read(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rgb = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut rgb)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if info.color_type != png::ColorType::Rgb || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("{}: not an 8-bit RGB PNG", path.display()));
    }
    Ok((info.width as usize, info.height as usize, rgb))
}

fn paste(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    image: Option<&Path>,
    (w, h): (usize, usize),
) -> Result<(), String> {
    let Some(path) = image else {
        canvas.fill_rect(x, y, w, h, EMPTY);
        canvas.text(x + PAD, y + PAD, SCALE, DIM, "NONE");
        return Ok(());
    };
    let (width, height, rgb) = read(path)?;
    for row in 0..height {
        for col in 0..width {
            let i = (row * width + col) * 3;
            canvas.set(x + col, y + row, [rgb[i], rgb[i + 1], rgb[i + 2]]);
        }
    }
    Ok(())
}

pub fn contact_sheet(spec: &Spec) -> Result<Vec<u8>, String> {
    let panels = spec
        .panels
        .iter()
        .map(|p| size(p))
        .collect::<Result<Vec<_>, _>>()?;
    let column = |(w, _): (usize, usize)| 2 * (w + 2 * FRAME) + GAP;
    let width = PAD + panels.iter().map(|&p| column(p) + PAD).sum::<usize>();
    let tallest = panels.iter().map(|&(_, h)| h).max().unwrap_or(0) + 2 * FRAME;
    let row_height = 2 * LINE + tallest + PAD;
    let height = PAD + LINE + spec.rows.len() * row_height;
    let mut canvas = Canvas::new(width, height, BACKGROUND);
    canvas.text(PAD, PAD, SCALE, INK, &spec.title);
    for (index, row) in spec.rows.iter().enumerate() {
        let top = PAD + LINE + index * row_height;
        canvas.text(PAD, top + 4, SCALE, INK, &row.label);
        let mut x = PAD;
        for (name, &panel) in spec.panels.iter().zip(&panels) {
            let y = top + LINE;
            if let Some(cell) = row.cells.get(name) {
                let color = frame(&cell.state);
                let ink = if cell.state == "unchanged" {
                    DIM
                } else {
                    color
                };
                let label = format!("{name} {}", cell.state);
                canvas.text(x, y + 4, SCALE, ink, &label);
                for (side, image) in [cell.before.as_deref(), cell.after.as_deref()]
                    .into_iter()
                    .enumerate()
                {
                    let left = x + side * (panel.0 + 2 * FRAME + GAP);
                    canvas.fill_rect(
                        left,
                        y + LINE,
                        panel.0 + 2 * FRAME,
                        panel.1 + 2 * FRAME,
                        color,
                    );
                    paste(&mut canvas, left + FRAME, y + LINE + FRAME, image, panel)?;
                }
            }
            x += column(panel) + PAD;
        }
    }
    Ok(canvas.png())
}

pub fn write_contact_sheet(spec: &Path, out: &Path) -> Result<(), String> {
    let text = std::fs::read(spec).map_err(|e| format!("{}: {e}", spec.display()))?;
    let spec: Spec =
        serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", spec.display()))?;
    std::fs::write(out, contact_sheet(&spec)?).map_err(|e| format!("{}: {e}", out.display()))
}
