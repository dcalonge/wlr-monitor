use std::collections::HashMap;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::compositor::Kind;
use crate::model::tests::load_outputs;
use crate::testutil::TempDir;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn chars(edit: &mut LineEdit, text: &str) {
    for c in text.chars() {
        assert!(edit.handle_key(&key(KeyCode::Char(c)), &mut Clipboard::default()));
    }
}

/// Press keys on a line edit with a throwaway clipboard.
fn edit_keys(edit: &mut LineEdit, keys: &[KeyEvent]) {
    let mut clipboard = Clipboard::default();
    for key in keys {
        edit.handle_key(key, &mut clipboard);
    }
}

fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

#[test]
fn line_edit_supports_the_usual_keys() {
    let mut edit = LineEdit::new("work");
    assert_eq!(edit.cursor, 4);
    chars(&mut edit, " desk");
    assert_eq!(edit.value, "work desk");
    edit_keys(&mut edit, &[key(KeyCode::Home)]);
    chars(&mut edit, "my ");
    assert_eq!(edit.value, "my work desk");
    edit_keys(&mut edit, &[ctrl('e'), ctrl('w')]);
    assert_eq!(edit.value, "my work ");
    edit_keys(
        &mut edit,
        &[
            key(KeyCode::Backspace),
            key(KeyCode::Left),
            key(KeyCode::Delete),
        ],
    );
    assert_eq!(edit.value, "my wor");
    edit_keys(&mut edit, &[ctrl('a'), key(KeyCode::Right), ctrl('k')]);
    assert_eq!(edit.value, "m");
    edit_keys(&mut edit, &[ctrl('u')]);
    assert_eq!((edit.value.as_str(), edit.cursor), ("", 0));
    edit.insert("café\tbar");
    assert_eq!(edit.value, "cafébar");
    edit_keys(
        &mut edit,
        &[
            key(KeyCode::Left),
            key(KeyCode::Left),
            key(KeyCode::Left),
            key(KeyCode::Backspace),
        ],
    );
    assert_eq!(edit.value, "cafbar");
    assert!(!edit.handle_key(&key(KeyCode::Tab), &mut Clipboard::default()));
}

#[test]
fn prompt_trims_and_treats_blank_as_cancel() {
    let mut clipboard = Clipboard::default();
    let mut prompt = PromptDialog::new("Save", "name", "  ");
    assert_eq!(
        prompt.handle_key(&key(KeyCode::Enter), &mut clipboard),
        Response::Close(None)
    );
    let mut prompt = PromptDialog::new("Save", "name", " work ");
    assert_eq!(
        prompt.handle_key(&key(KeyCode::Enter), &mut clipboard),
        Response::Close(Some("work".into()))
    );
    assert_eq!(
        prompt.handle_key(&key(KeyCode::Esc), &mut clipboard),
        Response::Close(None)
    );
    prompt.paste("a\nb");
    assert_eq!(prompt.input.value, " work a");
    prompt.paste("");
    assert_eq!(prompt.input.value, " work a");
}

