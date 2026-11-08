//! Geometry, snapping and alignment-guide helpers for the layout map.

use crate::model::Output;

/// A terminal cell is roughly twice as tall as it is wide. Multiplying the
/// horizontal scale by this factor keeps monitor boxes looking like monitors.
pub const CELL_ASPECT: f64 = 2.0;

/// How a moved output should align with its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapMode {
    Off,
    Edges,
    Centers,
    Both,
}

impl SnapMode {
    pub const ALL: [SnapMode; 4] = [Self::Off, Self::Edges, Self::Centers, Self::Both];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Edges => "Edges",
            Self::Centers => "Centers",
            Self::Both => "Both",
        }
    }

    pub fn cycled(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn from_label(label: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|mode| mode.label().eq_ignore_ascii_case(label))
            .unwrap_or(Self::Edges)
    }

    fn edges(self) -> bool {
        matches!(self, Self::Edges | Self::Both)
    }

    fn centers(self) -> bool {
        matches!(self, Self::Centers | Self::Both)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

/// An alignment guide expressed in logical output coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guide {
    pub orientation: Orientation,
    pub value: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    pub fn right(&self) -> i64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> i64 {
        self.y + self.height
    }

    pub fn contains(&self, px: i64, py: i64) -> bool {
        self.x <= px && px < self.right() && self.y <= py && py < self.bottom()
    }
}

/// Mapping between logical output coordinates and terminal cells.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutGeometry {
    pub scale: f64,
    pub min_x: i64,
    pub min_y: i64,
    pub offset_x: f64,
    pub offset_y: f64,
    /// Cell rectangles by output name, in output order.
    pub boxes: Vec<(String, Rect)>,
    pub cell_aspect: f64,
}

impl LayoutGeometry {
    pub fn get(&self, name: &str) -> Option<Rect> {
        self.boxes
            .iter()
            .find(|(box_name, _)| box_name == name)
            .map(|(_, rect)| *rect)
    }

    pub fn cell_delta_to_pixels(&self, dx: i64, dy: i64) -> (f64, f64) {
        if self.scale <= 0.0 {
            return (0.0, 0.0);
        }
        (
            dx as f64 / (self.scale * self.cell_aspect),
            dy as f64 / self.scale,
        )
    }

    /// Convert logical pixels to cell coordinates (may be off-screen).
    pub fn world_to_cell(&self, x: i64, y: i64) -> (i64, i64) {
        let column = self.offset_x + (x - self.min_x) as f64 * self.cell_aspect * self.scale;
        let row = self.offset_y + (y - self.min_y) as f64 * self.scale;
        (
            column.round_ties_even() as i64,
            row.round_ties_even() as i64,
        )
    }
}

/// Compute cell rectangles for every output that has a mode.
///
/// Disabled outputs are included so they remain visible (and can be
/// re-enabled with a right click), just like hyprmon.
pub fn compute_layout(
    outputs: &[Output],
    width: i64,
    height: i64,
    padding: i64,
    cell_aspect: f64,
) -> Option<LayoutGeometry> {
    let drawable: Vec<&Output> = outputs
        .iter()
        .filter(|output| output.logical_width() > 0 && output.logical_height() > 0)
        .collect();
    if drawable.is_empty() || width <= 0 || height <= 0 {
        return None;
    }

    let min_x = drawable.iter().map(|output| output.x).min()?;
    let min_y = drawable.iter().map(|output| output.y).min()?;
    let max_x = drawable
        .iter()
        .map(|output| output.x + output.logical_width())
        .max()?;
    let max_y = drawable
        .iter()
        .map(|output| output.y + output.logical_height())
        .max()?;
    let span_x = (max_x - min_x).max(1) as f64;
    let span_y = (max_y - min_y).max(1) as f64;

    let available_w = (width - 2 * padding).max(1) as f64;
    let available_h = (height - 2 * padding).max(1) as f64;
    let scale = (available_w / (span_x * cell_aspect)).min(available_h / span_y);
    if scale.is_nan() || scale <= 0.0 || !scale.is_finite() {
        return None;
    }

    let drawn_w = span_x * cell_aspect * scale;
    let drawn_h = span_y * scale;
    let offset_x = padding as f64 + ((available_w - drawn_w) / 2.0).max(0.0);
    let offset_y = padding as f64 + ((available_h - drawn_h) / 2.0).max(0.0);

    let boxes = drawable
        .iter()
        .map(|output| {
            let rel_x = (output.x - min_x) as f64;
            let rel_y = (output.y - min_y) as f64;
            let cell = |value: f64| value.round_ties_even() as i64;
            let left = cell(offset_x + rel_x * cell_aspect * scale);
            let top = cell(offset_y + rel_y * scale);
            let right =
                cell(offset_x + (rel_x + output.logical_width() as f64) * cell_aspect * scale);
            let bottom = cell(offset_y + (rel_y + output.logical_height() as f64) * scale);
            let left = left.clamp(0, width - 1);
            let top = top.clamp(0, height - 1);
            let right = right.min(width).max(left + 1);
            let bottom = bottom.min(height).max(top + 1);
            (
                output.name.clone(),
                Rect {
                    x: left,
                    y: top,
                    width: right - left,
                    height: bottom - top,
                },
            )
        })
        .collect();

    Some(LayoutGeometry {
        scale,
        min_x,
        min_y,
        offset_x,
        offset_y,
        boxes,
        cell_aspect,
    })
}

