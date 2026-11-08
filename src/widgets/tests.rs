use super::*;
use crate::layout::Orientation;
use crate::model::tests::load_outputs;

fn keys(lines: &[Vec<HelpItem>]) -> Vec<Vec<&'static str>> {
    lines
        .iter()
        .map(|line| line.iter().map(|item| item.key).collect())
        .collect()
}

#[test]
fn help_bar_wraps_and_drops_low_priority_items() {
    let items = [
        help("a", "Apply", 0),
        help("z", "Reset", 6),
        help("q", "Quit", 1),
    ];
    assert_eq!(keys(&layout_help(&items, 80, 2)), [vec!["a", "z", "q"]]);
    assert_eq!(keys(&layout_help(&items, 10, 2)), [vec!["a"], vec!["q"]]);
}

#[test]
fn ties_drop_the_first_low_priority_item() {
    let items = [
        help("a", "One", 5),
        help("b", "Two", 5),
        help("c", "Three", 0),
    ];
    // Only two fit: the first of the two priority-5 items goes, like Python's max().
    assert_eq!(keys(&layout_help(&items, 7, 2)), [vec!["b"], vec!["c"]]);
}

#[test]
fn text_helpers() {
    assert_eq!(center("ab", 5), "  ab ");
    assert_eq!(center("abc", 6), " abc  ");
    assert_eq!(center("abc", 4), "abc ");
    assert_eq!(center("ab", 3), " ab");
    assert_eq!(center("a", 4), " a  ");
    assert_eq!(center("toolong", 3), "toolong");
    assert_eq!(ellipsize("HDMI-A-1", 5), "HDMI…");
    assert_eq!(ellipsize("HDMI", 5), "HDMI");
    let fitted = fit_spans(
        vec![Span::raw("Rotation       "), Span::raw("flipped-270")],
        20,
    );
    assert_eq!(plain_text(&[Line::from(fitted)]), "Rotation       flip…");
    let padded = fit_spans(vec![Span::raw("ab")], 4);
    assert_eq!(plain_text(&[Line::from(padded)]), "ab  ");
}

#[test]
fn wrap_line_breaks_on_words() {
    let lines = wrap_line(Line::from("Pending  position 0,0 → 32,0 · scale 1 → 2"), 20);
    let text: Vec<String> = lines
        .iter()
        .map(|l| plain_text(std::slice::from_ref(l)))
        .collect();
    assert_eq!(text, ["Pending  position", "0,0 → 32,0 · scale 1", "→ 2"]);
    assert!(text.iter().all(|line| text_width(line) <= 20));
    let folded = wrap_line(Line::from("abcdefghij"), 4);
    assert_eq!(folded.len(), 3);
}

#[test]
fn table_drops_columns_to_fit() {
    let all: Vec<&str> = visible_columns(200).iter().map(|c| c.header).collect();
    assert_eq!(
        all,
        [
            "Name", "Mode", "State", "Scale", "Position", "Rotation", "VRR"
        ]
    );
    let narrow: Vec<&str> = visible_columns(40).iter().map(|c| c.header).collect();
    assert_eq!(narrow, ["Name", "Mode", "State"]);
    let medium: Vec<&str> = visible_columns(42).iter().map(|c| c.header).collect();
    assert_eq!(medium, ["Name", "Mode", "State", "Scale"]);
    assert_eq!(visible_columns(5).len(), 1);
}

#[test]
fn table_marks_selected_and_dirty_rows() {
    let outputs = load_outputs();
    let dirty = HashSet::from(["eDP-1".to_string()]);
    let lines = table_lines(&outputs, 0, &dirty, 100);
    let text = plain_text(&lines);
    let rows: Vec<&str> = text.lines().collect();
    assert!(rows[0].contains("Name") && rows[0].contains("Rotation"));
    assert_eq!(rows[1], "");
    assert!(rows[2].starts_with("▸ ") && rows[2].contains("1920x1080@100"));
    assert!(rows[3].starts_with("● ") && rows[3].contains("1920,0"));
    assert_eq!(text_width(rows[2]), 100);
    assert_eq!(lines[2].spans[0].style.bg, Some(Color::DarkGray));
    assert_eq!(
        plain_text(&table_lines(&[], 0, &dirty, 30)).trim(),
        "No outputs found"
    );
}

#[test]
fn map_draws_outputs_guides_and_disabled_state() {
    let mut outputs = load_outputs();
    let map = render_map(&outputs, Some("HDMI-A-1"), &HashSet::new(), &[], 100, 20);
    let text = plain_text(&map.lines);
    assert!(text.contains("HDMI-A-1 [ON]") && text.contains("eDP-1 [ON]"));
    assert!(text.contains("1920x1080@100Hz"));
    assert!(text.contains("VRR x1.2"));
    assert!(text.contains('┏'), "the selected output is drawn heavy");
    assert_eq!(map.draw_order.last().map(String::as_str), Some("HDMI-A-1"));

    outputs[1].enabled = false;
    let guides = [Guide {
        orientation: Orientation::Vertical,
        value: 0,
    }];
    let map = render_map(
        &outputs,
        Some("HDMI-A-1"),
        &HashSet::new(),
        &guides,
        100,
        20,
    );
    let text = plain_text(&map.lines);
    assert!(text.contains("[OFF]"));
    assert!(text.contains('·'));
    assert!(text.contains('┊'));

    let rect = map.geometry.as_ref().unwrap().get("eDP-1").unwrap();
    assert_eq!(map.hit(rect.x + 1, rect.y + 1).as_deref(), Some("eDP-1"));
    assert_eq!(map.hit(-1, -1), None);

    let tiny = render_map(&outputs, None, &HashSet::new(), &[], 10, 2);
    assert_eq!(plain_text(&tiny.lines), "Too small for the layout map");
    outputs.iter_mut().for_each(|output| output.mode = None);
    let empty = render_map(&outputs, None, &HashSet::new(), &[], 50, 10);
    assert_eq!(plain_text(&empty.lines), "No outputs with a mode");
}