#[test]
fn line_edit_words_follow_textual() {
    // Word starts are `(?<=\W)\w`, so paths edit one component at a time.
    let mut edit = LineEdit::new("~/icc/display.icc");
    edit_keys(&mut edit, &[ctrl('w')]);
    assert_eq!(edit.value, "~/icc/display.");
    edit_keys(&mut edit, &[with(KeyCode::Left, KeyModifiers::CONTROL)]);
    assert_eq!(edit.cursor, 6);
    edit_keys(&mut edit, &[with(KeyCode::Left, KeyModifiers::CONTROL)]);
    assert_eq!(edit.cursor, 2);
    edit_keys(&mut edit, &[with(KeyCode::Left, KeyModifiers::CONTROL)]);
    assert_eq!(edit.cursor, 0);
    edit_keys(&mut edit, &[with(KeyCode::Right, KeyModifiers::CONTROL)]);
    assert_eq!(edit.cursor, 2);
    edit_keys(&mut edit, &[with(KeyCode::Right, KeyModifiers::CONTROL)]);
    assert_eq!(edit.cursor, 6);
    edit_keys(&mut edit, &[with(KeyCode::Right, KeyModifiers::CONTROL)]);
    assert_eq!(edit.cursor, 14);

    // Ctrl+Backspace and Alt+Backspace delete to the next word start on
    // the right; with none left, everything to the right goes.
    let mut edit = LineEdit::new("one two three");
    edit_keys(&mut edit, &[key(KeyCode::Home)]);
    edit_keys(
        &mut edit,
        &[with(KeyCode::Backspace, KeyModifiers::CONTROL)],
    );
    assert_eq!(edit.value, "two three");
    edit_keys(&mut edit, &[with(KeyCode::Backspace, KeyModifiers::ALT)]);
    assert_eq!(edit.value, "three");
    edit_keys(
        &mut edit,
        &[with(KeyCode::Backspace, KeyModifiers::CONTROL)],
    );
    assert_eq!(edit.value, "");

    // Keys Textual's Input does not bind do nothing.
    let mut edit = LineEdit::new("one two");
    edit_keys(&mut edit, &[key(KeyCode::Home)]);
    for unbound in [
        ctrl('f'),
        with(KeyCode::Char('d'), KeyModifiers::ALT),
        with(KeyCode::Delete, KeyModifiers::CONTROL),
        with(KeyCode::Left, KeyModifiers::ALT),
    ] {
        assert!(!edit.handle_key(&unbound, &mut Clipboard::default()));
    }
    assert_eq!((edit.value.as_str(), edit.cursor), ("one two", 0));
}

#[test]
fn line_edit_selects_cuts_copies_and_pastes() {
    let mut clipboard = Clipboard::default();
    let mut edit = LineEdit::new("~/icc/display.icc");
    let shift_left = with(KeyCode::Left, KeyModifiers::SHIFT);
    for _ in 0..3 {
        edit.handle_key(&shift_left, &mut clipboard);
    }
    assert_eq!(edit.selected_text(), "icc");
    assert!(edit.handle_key(&ctrl('c'), &mut clipboard));
    assert_eq!(clipboard.text, "icc");
    assert_eq!(clipboard.pending.take().as_deref(), Some("icc"));
    // Typing replaces the selection.
    edit.handle_key(&key(KeyCode::Char('x')), &mut clipboard);
    assert_eq!(edit.value, "~/icc/display.x");
    assert!(!edit.has_selection());
    // Nothing selected: Ctrl+C is not used (and reaches nothing else).
    assert!(!edit.handle_key(&ctrl('c'), &mut clipboard));
    // An empty cut keeps the clipboard.
    edit.handle_key(&ctrl('x'), &mut clipboard);
    assert_eq!(clipboard.text, "icc");

    // Left/Right collapse a selection to its start/end.
    edit.handle_key(&shift_left, &mut clipboard);
    edit.handle_key(&shift_left, &mut clipboard);
    edit.handle_key(&key(KeyCode::Left), &mut clipboard);
    assert_eq!((edit.cursor, edit.has_selection()), (13, false));
    edit.handle_key(&with(KeyCode::End, KeyModifiers::SHIFT), &mut clipboard);
    assert_eq!(edit.selected_text(), ".x");
    edit.handle_key(&key(KeyCode::Right), &mut clipboard);
    assert_eq!((edit.cursor, edit.has_selection()), (15, false));

    // Select all, cut, paste twice.
    edit.handle_key(
        &with(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
        &mut clipboard,
    );
    edit.handle_key(&ctrl('x'), &mut clipboard);
    assert_eq!(
        (edit.value.as_str(), clipboard.text.as_str()),
        ("", "~/icc/display.x")
    );
    edit.handle_key(&ctrl('v'), &mut clipboard);
    edit.handle_key(&ctrl('v'), &mut clipboard);
    assert_eq!(edit.value, "~/icc/display.x~/icc/display.x");

    // Shift+Home selects from the cursor to the start; Backspace deletes it.
    let mut edit = LineEdit::new("one two");
    edit.handle_key(&key(KeyCode::Left), &mut clipboard);
    edit.handle_key(&with(KeyCode::Home, KeyModifiers::SHIFT), &mut clipboard);
    assert_eq!(edit.selected_text(), "one tw");
    edit.handle_key(&key(KeyCode::Backspace), &mut clipboard);
    assert_eq!((edit.value.as_str(), edit.cursor), ("o", 0));
}

#[test]
fn prompt_input_is_padded_and_the_cursor_covers_the_placeholder() {
    let mut prompt = PromptDialog::new("Scale", "1.25", "");
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| prompt.render(frame, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let area = prompt.input.area;
    // Two cells of padding inside the input's border.
    assert_eq!(buffer[(area.x - 3, area.y)].symbol(), "┃");
    assert_eq!(buffer[(area.x - 1, area.y)].symbol(), " ");
    assert_eq!(buffer[(area.x, area.y)].symbol(), "1");
    assert_eq!(buffer[(area.x, area.y)].bg, Color::Gray);
    assert_eq!(buffer[(area.x + 1, area.y)].symbol(), ".");
    assert_ne!(buffer[(area.x + 1, area.y)].bg, Color::Gray);

    // Clicking moves the cursor; dragging selects.
    let mut prompt = PromptDialog::new("Scale", "1.25", "abcdef");
    terminal
        .draw(|frame| prompt.render(frame, frame.area()))
        .unwrap();
    let area = prompt.input.area;
    let mouse = |kind, column| MouseEvent {
        kind,
        column,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    prompt.handle_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1));
    assert_eq!(prompt.input.cursor, 1);
    prompt.handle_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), area.x + 4));
    prompt.handle_mouse(&mouse(MouseEventKind::Up(MouseButton::Left), area.x + 4));
    assert_eq!(prompt.input.selected_text(), "bcd");
}