fn add_guide(guides: &mut Vec<Guide>, orientation: Orientation, value: i64) {
    let guide = Guide { orientation, value };
    if !guides.contains(&guide) {
        guides.push(guide);
    }
}

/// Snap a proposed position for `outputs[index]` and return the guides that
/// should be shown.
///
/// The algorithm follows hyprmon: edges first, then centers, then the origin.
pub fn snap_position(
    outputs: &[Output],
    index: usize,
    x: i64,
    y: i64,
    mode: SnapMode,
    threshold: i64,
) -> (i64, i64, Vec<Guide>) {
    use Orientation::{Horizontal, Vertical};

    if mode == SnapMode::Off || threshold <= 0 {
        return (x, y, Vec::new());
    }
    let Some(output) = outputs.get(index) else {
        return (x, y, Vec::new());
    };

    let mut guides = Vec::new();
    let (mut new_x, mut new_y) = (x, y);
    let width = output.logical_width();
    let height = output.logical_height();
    let near = |a: i64, b: i64| (a - b).abs() < threshold;

    for (other_index, other) in outputs.iter().enumerate() {
        if other_index == index || !other.enabled {
            continue;
        }
        let other_width = other.logical_width();
        let other_height = other.logical_height();

        if mode.edges() {
            if near(x, other.x + other_width) {
                new_x = other.x + other_width;
                add_guide(&mut guides, Vertical, new_x);
            } else if near(x + width, other.x) {
                new_x = other.x - width;
                add_guide(&mut guides, Vertical, other.x);
            } else if near(x, other.x) {
                new_x = other.x;
                add_guide(&mut guides, Vertical, new_x);
            }

            if near(y, other.y + other_height) {
                new_y = other.y + other_height;
                add_guide(&mut guides, Horizontal, new_y);
            } else if near(y + height, other.y) {
                new_y = other.y - height;
                add_guide(&mut guides, Horizontal, other.y);
            } else if near(y, other.y) {
                new_y = other.y;
                add_guide(&mut guides, Horizontal, new_y);
            }
        }

        if mode.centers() {
            let center_x = x + width.div_euclid(2);
            let center_y = y + height.div_euclid(2);
            let other_center_x = other.x + other_width.div_euclid(2);
            let other_center_y = other.y + other_height.div_euclid(2);
            if near(center_x, other_center_x) {
                new_x = other_center_x - width.div_euclid(2);
                add_guide(&mut guides, Vertical, other_center_x);
            }
            if near(center_y, other_center_y) {
                new_y = other_center_y - height.div_euclid(2);
                add_guide(&mut guides, Horizontal, other_center_y);
            }
        }
    }

    // The origin acts as an additional snap target in every snapping mode.
    if x.abs() < threshold {
        new_x = 0;
        add_guide(&mut guides, Vertical, 0);
    }
    if y.abs() < threshold {
        new_y = 0;
        add_guide(&mut guides, Horizontal, 0);
    }

    (new_x, new_y, guides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::load_outputs;

    use crate::model::Mode;
    use Orientation::{Horizontal, Vertical};

    fn layout(outputs: &[Output], width: i64, height: i64) -> LayoutGeometry {
        compute_layout(outputs, width, height, 1, CELL_ASPECT).unwrap()
    }

    fn guide(orientation: Orientation, value: i64) -> Guide {
        Guide { orientation, value }
    }

    /// The fixture outputs with the second one as a 1000x1000 square.
    fn square_pair() -> Vec<Output> {
        let mut outputs = load_outputs();
        outputs[0].x = 0;
        outputs[0].y = 0;
        outputs[1].mode = Some(Mode::new(1000, 1000, 60.0));
        outputs[1].scale = 1.0;
        outputs
    }

    #[test]
    fn single_output_map_is_wide() {
        let output = load_outputs().remove(0);
        let geometry = layout(std::slice::from_ref(&output), 100, 30);
        let rect = geometry.get(&output.name).unwrap();
        assert!(rect.width > rect.height);
        assert!(rect.x >= 0 && rect.right() <= 100);
        assert!(rect.y >= 0 && rect.bottom() <= 30);
    }

    #[test]
    fn side_by_side_outputs_do_not_overlap() {
        let geometry = layout(&load_outputs(), 120, 36);
        let first = geometry.get("HDMI-A-1").unwrap();
        let second = geometry.get("eDP-1").unwrap();
        assert!(first.right() <= second.x + 1);
        assert_eq!(first.y, second.y);
    }

    #[test]
    fn disabled_outputs_stay_on_map() {
        let mut output = load_outputs().remove(0);
        output.enabled = false;
        let geometry = layout(std::slice::from_ref(&output), 100, 30);
        assert!(geometry.get(&output.name).is_some());
    }

    #[test]
    fn outputs_without_a_mode_or_space_have_no_layout() {
        let mut output = load_outputs().remove(0);
        assert!(compute_layout(std::slice::from_ref(&output), 0, 30, 1, CELL_ASPECT).is_none());
        output.mode = None;
        assert!(compute_layout(&[output], 100, 30, 1, CELL_ASPECT).is_none());
    }

    #[test]
    fn world_to_cell_matches_layout_origin() {
        let output = load_outputs().remove(1);
        let geometry = layout(std::slice::from_ref(&output), 100, 30);
        let (column, row) = geometry.world_to_cell(output.x, output.y);
        assert!(column >= 0 && row >= 0);
    }

    #[test]
    fn snap_to_left_edge_and_origin() {
        let outputs = load_outputs();
        let (x, y, guides) = snap_position(&outputs, 1, 1915, 4, SnapMode::Both, 8);
        assert_eq!((x, y), (1920, 0));
        assert!(guides.contains(&guide(Vertical, 1920)));
        assert!(guides.contains(&guide(Horizontal, 0)));
    }

    #[test]
    fn snap_to_centered() {
        let mut outputs = square_pair();
        outputs[1].x = 460;
        outputs[1].y = 1080;
        let (x, y, guides) = snap_position(&outputs, 1, 458, 1084, SnapMode::Both, 8);
        assert_eq!((x, y), (460, 1080));
        assert!(guides.contains(&guide(Vertical, 960)));
    }

    #[test]
    fn snap_leaves_far_positions_alone() {
        let outputs = load_outputs();
        let (x, y, guides) = snap_position(&outputs, 1, 3000, 4000, SnapMode::Both, 8);
        assert_eq!((x, y), (3000, 4000));
        assert!(guides.is_empty());
    }

    #[test]
    fn snap_off_never_moves_or_draws_guides() {
        let outputs = load_outputs();
        let (x, y, guides) = snap_position(&outputs, 1, 1915, 4, SnapMode::Off, 8);
        assert_eq!((x, y), (1915, 4));
        assert!(guides.is_empty());
    }

    #[test]
    fn snap_centers_only() {
        // Their centers are 2px apart, but no edges are within the threshold.
        let outputs = square_pair();
        let (x, y, guides) = snap_position(&outputs, 1, 458, 2000, SnapMode::Centers, 8);
        assert_eq!((x, y), (460, 2000));
        assert!(guides.contains(&guide(Vertical, 960)));
    }

    #[test]
    fn snap_both_combines_edge_and_center_guides() {
        let outputs = square_pair();
        let (x, y, guides) = snap_position(&outputs, 1, 458, 1076, SnapMode::Both, 8);
        assert_eq!((x, y), (460, 1080));
        assert!(guides.contains(&guide(Vertical, 960)));
        assert!(guides.contains(&guide(Horizontal, 1080)));
    }

    #[test]
    fn disabled_outputs_are_not_snap_targets() {
        let mut outputs = load_outputs();
        outputs[0].enabled = false;
        let (x, y, guides) = snap_position(&outputs, 1, 5, 5, SnapMode::Both, 10);
        // Only the origin remains as a snap target.
        assert_eq!((x, y), (0, 0));
        assert!(guides.contains(&guide(Vertical, 0)));
        assert!(guides.contains(&guide(Horizontal, 0)));
    }

    #[test]
    fn snap_mode_labels_and_cycling() {
        assert_eq!(SnapMode::Off.label(), "Off");
        assert_eq!(SnapMode::Edges.label(), "Edges");
        assert_eq!(SnapMode::Centers.label(), "Centers");
        assert_eq!(SnapMode::Both.label(), "Both");
        assert_eq!(SnapMode::Off.cycled(), SnapMode::Edges);
        assert_eq!(SnapMode::Both.cycled(), SnapMode::Off);
        assert_eq!(SnapMode::from_label("centers"), SnapMode::Centers);
        assert_eq!(SnapMode::from_label("nonsense"), SnapMode::Edges);
    }
}