#[test]
fn map_lines_have_the_requested_size() {
    let mut outputs = load_outputs();
    outputs[0].name = "宽屏-1".into(); // wide characters keep the grid aligned
    let map = render_map(&outputs, Some("宽屏-1"), &HashSet::new(), &[], 80, 15);
    assert_eq!(map.lines.len(), 15);
    for line in &map.lines {
        assert_eq!(text_width(&plain_text(std::slice::from_ref(line))), 80);
    }
}

#[test]
fn details_show_rows_pending_changes_and_errors() {
    let outputs = load_outputs();
    let baseline = outputs[0].state();
    let mut changed = outputs[0].clone();
    changed.transform = "90".into();
    let caps = crate::compositor::Kind::Sway.default_capabilities();
    let view = DetailsView {
        output: Some(&changed),
        baseline: Some(&baseline),
        error: None,
        profile_name: Some("work"),
        capabilities: &caps,
        loading: false,
    };
    let (title, lines) = details(&view, 100);
    assert_eq!(title, "HDMI-A-1 · Microstep MSI MP275");
    let text = plain_text(&lines);
    assert!(
        text.contains("Rotation") && text.contains("rotation normal → 90"),
        "{text}"
    );
    assert!(text.contains("HDR            default"));
    assert!(text.contains("Profile        work (active)"));
    assert!(text.contains("Color profile  default"));

    let generic = Capabilities::default();
    let view = DetailsView {
        output: Some(&outputs[1]),
        baseline: Some(&outputs[1].state()),
        error: Some("wlr-randr failed: boom"),
        profile_name: None,
        capabilities: &generic,
        loading: false,
    };
    let text = plain_text(&details(&view, 70).1);
    assert!(text.starts_with("Error: wlr-randr failed: boom"));
    assert!(!text.contains("HDR"));
    assert!(text.ends_with("No pending changes"));

    let view = DetailsView {
        output: None,
        baseline: None,
        error: None,
        profile_name: None,
        capabilities: &generic,
        loading: false,
    };
    assert!(plain_text(&details(&view, 70).1).starts_with("No outputs detected."));
}

#[test]
fn pending_changes_describe_every_setting() {
    let output = load_outputs().remove(0);
    let old = output.state();
    let mut new = output.clone();
    new.enabled = false;
    new.mode = Some(new.modes[0].clone());
    new.x = 10;
    new.scale = 1.25;
    new.adaptive_sync = true;
    new.hdr = Some(true);
    new.bit_depth = Some(10);
    new.color_profile = Some("icc:/x.icc".into());
    assert_eq!(
        pending_changes(&new, Some(&old)),
        [
            "state on → off",
            "mode 1920x1080 @ 100 Hz → 1920x1080 @ 60 Hz",
            "position 0,0 → 10,0",
            "scale 1 → 1.25",
            "VRR off → on",
            "HDR default → on",
            "color depth default → 10 bit",
            "color profile default → ICC /x.icc",
        ]
    );
    assert!(pending_changes(&new, None).is_empty());
}

#[test]
fn only_ansi_colors_are_used() {
    let outputs = load_outputs();
    let dirty = HashSet::from(["eDP-1".to_string()]);
    let mut lines = table_lines(&outputs, 0, &dirty, 100);
    lines.extend(render_map(&outputs, Some("eDP-1"), &dirty, &[], 100, 20).lines);
    for style in styles_of(&lines) {
        for color in [style.fg, style.bg].into_iter().flatten() {
            assert!(
                !matches!(color, Color::Rgb(..) | Color::Indexed(_)),
                "{color:?} is not a palette color"
            );
        }
    }
}

#[test]
fn wrapped_text_keeps_its_line_breaks() {
    let text = |lines: &[Line]| plain_text(lines);
    // ratatui's own constructors drop `\n`; `text_line` keeps it.
    let lines = wrap_line(text_line("first line\nsecond line", theme::plain()), 40);
    assert_eq!(text(&lines), "first line\nsecond line");
    let lines = wrap_line(
        Line::from(vec![
            Span::styled("Error: ", theme::bold()),
            Span::raw("one two\nthree"),
        ]),
        9,
    );
    assert_eq!(text(&lines), "Error:\none two\nthree");
    assert_eq!(
        text(&wrap_line(text_line("a\n\nb\n", theme::plain()), 5)),
        "a\n\nb\n"
    );
}

#[test]
fn centering_rounds_like_rich() {
    // floor((width - len) / 2) on the left: ratatui would put "abc" at 4.
    let line = center_line(Line::raw("abc"), 10);
    assert_eq!(plain_text(&[line]), "   abc");
    let line = center_line(Line::raw("abcd"), 10);
    assert_eq!(plain_text(&[line]), "   abcd");
    let line = center_line(Line::raw("too wide"), 4);
    assert_eq!(plain_text(&[line]), "too wide");
}