#[test]
fn option_list_navigation_and_mouse() {
    let options: Vec<(String, String)> = (0..30)
        .map(|i| (i.to_string(), format!("item {i}")))
        .collect();
    let mut dialog = ChoiceDialog::new("Pick", options, Some(3));
    assert_eq!(dialog.list.highlighted_id(), Some("3"));
    dialog.handle_key(&key(KeyCode::Up));
    // Choice dialogs have no j/k (only the profile browser does).
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('k'))),
        Response::Ignored
    );
    assert_eq!(dialog.list.highlighted_id(), Some("2"));
    dialog.handle_key(&key(KeyCode::Home));
    dialog.handle_key(&key(KeyCode::Up));
    assert_eq!(dialog.list.highlighted_id(), Some("0"));
    dialog.handle_key(&key(KeyCode::End));
    dialog.handle_key(&key(KeyCode::Down));
    assert_eq!(dialog.list.highlighted_id(), Some("29"));

    // Render so the list knows where it is, then scroll and click.
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();
    let area = dialog.list.area;
    assert!(area.height > 0);
    dialog.handle_key(&key(KeyCode::PageUp));
    assert_eq!(dialog.list.highlighted, Some(29 - usize::from(area.height)));
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();

    // The wheel scrolls two rows and leaves the highlight where it is.
    let highlighted = dialog.list.highlighted;
    let offset = dialog.list.offset;
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: area.x + 2,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    for _ in 0..3 {
        dialog.handle_mouse(&wheel);
        terminal
            .draw(|frame| dialog.render(frame, frame.area()))
            .unwrap();
    }
    assert_eq!(dialog.list.highlighted, highlighted);
    assert_eq!(dialog.list.offset, offset - 6);

    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        ..wheel
    };
    let first_visible = dialog.list.offset;
    assert_eq!(
        dialog.handle_mouse(&click),
        Response::Close(Some(first_visible.to_string()))
    );
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('q'))),
        Response::Close(None)
    );
    assert_eq!(
        dialog.handle_key(&key(KeyCode::Enter)),
        Response::Close(Some(first_visible.to_string()))
    );
}

#[test]
fn empty_lists_have_no_highlight() {
    let mut list = OptionList::new(Vec::new(), Some(2));
    assert_eq!(list.highlighted, None);
    assert_eq!(list.handle_key(&key(KeyCode::Enter)), Some(false));
}

