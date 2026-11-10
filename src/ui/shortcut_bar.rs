use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::config::{ShortcutBarConfig, ShortcutBarTheme};

fn color(value: Option<&str>, fallback: Color) -> Color {
    value
        .and_then(|value| value.strip_prefix('#'))
        .filter(|value| value.len() == 6 && value.is_ascii())
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .map(|rgb| Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
        .unwrap_or(fallback)
}

pub(crate) fn draw(
    frame: &mut Frame,
    area: Rect,
    config: &ShortcutBarConfig,
    shortcuts: &[(String, String)],
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
    let spans = shortcuts
        .iter()
        .flat_map(|(key, label)| {
            [
                Span::styled(key.as_str(), style.add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {label}  ")),
            ]
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(style),
        Rect { height: 1, ..area },
    );
}

pub(crate) fn items(app: &crate::app::App) -> Vec<(String, String)> {
    use crate::command::{Command, MoveAmountVisual, MoveDirection};
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
        let selected = app
            .selected_channel_id()
            .and_then(|id| app.positions.get(&id))
            .is_some_and(|pos| pos.selected.is_some());
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
                (Command::SearchMessages, "search"),
                (Command::CheckSpelling, "spelling"),
                (Command::JumpOldest, "oldest"),
                (Command::JumpNewest, "newest/follow"),
                (Command::AttachFiles, "attach"),
                (Command::CaptureScreenshot, "screenshot"),
            ]);
            if selected && app.input.is_empty() && !app.is_editing() {
                entries.push((Command::EditMessage, "edit"));
            }
            if selected {
                entries.push((Command::ViewImage, "image"));
                entries.push((Command::SelectText, "select text"));
                entries.push((Command::ForwardMessage, "forward"));
            }
            entries.extend([
                (Command::ToggleFavorite, "favorite"),
                (Command::HideChannel, "hide"),
                (Command::ShowHiddenChannels, "restore"),
            ]);
            if selected {
                entries.push((
                    Command::SelectMessage(MoveDirection::Next, MoveAmountVisual::Entry),
                    "next/unselect",
                ));
            }
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
            } else if selected {
                "reply"
            } else {
                "send"
            };
            entries.insert(0, ("Enter".into(), label.into()));
            if app.composer_error.is_some() {
                entries.push(("Esc".into(), "dismiss notice".into()));
            } else if !app.pending_attachments.is_empty() {
                entries.push(("Esc".into(), "remove files".into()));
            } else if app.is_editing() {
                entries.push(("Esc".into(), "cancel edit".into()));
            } else if selected {
                entries.push(("Esc".into(), "unselect".into()));
            }
        }
        entries
    };
    if let Some(key) = command_key(app, &Command::ToggleShortcutBar) {
        result.insert(0, (key, "keys".into()));
    }
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
            *cmd == command && !literal_slash && app.event_to_command(&event) == Some(command)
        })
        .map(|(key, _)| key.to_string())
        .collect::<Vec<_>>();
    keys.sort_by_key(|key| (key.len(), key.clone()));
    keys.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[tokio::test(flavor = "multi_thread")]
    async fn contexts_use_configured_keys_and_track_reply_selection() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.input.put_char('x');
        assert!(items(&app).iter().any(|(_, label)| label == "send"));
        app.on_pgup();
        assert!(items(&app).iter().any(|(_, label)| label == "reply"));
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
                "newest/follow".into()
            )),
            "actual hints: {hints:?}"
        );
        assert!(!hints.iter().any(|(key, _)| key == "F4"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_hint_uses_f8_while_composer_contains_text() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.mode_keybindings = crate::command::get_keybindings(&Default::default(), true).unwrap();
        assert!(items(&app).contains(&("/".into(), "search".into())));
        app.input.put_char('x');
        assert!(items(&app).contains(&("F8".into(), "search".into())));
        assert!(!items(&app).contains(&("/".into(), "search".into())));
    }

    #[test]
    fn fills_row_and_clips_shortcuts() {
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
        for x in 0..8 {
            assert_eq!(buffer[(x, 0)].bg, Color::Rgb(211, 211, 211));
            assert_eq!(buffer[(x, 0)].fg, Color::Black);
            assert_eq!(buffer[(x, 1)].symbol(), " ");
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
