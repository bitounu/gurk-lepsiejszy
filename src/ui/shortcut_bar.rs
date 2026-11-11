use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::config::{ShortcutBarConfig, ShortcutBarTheme};
use crossterm::event::KeyCode;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) struct BarLayout {
    main: Vec<Line<'static>>,
    context: Vec<Line<'static>>,
}

impl BarLayout {
    pub(crate) fn new(app: &crate::app::App, width: u16) -> Self {
        Self {
            main: hint_lines(&items(app), width),
            context: hint_lines(&context_items(app), width),
        }
    }

    pub(crate) fn height(&self, available: u16) -> u16 {
        (self.main.len().max(1) + self.context.len()).min(usize::from(available)) as u16
    }

    pub(crate) fn draw(&self, frame: &mut Frame, area: Rect, config: &ShortcutBarConfig) {
        let main_height = self.main.len().max(1).min(usize::from(area.height));
        let context_height = self
            .context
            .len()
            .min(usize::from(area.height) - main_height);
        let lines: Vec<_> = self
            .context
            .iter()
            .take(context_height)
            .chain(self.main.iter().take(main_height))
            .cloned()
            .collect();
        draw_lines(frame, area, config, lines);
    }
}

pub(crate) fn height(app: &crate::app::App, area: Rect) -> u16 {
    if !app.shortcut_bar_visible || area.width == 0 || area.height <= 1 {
        return 0;
    }
    BarLayout::new(app, area.width).height(area.height - 1)
}