#[test]
fn confirm_keys() {
    let mut dialog = ConfirmDialog::new("Sure?", "Really?");
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('Y'))),
        Response::Close(true)
    );
    assert_eq!(
        dialog.handle_key(&key(KeyCode::Enter)),
        Response::Close(true)
    );
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('n'))),
        Response::Close(false)
    );
    assert_eq!(
        dialog.handle_key(&key(KeyCode::Esc)),
        Response::Close(false)
    );
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('x'))),
        Response::Ignored
    );
}

#[test]
fn advanced_cycles_color_settings_and_keeps_custom_profiles() {
    let mut output = load_outputs().remove(0);
    output.color_profile = Some("icc:/x.icc".into());
    let caps = Kind::Sway.default_capabilities();
    let mut dialog = AdvancedDialog::new(output, caps, "sway", true);
    let mut notes = Notifications::default();
    dialog.focus = dialog
        .fields
        .iter()
        .position(|f| *f == Field::Color)
        .unwrap();
    let mut seen = Vec::new();
    for _ in 0..4 {
        dialog.handle_key(&key(KeyCode::Right), &mut notes);
        seen.push(dialog.output.color_profile.clone());
    }
    assert_eq!(
        seen,
        [
            None,
            Some("gamma22".into()),
            Some("srgb".into()),
            Some("icc:/x.icc".into())
        ]
    );
    dialog.focus = dialog
        .fields
        .iter()
        .position(|f| *f == Field::BitDepth)
        .unwrap();
    dialog.handle_key(&key(KeyCode::Left), &mut notes);
    assert_eq!(dialog.output.bit_depth, Some(10));
    dialog.focus = dialog.fields.iter().position(|f| *f == Field::Hdr).unwrap();
    dialog.handle_key(&KeyEvent::from(KeyCode::Char(' ')), &mut notes);
    dialog.handle_key(&KeyEvent::from(KeyCode::Char(' ')), &mut notes);
    assert_eq!(dialog.output.hdr, Some(false));
    // Tab / Shift+Tab move between fields and wrap around.
    dialog.focus = 0;
    dialog.handle_key(&key(KeyCode::BackTab), &mut notes);
    assert_eq!(dialog.fields[dialog.focus], Field::Color);
    dialog.handle_key(&key(KeyCode::Tab), &mut notes);
    assert_eq!(dialog.focus, 0);
}

#[test]
fn advanced_space_on_icc_only_color_opens_the_picker() {
    let output = load_outputs().remove(0);
    let caps = Kind::Mango.default_capabilities();
    let mut dialog = AdvancedDialog::new(output, caps, "mango", true);
    assert_eq!(
        dialog.fields,
        [
            Field::Transform,
            Field::Vrr,
            Field::Enabled,
            Field::Hdr,
            Field::Color
        ]
    );
    dialog.focus = 4;
    let mut notes = Notifications::default();
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char(' ')), &mut notes),
        AdvancedResponse::PickIcc(String::new())
    );
    let mut generic = AdvancedDialog::new(
        load_outputs().remove(0),
        Capabilities::default(),
        "wlroots",
        true,
    );
    assert_eq!(
        generic.handle_key(&KeyEvent::from(KeyCode::Char('i')), &mut notes),
        AdvancedResponse::Handled
    );
    assert_eq!(
        generic.handle_key(&KeyEvent::from(KeyCode::Char('q')), &mut notes),
        AdvancedResponse::Ignored
    );
}

#[test]
fn icc_paths_are_checked() {
    let dir = TempDir::new();
    let icc = dir.join("display.icc");
    std::fs::write(&icc, "").unwrap();
    let mut dialog = AdvancedDialog::new(
        load_outputs().remove(0),
        Kind::Mango.default_capabilities(),
        "mango",
        true,
    );
    let mut notes = Notifications::default();
    let mango = crate::compositor::Compositor::new(Kind::Mango, Some(HashMap::new()), None, true);
    let ok = |value: &str| mango.color_profile_supported(value);
    dialog.finish_icc(Some("/a,b.icc".into()), dir.path(), &ok, &mut notes);
    assert_eq!(dialog.output.color_profile, None);
    assert!(notes.messages()[0].contains("mango cannot use that ICC path"));
    dialog.finish_icc(Some(icc.display().to_string()), dir.path(), &ok, &mut notes);
    assert_eq!(
        dialog.output.color_profile,
        Some(format!("icc:{}", icc.display()))
    );
    assert_eq!(
        notes.messages().len(),
        1,
        "an existing file gives no warning"
    );
    dialog.finish_icc(None, dir.path(), &ok, &mut notes);
    assert!(dialog.output.color_profile.is_some());
}

