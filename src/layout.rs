//! Geometry and snapping for the layout map.

use ratatui::layout::Rect;

use crate::output::Output;

/// A terminal cell is about twice as tall as it is wide.
const CELL_ASPECT: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapMode {
    Off,
    Edges,
    Centers,
    Both,
}

impl SnapMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Edges => "Edges",
            Self::Centers => "Centers",
            Self::Both => "Both",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Edges,
            Self::Edges => Self::Centers,
            Self::Centers => Self::Both,
            Self::Both => Self::Off,
        }
    }

    fn edges(self) -> bool {
        matches!(self, Self::Edges | Self::Both)
    }

    fn centers(self) -> bool {
        matches!(self, Self::Centers | Self::Both)
    }
}

/// An alignment guide at a logical coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guide {
    Vertical(i32),
    Horizontal(i32),
}

/// Where each output is drawn, in cells relative to the map area.
#[derive(Debug, Clone)]
pub struct Geometry {
    scale: f64,
    min_x: i32,
    min_y: i32,
    offset_x: f64,
    offset_y: f64,
    /// By output index; `None` for outputs without a mode.
    pub boxes: Vec<Option<Rect>>,
}

impl Geometry {
    /// Fit every output with a mode into `width` x `height` cells.
    pub fn new(outputs: &[Output], width: u16, height: u16) -> Option<Self> {
        let sizes: Vec<_> = outputs.iter().map(Output::logical_size).collect();
        let drawn = || {
            outputs
                .iter()
                .zip(&sizes)
                .filter(|(_, size)| size.0 > 0 && size.1 > 0)
        };
        let min_x = drawn().map(|(o, _)| o.position.x).min()?;
        let min_y = drawn().map(|(o, _)| o.position.y).min()?;
        let max_x = drawn().map(|(o, s)| o.position.x + s.0).max()?;
        let max_y = drawn().map(|(o, s)| o.position.y + s.1).max()?;
        let span_x = f64::from(max_x - min_x).max(1.0);
        let span_y = f64::from(max_y - min_y).max(1.0);

        let (area_w, area_h) = (f64::from(width), f64::from(height));
        let scale = (area_w / (span_x * CELL_ASPECT)).min(area_h / span_y);
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let offset_x = ((area_w - span_x * CELL_ASPECT * scale) / 2.0).max(0.0);
        let offset_y = ((area_h - span_y * scale) / 2.0).max(0.0);

        let mut geometry = Self {
            scale,
            min_x,
            min_y,
            offset_x,
            offset_y,
            boxes: Vec::new(),
        };
        geometry.boxes = outputs
            .iter()
            .zip(&sizes)
            .map(|(output, size)| {
                if size.0 <= 0 || size.1 <= 0 {
                    return None;
                }
                let (left, top) = geometry.to_cell(output.position.x, output.position.y);
                let (right, bottom) =
                    geometry.to_cell(output.position.x + size.0, output.position.y + size.1);
                let left = left.clamp(0, i32::from(width) - 1);
                let top = top.clamp(0, i32::from(height) - 1);
                let right = right.clamp(left + 1, i32::from(width).max(left + 1));
                let bottom = bottom.clamp(top + 1, i32::from(height).max(top + 1));
                Some(Rect::new(
                    left as u16,
                    top as u16,
                    (right - left) as u16,
                    (bottom - top) as u16,
                ))
            })
            .collect();
        Some(geometry)
    }

    /// Output indexes, biggest box first and `selected` last, so small
    /// outputs and the selection are never hidden.
    pub fn draw_order(&self, selected: usize) -> Vec<usize> {
        let mut order: Vec<_> = (0..self.boxes.len())
            .filter(|&i| self.boxes[i].is_some())
            .collect();
        order.sort_by_key(|&i| {
            let area = self.boxes[i].map_or(0, |rect| rect.area());
            (i == selected, std::cmp::Reverse(area))
        });
        order
    }

    /// Logical coordinates to map cells (possibly off the map).
    pub fn to_cell(&self, x: i32, y: i32) -> (i32, i32) {
        let column = self.offset_x + f64::from(x - self.min_x) * CELL_ASPECT * self.scale;
        let row = self.offset_y + f64::from(y - self.min_y) * self.scale;
        (
            column.round_ties_even() as i32,
            row.round_ties_even() as i32,
        )
    }

