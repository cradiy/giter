use std::{
    io::{self, Write},
    sync::Arc,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use ratatui::layout::Rect;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::graph::Layout;

const COLORS: [(u8, u8, u8); 6] = [
    (148, 226, 213),
    (203, 166, 247),
    (250, 179, 135),
    (137, 180, 250),
    (166, 227, 161),
    (249, 226, 175),
];

#[derive(Clone)]
pub struct Scene {
    pub area: Rect,
    pub layout: Arc<Layout>,
    pub visible: Vec<usize>,
    pub selected: Option<usize>,
    pub filtered: bool,
}

impl Scene {
    fn same(&self, other: &Self) -> bool {
        self.area == other.area
            && Arc::ptr_eq(&self.layout, &other.layout)
            && self.visible == other.visible
            && self.selected == other.selected
            && self.filtered == other.filtered
    }

    pub fn raster(&self, cell: (u32, u32)) -> io::Result<Pixmap> {
        let width = u32::from(self.area.width).saturating_mul(cell.0);
        let height = u32::from(self.area.height).saturating_mul(cell.1);
        let padding = cell.1.saturating_mul(2);
        let canvas_height = height.saturating_add(padding.saturating_mul(2));
        if width.saturating_mul(canvas_height) > 16 * 1024 * 1024 {
            return Err(io::Error::other("Graph image exceeds pixel limit"));
        }
        let mut image = Pixmap::new(width, canvas_height)
            .ok_or_else(|| io::Error::other("Cannot allocate graph image"))?;
        let Some(&first) = self.visible.first() else {
            return Pixmap::new(width, height)
                .ok_or_else(|| io::Error::other("Invalid graph viewport"));
        };
        let spacing = width as f32 / (self.layout.lanes.max(1) + 1) as f32;
        let row = cell.1 as f32;
        let xy = |index: usize, lane: usize| {
            (
                (lane + 1) as f32 * spacing,
                (index as f32 - first as f32) * 2.0 * row + row * 0.5 + padding as f32,
            )
        };
        let thickness = (cell.0 as f32 * 0.18).clamp(1.6, 3.0);
        if !self.filtered {
            for edge in &self.layout.edges {
                let (x1, y1) = xy(edge.from, self.layout.nodes[edge.from].lane);
                let lane = self
                    .layout
                    .nodes
                    .get(edge.to)
                    .map_or(edge.lane, |node| node.lane);
                let (x2, y2) = xy(edge.to, lane);
                if y2 < -row || y1 > canvas_height as f32 + row {
                    continue;
                }
                let mut path = PathBuilder::new();
                path.move_to(x1, y1);
                let bend = (y2 - y1).min(row * 2.0).max(0.0);
                path.line_to(x1, y2 - bend);
                path.cubic_to(x1, y2 - bend * 0.4, x2, y2 - bend * 0.6, x2, y2);
                if let Some(path) = path.finish() {
                    let (r, g, b) = COLORS[lane % COLORS.len()];
                    let mut paint = Paint::default();
                    paint.set_color_rgba8(r, g, b, 210);
                    image.stroke_path(
                        &path,
                        &paint,
                        &Stroke {
                            width: thickness,
                            line_cap: tiny_skia::LineCap::Round,
                            ..Stroke::default()
                        },
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
        for (position, &index) in self.visible.iter().enumerate() {
            let node = &self.layout.nodes[index];
            let (x, y) = if self.filtered {
                (
                    spacing,
                    position as f32 * row * 2.0 + row * 0.5 + padding as f32,
                )
            } else {
                xy(index, node.lane)
            };
            let radius = (cell.0 as f32 * 0.36).clamp(3.0, 6.0).min(spacing * 0.35);
            let selected = self.selected == Some(index);
            let mut paint = Paint::default();
            let bg = if selected { (30, 30, 46) } else { (24, 24, 37) };
            paint.set_color_rgba8(bg.0, bg.1, bg.2, 255);
            if let Some(circle) = PathBuilder::from_circle(x, y, radius + thickness) {
                image.fill_path(
                    &circle,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let (r, g, b) = COLORS[node.lane % COLORS.len()];
            paint.set_color_rgba8(r, g, b, 255);
            if let Some(circle) = PathBuilder::from_circle(x, y, radius) {
                if node.merge || selected {
                    image.fill_path(
                        &circle,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                } else {
                    image.stroke_path(
                        &circle,
                        &paint,
                        &Stroke {
                            width: thickness,
                            ..Stroke::default()
                        },
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
        image
            .clone_rect(
                tiny_skia::IntRect::from_xywh(0, padding as i32, width, height)
                    .ok_or_else(|| io::Error::other("Invalid graph viewport"))?,
            )
            .ok_or_else(|| io::Error::other("Cannot crop graph viewport"))
    }
}

pub struct Graphics {
    pub scene: Option<Scene>,
    cached: Option<Scene>,
    cell: (u32, u32),
    id: u32,
    visible: bool,
    uploaded: bool,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            scene: None,
            cached: None,
            cell: (0, 0),
            id: 0x4700_0000 | (std::process::id() & 0x00ff_ffff),
            visible: false,
            uploaded: false,
        }
    }
}

impl Graphics {
    pub fn release(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.uploaded {
            write!(out, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", self.id)?;
            out.flush()?;
            self.uploaded = false;
            self.visible = false;
        }
        Ok(())
    }
    pub fn hide(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.visible {
            write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", self.id)?;
            self.visible = false;
        }
        Ok(())
    }

    pub fn present(&mut self, out: &mut impl Write) -> io::Result<()> {
        let Some(scene) = &self.scene else {
            return Ok(());
        };
        if scene.area.is_empty() || scene.visible.is_empty() {
            return Ok(());
        }
        let cell = crossterm::terminal::window_size()
            .ok()
            .filter(|size| size.width > 0 && size.height > 0 && size.columns > 0 && size.rows > 0)
            .map_or((12, 24), |size| {
                (
                    u32::from(size.width) / u32::from(size.columns),
                    u32::from(size.height) / u32::from(size.rows),
                )
            });
        let changed = self.cell != cell
            || self
                .cached
                .as_ref()
                .is_none_or(|cached| !scene.same(cached));
        if changed {
            let image = scene.raster(cell)?;
            let png = image.encode_png().map_err(io::Error::other)?;
            let encoded = STANDARD.encode(png);
            let chunks = encoded.as_bytes().chunks(4096);
            let count = chunks.len();
            for (index, chunk) in chunks.enumerate() {
                if index == 0 {
                    write!(
                        out,
                        "\x1b_Ga=t,f=100,t=d,i={},q=2,m={};",
                        self.id,
                        usize::from(index + 1 < count)
                    )?;
                } else {
                    write!(out, "\x1b_Gm={};", usize::from(index + 1 < count))?;
                }
                out.write_all(chunk)?;
                out.write_all(b"\x1b\\")?;
            }
            self.cached = Some(scene.clone());
            self.cell = cell;
            self.uploaded = true;
        }
        write!(
            out,
            "\x1b7\x1b[{};{}H\x1b_Ga=p,i={},p=1,c={},r={},C=1,z=-1,q=2\x1b\\\x1b8",
            scene.area.y + 1,
            scene.area.x + 1,
            self.id,
            scene.area.width,
            scene.area.height
        )?;
        self.visible = true;
        out.flush()
    }
}

impl Drop for Graphics {
    fn drop(&mut self) {
        if self.uploaded {
            let mut out = io::stdout().lock();
            let _ = self.release(&mut out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Edge, Node};

    fn scene() -> Scene {
        Scene {
            area: Rect::new(4, 3, 8, 8),
            visible: vec![0, 1, 2, 3],
            selected: Some(2),
            filtered: false,
            layout: Arc::new(Layout {
                nodes: vec![
                    Node {
                        lane: 0,
                        merge: true,
                    },
                    Node {
                        lane: 0,
                        merge: false,
                    },
                    Node {
                        lane: 1,
                        merge: false,
                    },
                    Node {
                        lane: 0,
                        merge: false,
                    },
                ],
                edges: vec![
                    Edge {
                        from: 0,
                        to: 1,
                        lane: 0,
                    },
                    Edge {
                        from: 0,
                        to: 2,
                        lane: 1,
                    },
                    Edge {
                        from: 1,
                        to: 3,
                        lane: 0,
                    },
                    Edge {
                        from: 2,
                        to: 3,
                        lane: 0,
                    },
                ],
                lanes: 2,
            }),
        }
    }

    #[test]
    fn bezier_viewport_matches_scrolled_crop_and_preserves_transparency() {
        let full = scene();
        let full_image = full.raster((12, 24)).unwrap();
        let mut scrolled = full.clone();
        scrolled.visible = vec![2, 3];
        scrolled.area.height = 4;
        let crop = scrolled.raster((12, 24)).unwrap();
        let start = (96 * full_image.width() * 4) as usize;
        let differences: Vec<_> = crop
            .data()
            .iter()
            .zip(&full_image.data()[start..])
            .map(|(a, b)| a.abs_diff(*b))
            .collect();
        let maximum = differences.iter().copied().max().unwrap_or(0);
        let changed = differences
            .iter()
            .filter(|difference| **difference != 0)
            .count();
        assert!(
            maximum <= 2 && changed < crop.data().len() / 100,
            "Scrolled raster differs: max {maximum}, changed {changed}"
        );
        assert!(
            full_image
                .pixels()
                .iter()
                .any(|pixel| pixel.alpha() > 0 && pixel.alpha() < 210)
        );
        assert!(full_image.pixels().iter().any(|pixel| pixel.alpha() == 0));
    }

    #[test]
    fn kitty_packets_are_chunked_cached_and_deleted_by_owned_id() {
        let mut graphics = Graphics::default();
        graphics.scene = Some(scene());
        let mut output = Vec::new();
        graphics.present(&mut output).unwrap();
        let text = std::str::from_utf8(&output).unwrap();
        let mut encoded = String::new();
        for packet in text.split("\x1b_G").skip(1) {
            let packet = packet.split("\x1b\\").next().unwrap();
            if let Some((_, data)) = packet.split_once(';') {
                assert!(data.len() <= 4096);
                encoded.push_str(data);
            }
        }
        let png = STANDARD.decode(encoded).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(text.contains("C=1,z=-1,q=2"));
        output.clear();
        graphics.hide(&mut output).unwrap();
        graphics.present(&mut output).unwrap();
        let text = std::str::from_utf8(&output).unwrap();
        assert!(!text.contains("a=t"));
        assert!(text.contains("a=p"));
        output.clear();
        graphics.release(&mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", graphics.id)
        );
    }
}