#[test]
fn profiles_dialog_lists_marks_and_confirms() {
    let dir = TempDir::new();
    let store = ProfileStore::new(Some(dir.join("profiles.json")));
    let states: Vec<OutputState> = load_outputs().iter().map(Output::state).collect();
    store.save("work", &states).unwrap();
    store.save("solo", &states[..1]).unwrap();
    let mut notes = Notifications::default();
    let mut dialog = ProfilesDialog::new(&store, states.clone(), Some("solo"), false, true);
    assert_eq!(
        dialog.list.labels(),
        [
            "work *  (2 outputs)",
            "solo  (1 output)",
            "Save current layout…"
        ]
    );
    assert_eq!(dialog.list.highlighted_id(), Some("solo"));

    // Deleting asks first; other keys are ignored meanwhile.
    dialog.handle_key(&KeyEvent::from(KeyCode::Char('d')), &store, &mut notes);
    assert_eq!(dialog.confirm_delete.as_deref(), Some("solo"));
    assert_eq!(
        dialog.handle_key(&key(KeyCode::Enter), &store, &mut notes),
        ProfilesResponse::Handled
    );
    dialog.handle_key(&KeyEvent::from(KeyCode::Char('n')), &store, &mut notes);
    assert_eq!(dialog.confirm_delete, None);
    assert_eq!(
        dialog.list.highlighted_id(),
        Some("solo"),
        "cancelling keeps the place"
    );
    assert_eq!(store.names().unwrap(), ["work", "solo"]);

    // The special entries cannot be renamed or deleted.
    dialog.handle_key(&key(KeyCode::End), &store, &mut notes);
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('r')), &store, &mut notes),
        ProfilesResponse::Handled
    );
    dialog.handle_key(&KeyEvent::from(KeyCode::Char('d')), &store, &mut notes);
    assert_eq!(dialog.confirm_delete, None);
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('s')), &store, &mut notes),
        ProfilesResponse::Close(Some(SAVE_ID.into()))
    );
    // `l` only launches the editor from the standalone browser.
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('l')), &store, &mut notes),
        ProfilesResponse::Handled
    );

    // Renaming keeps the highlight on the renamed profile.
    dialog.list.highlight(1);
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('r')), &store, &mut notes),
        ProfilesResponse::Rename("solo".into())
    );
    dialog.finish_rename("solo", Some("single".into()), &store);
    assert_eq!(dialog.list.highlighted_id(), Some("single"));
    dialog.finish_rename("single", Some("work".into()), &store);
    assert!(dialog.messages()[0].contains("already exists"));
}

#[test]
fn standalone_profiles_dialog_offers_the_editor() {
    let dir = TempDir::new();
    let store = ProfileStore::new(Some(dir.join("profiles.json")));
    let mut notes = Notifications::default();
    let mut dialog = ProfilesDialog::new(&store, Vec::new(), None, true, false);
    assert_eq!(dialog.list.labels(), ["[ Open monitor editor ]"]);
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('s')), &store, &mut notes),
        ProfilesResponse::Handled
    );
    assert_eq!(
        dialog.handle_key(&KeyEvent::from(KeyCode::Char('l')), &store, &mut notes),
        ProfilesResponse::Close(Some(LAUNCH_ID.into()))
    );
    let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Apply & exit"));
}

#[test]
fn notifications_queue_and_expire() {
    let mut notes = Notifications::default();
    notes.notify("one", "Info", Severity::Information, 4);
    notes.notify("two", "Warning", Severity::Warning, 8);
    assert_eq!(notes.messages(), ["one", "two"]);
    assert!(notes.next_expiry().is_some());
    assert!(notes.expire(Instant::now() + Duration::from_secs(5)));
    assert_eq!(notes.messages(), ["two"]);
    assert!(!notes.expire(Instant::now()));
}