    /// How many logical pixels a move of `columns` x `rows` cells covers.
    pub fn to_pixels(&self, columns: i32, rows: i32) -> (f64, f64) {
        (
            f64::from(columns) / (self.scale * CELL_ASPECT),
            f64::from(rows) / self.scale,
        )
    }
}

/// Snap a proposed position for `outputs[index]` to the other outputs and
/// the origin, and return the guides to show.
pub fn snap(
    outputs: &[Output],
    index: usize,
    x: i32,
    y: i32,
    mode: SnapMode,
    threshold: i32,
) -> (i32, i32, Vec<Guide>) {
    if mode == SnapMode::Off {
        return (x, y, Vec::new());
    }
    let (width, height) = outputs[index].logical_size();
    let near = |a: i32, b: i32| (a - b).abs() < threshold;
    let (mut new_x, mut new_y) = (x, y);
    let mut guides = Vec::new();
    let mut guide = |guide| {
        if !guides.contains(&guide) {
            guides.push(guide);
        }
    };

    let others = outputs
        .iter()
        .enumerate()
        .filter(|(i, other)| *i != index && other.enabled);
    for (_, other) in others {
        let (ox, oy) = (other.position.x, other.position.y);
        let (ow, oh) = other.logical_size();
        if mode.edges() {
            if near(x, ox + ow) {
                new_x = ox + ow;
                guide(Guide::Vertical(new_x));
            } else if near(x + width, ox) {
                new_x = ox - width;
                guide(Guide::Vertical(ox));
            } else if near(x, ox) {
                new_x = ox;
                guide(Guide::Vertical(new_x));
            }
            if near(y, oy + oh) {
                new_y = oy + oh;
                guide(Guide::Horizontal(new_y));
            } else if near(y + height, oy) {
                new_y = oy - height;
                guide(Guide::Horizontal(oy));
            } else if near(y, oy) {
                new_y = oy;
                guide(Guide::Horizontal(new_y));
            }
        }
        if mode.centers() {
            let (center_x, other_x) = (x + width / 2, ox + ow / 2);
            let (center_y, other_y) = (y + height / 2, oy + oh / 2);
            if near(center_x, other_x) {
                new_x = other_x - width / 2;
                guide(Guide::Vertical(other_x));
            }
            if near(center_y, other_y) {
                new_y = other_y - height / 2;
                guide(Guide::Horizontal(other_y));
            }
        }
    }

    // The origin is always a snap target.
    if x.abs() < threshold {
        new_x = 0;
        guide(Guide::Vertical(0));
    }
    if y.abs() < threshold {
        new_y = 0;
        guide(Guide::Horizontal(0));
    }
    (new_x, new_y, guides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::tests::fixture_outputs;

    #[test]
    fn side_by_side_outputs_do_not_overlap() {
        let outputs = fixture_outputs();
        let geometry = Geometry::new(&outputs, 120, 30).unwrap();
        let (first, second) = (geometry.boxes[0].unwrap(), geometry.boxes[1].unwrap());
        assert!(first.right() <= second.x + 1);
        assert!(second.right() <= 120 && second.bottom() <= 30);
    }

    #[test]
    fn outputs_without_a_mode_are_not_drawn() {
        let mut outputs = fixture_outputs();
        outputs[0].mode = None;
        assert!(Geometry::new(&outputs, 100, 30).unwrap().boxes[0].is_none());
        outputs[1].mode = None;
        assert!(Geometry::new(&outputs, 100, 30).is_none());
    }

    #[test]
    fn snaps_to_edges_and_origin_but_not_when_off_or_far() {
        let outputs = fixture_outputs();
        // eDP-1 sits to the right of HDMI-A-1 (1920 wide).
        let (x, y, guides) = snap(&outputs, 1, 1915, 4, SnapMode::Both, 8);
        assert_eq!((x, y), (1920, 0));
        assert!(guides.contains(&Guide::Vertical(1920)));
        assert!(guides.contains(&Guide::Horizontal(0)));

        assert_eq!(snap(&outputs, 1, 1915, 4, SnapMode::Off, 8).0, 1915);
        let (x, y, guides) = snap(&outputs, 1, 3000, 4000, SnapMode::Both, 8);
        assert_eq!((x, y), (3000, 4000));
        assert!(guides.is_empty());
    }
}