/// Use as many aligned columns as fit, measuring terminal cells rather than bytes.
fn hint_lines(shortcuts: &[(String, String)], width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if width == 0 || shortcuts.is_empty() {
        return Vec::new();
    }
    let sizes: Vec<_> = shortcuts
        .iter()
        .map(|(key, label)| key.width() + 1 + label.width())
        .collect();
    for columns in (1..=shortcuts.len()).rev() {
        let mut widths = vec![0; columns];
        for (index, size) in sizes.iter().enumerate() {
            widths[index % columns] = widths[index % columns].max(*size);
        }
        if widths.iter().sum::<usize>() + 2 * (columns - 1) > width {
            continue;
        }
        return shortcuts
            .chunks(columns)
            .map(|row| {
                let mut spans = Vec::new();
                for (column, (key, label)) in row.iter().enumerate() {
                    spans.push(Span::styled(
                        key.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::raw(format!(" {label}")));
                    if column + 1 < row.len() {
                        spans.push(Span::raw(
                            " ".repeat(widths[column] - key.width() - 1 - label.width() + 2),
                        ));
                    }
                }
                Line::from(spans)
            })
            .collect();
    }
    // A single hint can exceed a very narrow terminal. Split at grapheme boundaries.
    let mut lines = Vec::new();
    for (key, label) in shortcuts {
        let mut spans = Vec::new();
        let mut used = 0;
        for (text, bold) in [(key.clone(), true), (format!(" {label}"), false)] {
            for grapheme in text.graphemes(true) {
                let grapheme = if grapheme.width() > width {
                    "…"
                } else {
                    grapheme
                };
                if used + grapheme.width() > width {
                    lines.push(Line::from(std::mem::take(&mut spans)));
                    used = 0;
                }
                used += grapheme.width();
                let style = if bold {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                spans.push(Span::styled(grapheme.to_owned(), style));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn color(value: Option<&str>, fallback: Color) -> Color {
    value
        .and_then(|value| value.strip_prefix('#'))
        .filter(|value| value.len() == 6 && value.is_ascii())
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .map(|rgb| Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
        .unwrap_or(fallback)
}

#[cfg(test)]
fn draw(frame: &mut Frame, area: Rect, config: &ShortcutBarConfig, shortcuts: &[(String, String)]) {
    draw_lines(frame, area, config, hint_lines(shortcuts, area.width));
}

fn draw_lines(
    frame: &mut Frame,
    area: Rect,
    config: &ShortcutBarConfig,
    lines: Vec<Line<'static>>,
) {
    if area.is_empty() {
        return;
    }
    let (foreground, background) = match config.theme {
        ShortcutBarTheme::Contrast => (Color::Black, Color::Rgb(211, 211, 211)),
        ShortcutBarTheme::Dark => (Color::Rgb(224, 224, 224), Color::Rgb(48, 48, 48)),
    };
    let style = Style::default()
        .fg(color(config.foreground.as_deref(), foreground))
        .bg(color(config.background.as_deref(), background));
    frame.render_widget(Paragraph::new(lines).style(style), area);
}

fn function_number(key: &str) -> Option<u8> {
    use crossterm::event::{KeyCode, KeyEvent};
    let event: KeyEvent = key.parse::<crokey::KeyCombination>().ok()?.into();
    match event.code {
        KeyCode::F(number) if event.modifiers.is_empty() => Some(number),
        _ => None,
    }
}

fn sort_hints(hints: &mut [(String, String)]) {
    hints.sort_by_key(|(key, _)| function_number(key).map_or((1, 0), |number| (0, number)));
}

fn conversation_view(app: &crate::app::App) -> bool {
    !app.is_help()
        && app.spelling_dialog.is_none()
        && app.message_search.is_none()
        && app.text_selection.is_none()
        && app.mention_completion.is_none()
        && app.send_job.is_none()
        && app.image_viewer.is_none()
        && app.file_picker.is_none()
        && app.forward_dialog.is_none()
        && app.hidden_dialog.is_none()
        && app.alias_editor.is_none()
        && !app.select_channel.is_shown
}

fn fallback_key(app: &crate::app::App, key: KeyCode) -> bool {
    app.event_to_command(&key.into()).is_none()
}

fn context_items(app: &crate::app::App) -> Vec<(String, String)> {
    use crate::command::Command;
    if !conversation_view(app) {
        return Vec::new();
    }
    let Some(message) = app.selected_message() else {
        return Vec::new();
    };
    let mut commands = vec![(Command::SelectText, "copy text")];
    if !message.deleted {
        commands.push((Command::ForwardMessage, "forward"));
    }
    if !message.attachments.is_empty() {
        commands.push((Command::ViewImage, "preview"));
    }
    if message.from_id == app.user_id
        && !message.deleted
        && app.input.is_empty()
        && !app.is_editing()
    {
        commands.push((Command::EditMessage, "edit"));
    }
    let mut result: Vec<_> = commands
        .into_iter()
        .filter_map(|(cmd, label)| command_key(app, &cmd).map(|key| (key, label.into())))
        .collect();
    if fallback_key(app, KeyCode::Enter)
        && !app.is_editing()
        && app.pending_attachments.is_empty()
        && !app.is_multiline_input
    {
        result.insert(
            0,
            (
                "Enter".into(),
                if app.input.is_empty() {
                    "open link"
                } else {
                    "reply"
                }
                .into(),
            ),
        );
    }
    if fallback_key(app, KeyCode::Esc)
        && app.composer_error.is_none()
        && app.pending_attachments.is_empty()
        && !app.is_editing()
    {
        result.push(("Esc".into(), "unselect".into()));
    }
    sort_hints(&mut result);
    result
}

pub(crate) fn items(app: &crate::app::App) -> Vec<(String, String)> {
    use crate::command::{Command, MoveAmountVisual};
    let fixed = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(key, label)| (key.to_string(), label.to_string()))
            .collect::<Vec<_>>()
    };
    let mut result = if app.spelling_dialog.is_some() {
        fixed(&[
            ("Enter", "correct"),
            ("Up/Down", "suggestion"),
            ("i", "ignore"),
            ("Esc", "close"),
        ])
    } else if app.message_search.is_some() {
        fixed(&[
            ("Enter", "accept match"),
            ("Esc", "restore position"),
            ("Up/Down", "match"),
            ("Shift+F3/F3", "match"),
        ])
    } else if app.text_selection.is_some() {
        fixed(&[
            ("Space", "select"),
            ("Enter", "copy"),
            ("Esc", "cancel"),
            ("Arrows", "move"),
            ("PgUp/PgDown", "scroll"),
        ])
    } else if app.mention_completion.is_some() {
        fixed(&[
            ("Up/Down", "contact"),
            ("Tab/Enter", "complete @name"),
            ("Esc", "dismiss"),
        ])
    } else if app.send_job.is_some() {
        fixed(&[("Esc", "cancel preparation")])
    } else if app.image_viewer.is_some() {
        fixed(&[
            ("Alt+arrows", "pan"),
            ("Left/Right", "attachment"),
            ("+/-", "zoom"),
            ("Esc/q", "back"),
        ])
    } else if let Some(picker) = app.file_picker.as_ref() {
        if picker.is_path_entry() {
            fixed(&[
                ("Enter", "open path"),
                ("Esc", "back"),
                ("Left/Right", "cursor"),
            ])
        } else {
            fixed(&[
                ("Space", "mark"),
                ("Enter", "add/open"),
                ("Alt+P", "image"),
                ("Alt+O", "open"),
                ("Esc", "cancel"),
                ("Left/Right", "folders"),
                (".", "hidden"),
                ("t", "sort"),
                ("p/s", "pictures/shots"),
            ])
        }
    } else if app.forward_dialog.is_some() {
        fixed(&[
            ("Enter", "forward"),
            ("Up/Down", "recipient"),
            ("Esc", "cancel"),
        ])
    } else if app.hidden_dialog.is_some() {
        fixed(&[
            ("Enter", "restore"),
            ("Up/Down", "select"),
            ("Esc", "close"),
        ])
    } else if app.alias_editor.is_some() {
        fixed(&[("Enter", "save"), ("Esc", "cancel")])
    } else if app.select_channel.is_shown {
        fixed(&[("Enter", "select"), ("Up/Down", "choose"), ("Esc", "close")])
    } else {
        let mut entries = Vec::new();
        if app.is_help() {
            entries.push((Command::Help, "close help"));
            entries.push((
                Command::Scroll(
                    crate::command::Widget::Help,
                    crate::command::DirectionVertical::Up,
                    MoveAmountVisual::Entry,
                ),
                "up",
            ));
            entries.push((
                Command::Scroll(
                    crate::command::Widget::Help,
                    crate::command::DirectionVertical::Down,
                    MoveAmountVisual::Entry,
                ),
                "down",
            ));
        } else {
            entries.extend([
                (Command::Help, "help"),
                (Command::EditChannelAlias, "alias"),
                (Command::EditPersonAlias, "nick"),
                (Command::SearchMessages, "search"),
                (Command::CheckSpelling, "spelling"),
                (Command::JumpOldest, "oldest"),
                (Command::JumpNewest, "newest"),
                (Command::AttachFiles, "attach"),
                (Command::CaptureScreenshot, "screenshot"),
            ]);
            entries.extend([
                (Command::ToggleFavorite, "favorite"),
                (Command::HideChannel, "hide"),
                (Command::ShowHiddenChannels, "restore"),
            ]);
        }
        let mut entries: Vec<_> = entries
            .into_iter()
            .filter_map(|(cmd, label)| command_key(app, &cmd).map(|key| (key, label.to_string())))
            .collect();
        if !app.is_help() {
            let label = if !app.pending_attachments.is_empty() {
                "send files"
            } else if app.is_multiline_input {
                "newline"
            } else if app.input.is_empty() {
                "open link"
            } else if app.is_editing() {
                "save edit"
            } else {
                "send"
            };
            if fallback_key(app, KeyCode::Enter) {
                entries.insert(0, ("Enter".into(), label.into()));
            }
            if app.composer_error.is_some()
                && app.event_to_command(&KeyCode::Esc.into()) != Some(&Command::ToggleShortcutBar)
            {
                entries.push(("Esc".into(), "dismiss notice".into()));
            } else if !app.pending_attachments.is_empty() && fallback_key(app, KeyCode::Esc) {
                entries.push(("Esc".into(), "remove files".into()));
            } else if app.is_editing() && fallback_key(app, KeyCode::Esc) {
                entries.push(("Esc".into(), "cancel edit".into()));
            }
        }
        entries
    };
    if let Some(key) = command_key(app, &Command::ToggleShortcutBar) {
        result.insert(0, (key, "keys".into()));
    }
    sort_hints(&mut result);
    result
}

fn command_key(app: &crate::app::App, command: &crate::command::Command) -> Option<String> {
    let mut keys = app
        .mode_keybindings
        .values()
        .flat_map(|bindings| bindings.iter())
        .filter(|(key, cmd)| {
            let event: crossterm::event::KeyEvent = (**key).into();
            let literal_slash = *command == crate::command::Command::SearchMessages
                && !app.input.is_empty()
                && event.code == crossterm::event::KeyCode::Char('/')
                && event.modifiers.is_empty();
            let notice_owns_escape = event.code == crossterm::event::KeyCode::Esc
                && app.composer_error.is_some()
                && conversation_view(app)
                && **cmd != crate::command::Command::ToggleShortcutBar;
            *cmd == command
                && !literal_slash
                && !notice_owns_escape
                && app.event_to_command(&event) == Some(command)
        })
        .map(|(key, _)| key.to_string())
        .collect::<Vec<_>>();
    keys.sort_by_key(|key| (function_number(key).is_none(), key.len(), key.clone()));
    keys.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn column_layout_preserves_complete_hints_and_unicode_widths() {
        let hints = vec![
            ("F1".into(), "help".into()),
            ("F10".into(), "試験".into()),
            ("Ctrl-Home".into(), "oldest".into()),
            ("Alt-P".into(), "preview".into()),
        ];
        for width in [4, 16, 40, 100] {
            let rows = hint_lines(&hints, width);
            assert!(rows.iter().all(|line| line.width() <= usize::from(width)));
            let text: String = rows.iter().map(ToString::to_string).collect();
            let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            for (key, label) in &hints {
                assert!(
                    compact.contains(&format!("{key}{label}")),
                    "{width}: {text}"
                );
            }
        }
        assert!(hint_lines(&hints, 0).is_empty());
        assert_eq!(hint_lines(&hints, 100).len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remapped_enter_and_escape_do_not_advertise_fallback_actions() {
        use crossterm::event::KeyCode;
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.input.put_char('x');
        app.on_pgup();
        let config = toml::from_str("[normal]\nEnter = \"jump_newest\"\nEsc = \"help\"\n").unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&config, false).unwrap();
        let context = context_items(&app);
        assert!(
            !context
                .iter()
                .any(|(key, _)| key == "Enter" || key == "Esc")
        );
        assert!(
            !items(&app)
                .iter()
                .any(|(key, label)| key == "Enter" && label == "send")
        );
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert!(app.is_help());
        app.toggle_help();
        app.composer_error = Some("Notice".into());
        assert!(items(&app).contains(&("Esc".into(), "dismiss notice".into())));
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert!(app.composer_error.is_none());
        assert!(!app.is_help());
        let config = toml::from_str("[normal]\nEsc = \"toggle_shortcut_bar\"\n").unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&config, false).unwrap();
        app.composer_error = Some("Notice".into());
        assert!(!items(&app).contains(&("Esc".into(), "dismiss notice".into())));
        let before = app.shortcut_bar_visible;
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert_ne!(app.shortcut_bar_visible, before);
        assert!(app.composer_error.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn context_sits_above_main_and_entire_footer_ignores_mouse() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.shortcut_bar_visible = true;
        let id = app.selected_channel_id().unwrap();
        let template = app.channel(id).unwrap().clone();
        for n in 1..=25 {
            let mut channel = template.clone();
            channel.id = crate::data::ChannelId::Group([n; 32]);
            app.store_channel(channel);
        }
        let area = Rect::new(0, 0, 100, 30);
        let initial = height(&app, area);
        app.on_pgup();
        let layout = BarLayout::new(&app, area.width);
        assert!(!layout.context.is_empty());
        assert!(height(&app, area) > initial);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let footer_height = height(&app, area);
        let buffer = terminal.backend().buffer();
        let row_text = |y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        };
        assert_eq!(
            row_text(area.height - layout.main.len() as u16),
            format!("{:<100}", layout.main[0].to_string())
        );
        for row in area.height - footer_height..area.height {
            for kind in [
                MouseEventKind::Down(MouseButton::Left),
                MouseEventKind::ScrollDown,
            ] {
                app.on_mouse(
                    MouseEvent {
                        kind,
                        column: 2,
                        row,
                        modifiers: KeyModifiers::NONE,
                    },
                    area,
                );
                assert_eq!(app.selected_channel_id(), Some(id));
            }
        }
        app.on_pgdn();
        assert!(BarLayout::new(&app, area.width).context.is_empty());
        assert_eq!(height(&app, area), initial);
        app.on_pgup();
        app.open_hidden_channels();
        assert!(BarLayout::new(&app, area.width).context.is_empty());
        app.hidden_dialog = None;
        for (width, height) in [(1, 1), (1, 2), (8, 4), (40, 10)] {
            terminal.backend_mut().resize(width, height);
            terminal.autoresize().unwrap();
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn separate_context_keeps_main_stable_and_function_keys_sorted() {
        use crossterm::event::KeyCode;
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.input.put_char('x');
        let id = app.selected_channel_id().unwrap();
        app.store_message(
            id,
            crate::data::Message::text(uuid::Uuid::new_v4(), 1, "Incoming".into()),
        );
        let main = items(&app);
        let keys: Vec<_> = main
            .iter()
            .filter_map(|(key, _)| key.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()))
            .collect();
        assert_eq!(keys, (1..=9).collect::<Vec<_>>());
        assert!(context_items(&app).is_empty());
        app.on_pgup();
        assert_eq!(items(&app), main);
        let context = context_items(&app);
        assert!(context.iter().any(|(_, label)| label == "reply"));
        assert!(context.iter().any(|(_, label)| label == "forward"));
        assert!(!context.iter().any(|(_, label)| label == "edit"));
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert!(context_items(&app).is_empty());
        assert_eq!(items(&app), main);
        let config = toml::from_str("[anywhere]\nF12 = \"toggle_shortcut_bar\"\n[normal]\nF10 = \"attach_files\"\nF2 = \"search_messages\"\n").unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&config, false).unwrap();
        let hints = items(&app);
        assert_eq!(
            hints
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            ["F2", "F10", "F12", "Enter"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn contexts_use_configured_keys_and_track_reply_selection() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.input.put_char('x');
        assert!(items(&app).iter().any(|(_, label)| label == "send"));
        app.on_pgup();
        assert!(
            context_items(&app)
                .iter()
                .any(|(_, label)| label == "reply")
        );
        app.on_pgdn();
        assert!(items(&app).iter().any(|(_, label)| label == "send"));
        let config = toml::from_str(
            "[anywhere]\nF9 = \"toggle_shortcut_bar\"\n[normal]\nF8 = \"attach_files\"\nctrl-f = \"search_messages\"\nctrl-up = \"jump_oldest\"\nctrl-down = \"jump_newest\"\n",
        )
        .unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&config, false).unwrap();
        let hints = items(&app);
        assert!(hints.contains(&("F9".into(), "keys".into())));
        assert!(hints.contains(&("F8".into(), "attach".into())));
        assert!(
            hints.contains(&(
                "ctrl-f"
                    .parse::<crokey::KeyCombination>()
                    .unwrap()
                    .to_string(),
                "search".into()
            )),
            "actual hints: {hints:?}"
        );
        assert!(
            hints.contains(&(
                "ctrl-up"
                    .parse::<crokey::KeyCombination>()
                    .unwrap()
                    .to_string(),
                "oldest".into()
            )),
            "actual hints: {hints:?}"
        );
        assert!(
            hints.contains(&(
                "ctrl-down"
                    .parse::<crokey::KeyCombination>()
                    .unwrap()
                    .to_string(),
                "newest".into()
            )),
            "actual hints: {hints:?}"
        );
        assert!(!hints.iter().any(|(key, _)| key == "F4"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_hint_uses_f8_while_composer_contains_text() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.mode_keybindings = crate::command::get_keybindings(&Default::default(), true).unwrap();
        assert!(items(&app).contains(&("F8".into(), "search".into())));
        app.input.put_char('x');
        assert!(items(&app).contains(&("F8".into(), "search".into())));
        assert!(!items(&app).contains(&("/".into(), "search".into())));
    }

    #[test]
    fn fills_rows_and_wraps_shortcuts() {
        let mut terminal = Terminal::new(TestBackend::new(8, 2)).unwrap();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    &ShortcutBarConfig::default(),
                    &[("F4".into(), "Help".into()), ("F5".into(), "Next".into())],
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..8).map(|x| buffer[(x, 0)].symbol()).collect();
        assert_eq!(text, "F4 Help ");
        let second: String = (0..8).map(|x| buffer[(x, 1)].symbol()).collect();
        assert_eq!(second, "F5 Next ");
        for x in 0..8 {
            assert_eq!(buffer[(x, 0)].bg, Color::Rgb(211, 211, 211));
            assert_eq!(buffer[(x, 0)].fg, Color::Black);
        }
    }

    #[test]
    fn dark_preset_and_overrides_fill_empty_row() {
        let mut terminal = Terminal::new(TestBackend::new(4, 1)).unwrap();
        let mut config = ShortcutBarConfig {
            theme: ShortcutBarTheme::Dark,
            ..Default::default()
        };
        terminal
            .draw(|frame| draw(frame, frame.area(), &config, &[]))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(3, 0)].bg,
            Color::Rgb(48, 48, 48)
        );
        assert_eq!(
            terminal.backend().buffer()[(3, 0)].fg,
            Color::Rgb(224, 224, 224)
        );
        config.foreground = Some("#123456".into());
        config.background = Some("#AbCdEf".into());
        terminal
            .draw(|frame| draw(frame, frame.area(), &config, &[]))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(3, 0)].fg,
            Color::Rgb(18, 52, 86)
        );
        assert_eq!(
            terminal.backend().buffer()[(3, 0)].bg,
            Color::Rgb(171, 205, 239)
        );
    }
}
