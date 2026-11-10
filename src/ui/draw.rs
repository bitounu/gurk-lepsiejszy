//! Draw the UI

use std::fmt;

use chrono::Datelike;
use itertools::Itertools;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListDirection, ListItem, Paragraph};
use ratatui::{Frame, widgets::ListState};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Padding,
};
use ratatui::{
    style::{Color, Modifier, Style},
    widgets::Wrap,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use uuid::Uuid;

use crate::app::App;
use crate::channels::SelectChannel;
use crate::command::{Command, WindowMode};
use crate::cursor::Cursor;
use crate::data::{AssociatedValue, Message};
use crate::receipt::Receipt;
use crate::storage::MessageId;
use crate::util::utc_timestamp_msec_to_local;

use super::CHANNEL_VIEW_RATIO;
use super::image_preview::{PreviewCache, render_preview};
use super::name_resolver::NameResolver;

/// The main function drawing the UI for each frame
pub fn draw(f: &mut Frame, app: &mut App) {
    app.refresh_mentions();
    app.refresh_spelling();
    let full = f.area();
    let show_bar = app.shortcut_bar_visible && full.height > 1;
    let area = Rect {
        height: full.height.saturating_sub(u16::from(show_bar)),
        ..full
    };
    let shortcuts = if show_bar {
        super::shortcut_bar::items(app)
    } else {
        Vec::new()
    };
    draw_content(f, app, area);
    if show_bar {
        super::shortcut_bar::draw(
            f,
            Rect::new(full.x, full.bottom() - 1, full.width, 1),
            &app.config.shortcut_bar,
            &shortcuts,
        );
    }
}

fn draw_content(f: &mut Frame, app: &mut App, area: Rect) {
    if let Some(selection) = app.text_selection.as_mut() {
        selection.draw(f, area);
        return;
    }
    if app.image_viewer.is_some() {
        super::image_viewer::draw(f, app, area);
        return;
    }
    if app.is_help() {
        // Display shortcut panel
        let chunks = Layout::default()
            .constraints([
                Constraint::Percentage(5),
                Constraint::Percentage(90),
                Constraint::Percentage(5),
            ])
            .direction(Direction::Horizontal)
            .split(area);
        draw_help(f, app, chunks[1]);
        return;
    }

    let chunks = if app.is_channel_list_shown() {
        Layout::default()
            .constraints([
                Constraint::Ratio(1, CHANNEL_VIEW_RATIO),
                Constraint::Ratio(3, CHANNEL_VIEW_RATIO),
            ])
            .direction(Direction::Horizontal)
            .split(area)
    } else {
        Layout::default()
            .constraints([Constraint::Percentage(100)])
            .direction(Direction::Horizontal)
            .split(area)
    };

    if app.is_channel_list_shown() {
        draw_channels(f, app, chunks[0]);
        draw_chat(f, app, chunks[1]);
    } else {
        draw_chat(f, app, chunks[0]);
    }

    if app.spelling_dialog.is_some() {
        draw_spelling_dialog(f, app, area);
    }
    if app.select_channel.is_shown {
        draw_select_channel_popup(f, &mut app.select_channel, area);
    }
    if let Some(editor) = app.alias_editor.as_ref() {
        draw_alias_editor(f, editor, area);
    }
    if app.forward_dialog.is_some() {
        draw_forward_dialog(f, app, area);
    }
    if app.hidden_dialog.is_some() {
        draw_hidden_dialog(f, app, area);
    }
    if let Some(picker) = app.file_picker.as_mut() {
        picker.draw(f, app.config.nerd_font_icons, area);
    }
}

fn draw_forward_dialog(f: &mut Frame, app: &App, area: Rect) {
    let Some(dialog) = app.forward_dialog.as_ref() else {
        return;
    };
    let area = centered_rect(85, 70, area);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .title("Forward message — choose recipient")
        .title_bottom("Up/Down: select | Enter: send | Esc: cancel");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(inner);
    if dialog.entries.is_empty() {
        f.render_widget(Paragraph::new("No other visible conversations"), rows[0]);
    } else {
        let mut state = ListState::default();
        state.select(Some(dialog.selected));
        f.render_stateful_widget(
            List::new(
                dialog
                    .entries
                    .iter()
                    .map(|(_, name)| ListItem::new(super::file_picker::terminal_text(name))),
            )
            .highlight_style(Style::default().reversed()),
            rows[0],
            &mut state,
        );
    }
    let status = dialog.error.as_deref().unwrap_or(if dialog.preparing {
        "Preparing attachments — Esc cancels"
    } else {
        "Sends text/files without source attribution. Enter confirms sending."
    });
    f.render_widget(
        Paragraph::new(super::file_picker::terminal_text(status)).wrap(Wrap { trim: false }),
        rows[1],
    );
}

fn draw_hidden_dialog(f: &mut Frame, app: &App, area: Rect) {
    let Some(dialog) = app.hidden_dialog.as_ref() else {
        return;
    };
    let area = centered_rect(85, 70, area);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .title("Hidden conversations")
        .title_bottom("Up/Down: select | Enter: restore | Esc: close");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let regions = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    if dialog.entries.is_empty() {
        f.render_widget(Paragraph::new("No hidden conversations"), regions[0]);
    } else {
        let rows = dialog
            .entries
            .iter()
            .map(|(_, name)| ListItem::new(super::file_picker::terminal_text(name)));
        let mut state = ListState::default();
        state.select(Some(dialog.selected));
        f.render_stateful_widget(
            List::new(rows).highlight_style(Style::default().reversed()),
            regions[0],
            &mut state,
        );
    }
    if let Some(error) = dialog.error.as_ref() {
        f.render_widget(
            Paragraph::new(super::file_picker::terminal_text(error))
                .style(Style::default().fg(Color::Red)),
            regions[1],
        );
    }
}

fn draw_alias_editor(f: &mut Frame, editor: &crate::app::aliases::AliasEditor, area: Rect) {
    let area = centered_rect(85, 55, area);
    f.render_widget(Clear, area);
    let title = match editor.target {
        crate::storage::AliasTarget::Person(_) => "Local person alias",
        crate::storage::AliasTarget::Channel(_) => "Local channel alias",
    };
    let block = Block::bordered()
        .title(title)
        .title_bottom("Enter: save | Esc: cancel | Empty: remove");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
    let cursor_col = editor.input.cursor.col;
    let scroll = cursor_col.saturating_sub(inner.width.saturating_sub(1) as usize);
    f.render_widget(
        Paragraph::new(editor.input.data.as_str())
            .scroll((0, scroll.min(u16::MAX as usize) as u16)),
        chunks[0],
    );
    if let Some(error) = editor.error.as_ref() {
        f.render_widget(
            Paragraph::new(error.as_str())
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: true }),
            chunks[1],
        );
    }
    f.set_cursor_position((inner.x + (cursor_col - scroll) as u16, inner.y));
}

fn draw_select_channel_popup(f: &mut Frame, select_channel: &mut SelectChannel, area: Rect) {
    let area = centered_rect(60, 60, area);
    let chunks = Layout::default()
        .constraints([Constraint::Length(1 + 2), Constraint::Min(0)])
        .direction(Direction::Vertical)
        .split(area);
    f.render_widget(Clear, area);
    let input = Paragraph::new(Text::from(select_channel.input.data.clone())).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Select channel"),
    );
    f.render_widget(input, chunks[0]);
    let cursor = &select_channel.input.cursor;
    f.set_cursor_position((
        chunks[0].x + cursor.col as u16 + 1,
        chunks[0].y + cursor.line as u16 + 1,
    ));
    let items: Vec<_> = select_channel.filtered_names().map(ListItem::new).collect();
    match select_channel.state.selected() {
        Some(idx) if items.len() <= idx => {
            select_channel.state.select(items.len().checked_sub(1));
        }
        None if !items.is_empty() => {
            select_channel.state.select(Some(0));
        }
        _ => (),
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(Style::default().reversed());
    f.render_stateful_widget(list, chunks[1], &mut select_channel.state);
}

fn draw_channels(f: &mut Frame, app: &mut App, area: Rect) {
    let channel_list_width = area.width.saturating_sub(2) as usize;
    let favorite_count = app.favorite_channel_count();
    let separator = favorite_count > 0 && favorite_count < app.channels().len();
    let channels = app.channels().iter().enumerate().map(|(index, channel)| {
        let unread_messages_label = if channel.unread_messages != 0 {
            format!(" ({})", channel.unread_messages)
        } else {
            String::new()
        };
        let mute_label = if channel.muted { " [M]" } else { "" };
        let typing_label = if channel.is_writing() { " [T]" } else { "" };
        let favorite_label = if app.favorite_channels.contains(&channel.id) {
            " [*]"
        } else {
            ""
        };
        let suffix = format!("{favorite_label}{unread_messages_label}{mute_label}{typing_label}");
        let channel_name = app.channel_name(channel);
        let label = format!("{channel_name}{suffix}");
        let label_width = label.width();
        let label = if label_width <= channel_list_width || suffix.is_empty() {
            label
        } else {
            let available = channel_list_width.saturating_sub(suffix.width());
            let mut width = 0;
            let clipped: String = channel_name
                .chars()
                .take_while(|c| {
                    width += c.width().unwrap_or(0);
                    width <= available
                })
                .collect();
            format!("{clipped}{suffix}")
        };
        // Keep the separator inside the final favorite's height, never as a selectable item.
        let mut lines = vec![Line::raw(label)];
        if separator && index + 1 == favorite_count {
            lines.push(Line::raw(""));
        }
        ListItem::new(lines)
    });

    let channels = List::new(channels)
        .block(Block::default().borders(Borders::ALL).title("Channels"))
        .highlight_style(Style::default().reversed());
    let no_channels = channels.is_empty();
    f.render_stateful_widget(channels, area, &mut app.channels.state);
    // Overlay the separator so it never inherits the selected item's reverse style.
    let offset = app.channels.state.offset();
    if separator && offset < favorite_count && area.width > 2 && area.height > 2 {
        let row = favorite_count - offset;
        if row < usize::from(area.height - 2) {
            let separator_area = Rect::new(area.x + 1, area.y + 1 + row as u16, area.width - 2, 1);
            f.render_widget(Clear, separator_area);
            f.render_widget(
                Paragraph::new("─".repeat(channel_list_width)),
                separator_area,
            );
        }
    }

    if no_channels {
        f.render_widget(
            Paragraph::new("No channels\n(Channels will be added on incoming messages)")
                .wrap(Wrap { trim: true })
                .centered()
                .block(Block::default().padding(Padding::top(area.height / 2))),
            area,
        );
    };
}

fn wrap(text: &str, mut cursor: Cursor, width: usize) -> (String, Cursor, usize) {
    let mut res = String::new();

    let mut line = 0;
    let mut col = 0;

    for c in text.chars() {
        // current line too long => wrap
        if col > 0 && col % width == 0 {
            res.push('\n');

            // adjust cursor
            if line < cursor.line {
                cursor.line += 1;
                cursor.idx += 1;
            } else if line == cursor.line && col <= cursor.col {
                cursor.line += 1;
                cursor.col -= col;
                cursor.idx += 1;
            }

            line += 1;
            col = 0;
        }

        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += c.width().unwrap_or(0);
        }
        res.push(c);
    }

    // special case: cursor is at the end of the text and overflows `width`
    if cursor.idx == res.len() && cursor.col == width {
        res.push('\n');
        cursor.line += 1;
        cursor.col = 0;
        cursor.idx += 1;
        line += 1;
    }

    (res, cursor, line + 1)
}

// Wrapped text differs only by inserted newlines; keep byte ranges in original coordinates.
fn spelling_text(
    wrapped: &str,
    original: &str,
    errors: &[crate::spellcheck::Misspelling],
    mentions: &[crate::mentions::DraftMention],
) -> Text<'static> {
    let mut offset = 0;
    let mut lines = vec![Line::default()];
    for c in wrapped.chars() {
        let inserted = c == '\n' && !original[offset..].starts_with('\n');
        if c == '\n' {
            lines.push(Line::default());
        } else {
            let index = errors.partition_point(|error| error.range.end <= offset);
            let wrong = errors
                .get(index)
                .is_some_and(|error| error.range.contains(&offset));
            let confirmed = mentions.iter().any(|m| m.start <= offset && offset < m.end);
            let style = if confirmed {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if wrong {
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default()
            };
            let spans = &mut lines.last_mut().expect("line exists").spans;
            if let Some(last) = spans.last_mut().filter(|last| last.style == style) {
                last.content.to_mut().push(c);
            } else {
                spans.push(Span::styled(c.to_string(), style));
            }
        }
        if !inserted {
            offset += c.len_utf8();
        }
    }
    Text::from(lines)
}

fn draw_spelling_dialog(f: &mut Frame, app: &App, area: Rect) {
    let Some(dialog) = app.spelling_dialog.as_ref() else {
        return;
    };
    let width = area.width.min(64);
    let height = area.height.min(14);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let mut lines = vec![Line::from(format!("Word: {}", dialog.word))];
    if let Some(status) = app.spelling.status() {
        lines.push(Line::from(status.to_owned()));
    } else if dialog.range.is_none() {
        lines.push(Line::from("Checking… No misspelling found yet."));
    } else if app.spelling.suggestions().is_empty() {
        lines.push(Line::from("Loading suggestions, or none available."));
    }
    for (index, suggestion) in app.spelling.suggestions().iter().enumerate() {
        lines.push(Line::styled(
            format!(
                "{} {suggestion}",
                if index == dialog.selected { ">" } else { " " }
            ),
            if index == dialog.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            },
        ));
    }
    lines.push(Line::from(
        "↑/↓ choose · Enter correct · i ignore · Esc close",
    ));
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Spelling")),
        rect,
    );
}

fn draw_chat(f: &mut Frame, app: &mut App, area: Rect) {
    let search_height = if app.message_search.is_some() {
        area.height.min(4)
    } else {
        0
    };
    let search_area = Rect::new(
        area.x,
        area.bottom().saturating_sub(search_height),
        area.width,
        search_height,
    );
    let area = Rect {
        height: area.height.saturating_sub(search_height),
        ..area
    };
    let text_width = area.width.saturating_sub(2).max(1) as usize;
    let (wrapped_input, cursor, num_input_lines) =
        wrap(&app.input.data, app.input.cursor.clone(), text_width);

    let has_status = !app.pending_attachments.is_empty()
        || app.composer_error.is_some()
        || app.screenshot_in_progress
        || app.send_job.is_some();
    let attachment_height = if !has_status {
        0
    } else if app.pending_attachments.is_empty() {
        3
    } else {
        9
    };
    let chunks = Layout::default()
        .constraints([
            Constraint::Min(0),
            Constraint::Length(attachment_height),
            Constraint::Length(num_input_lines.min(u16::MAX as usize - 2) as u16 + 2),
        ])
        .direction(Direction::Vertical)
        .split(area);
    draw_messages(f, app, chunks[0]);
    super::mentions::draw(f, app, chunks[0]);
    if has_status {
        draw_pending_attachments(f, app, chunks[1]);
    }

    let title = match (app.is_editing(), app.is_multiline_input) {
        (true, true) => "Input (Editing, Multiline)",
        (true, false) => "Input (Editing)",
        (false, true) => "Input (Multiline)",
        (false, false) => "Input",
    };

    let title = app
        .spelling
        .status()
        .map(|status| format!("{title} · {status}"))
        .unwrap_or_else(|| title.to_owned());
    let input = Paragraph::new(spelling_text(
        &wrapped_input,
        &app.input.data,
        app.spelling.errors(),
        &app.input.mentions,
    ))
    .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(input, chunks[2]);
    if !app.select_channel.is_shown
        && app.spelling_dialog.is_none()
        && app.file_picker.is_none()
        && app.message_search.is_none()
        && chunks[2].width > 2
        && chunks[2].height > 2
    {
        f.set_cursor_position((
            chunks[2].x + cursor.col as u16 + 1,  // +1 for frame
            chunks[2].y + cursor.line as u16 + 1, // +1 for frame
        ));
    }
    draw_message_search(f, app, search_area);
}

fn draw_message_search(f: &mut Frame, app: &App, area: Rect) {
    let Some(search) = app.message_search.as_ref() else {
        return;
    };
    let count = search.current.map_or_else(
        || format!("0/{}", search.hits.len()),
        |index| format!("{}/{}", index + 1, search.hits.len()),
    );
    let block = Block::bordered().title(format!(
        "Search {count} · Up/Down F3/Shift+F3 · Enter accept · Esc cancel"
    ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let col = search.input.cursor.col;
    let scroll = col.saturating_sub(inner.width.saturating_sub(1) as usize);
    f.render_widget(
        Paragraph::new(search.input.data.as_str())
            .scroll((0, scroll.min(u16::MAX as usize) as u16)),
        Rect { height: 1, ..inner },
    );
    if inner.height > 1 {
        f.render_widget(
            Paragraph::new(
                search
                    .error
                    .as_deref()
                    .unwrap_or("Literal search · Enter keeps this result"),
            ),
            Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1),
        );
    }
    f.set_cursor_position((
        inner.x
            + col
                .saturating_sub(scroll)
                .min(inner.width.saturating_sub(1) as usize) as u16,
        inner.y,
    ));
}

fn draw_pending_attachments(f: &mut Frame, app: &App, area: Rect) {
    use super::file_picker::{file_icon, human_size, terminal_text};
    let title = if app.send_job.is_some() {
        "Preparing attachments — Esc: cancel"
    } else if app.composer_error.is_some() && app.pending_attachments.is_empty() {
        "Notice — Esc: dismiss"
    } else if app.composer_error.is_some() {
        "Attachments — Esc: dismiss notice | Enter: retry | Alt+A: add"
    } else {
        "Attachments — Enter: send | Esc: remove | Alt+A: add"
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let mut lines = Vec::new();
    if let Some(error) = app.composer_error.as_ref() {
        lines.push(Line::styled(
            terminal_text(error),
            Style::default().fg(Color::Red),
        ));
    }
    if app.screenshot_in_progress {
        lines.push(Line::raw("Capturing screenshot..."));
    }
    app.image_previews
        .request_visible(app.pending_attachments.iter().map(|path| path.as_path()));
    for path in app.pending_attachments.iter().take(3) {
        let size = std::fs::metadata(path)
            .map(|metadata| human_size(metadata.len()))
            .unwrap_or_else(|_| "missing".into());
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        lines.push(Line::raw(format!(
            "{} {} ({size})",
            file_icon(path, false, app.config.nerd_font_icons),
            terminal_text(&name)
        )));
    }
    if app.pending_attachments.len() > 3 {
        lines.push(Line::raw(format!(
            "... and {} more",
            app.pending_attachments.len() - 3
        )));
    }
    if let Some(image) = app
        .pending_attachments
        .iter()
        .find_map(|path| app.image_previews.cached(path))
    {
        lines.extend(render_preview(
            &image,
            inner.width as usize,
            (inner.height as usize).saturating_sub(lines.len()),
        ));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    // area without borders
    let height = area.height.saturating_sub(2) as usize;
    let width = area.width.saturating_sub(2) as usize;
    if height == 0 || width == 0 {
        return;
    }
    let title = app
        .selected_channel_id()
        .and_then(|id| app.channel(id))
        .map(|channel| format!("Messages — {}", app.channel_name(channel)))
        .unwrap_or_else(|| "Messages".to_owned());
    app.message_view_height = height;

    let Some(window) = app.window.as_ref() else {
        // No channel selected
        f.render_widget(
            Paragraph::new("No Channel selected")
                .block(
                    Block::bordered()
                        .title(title.clone())
                        .padding(Padding::top(area.height / 2)),
                )
                .centered(),
            area,
        );
        return;
    };

    if window.is_empty() {
        // No messages
        f.render_widget(
            Paragraph::new("No messages")
                .block(
                    Block::bordered()
                        .title(title.clone())
                        .padding(Padding::top(area.height / 2)),
                )
                .centered(),
            area,
        );
        return;
    }

    let pos = app
        .positions
        .entry(window.channel_id())
        .or_default()
        .clone();
    let selected = pos.selected;

    // calculate the bottom anchor
    let mut bottom = pos
        .viewport_bottom
        .unwrap_or_else(|| window.newest().expect("logic error: no messages"));
    if let Some(selected) = pos.selected
        && (
            app.storage
                .message_sort_key(MessageId::new(window.channel_id(), bottom)),
            bottom,
        ) < (
            app.storage
                .message_sort_key(MessageId::new(window.channel_id(), selected)),
            selected,
        )
    {
        // selected is newer than bottom => pull it to the bottom edge
        bottom = selected;
    }

    // build candidates and fit them into the viewport
    let (items, ats, heights) = build_candidates(app, bottom, width, height, pos.selected);
    let selected_pos = selected.and_then(|s| ats.iter().position(|&at| at == s));
    let (start, end) = fit_window(&heights, height, selected_pos);

    // Only visible messages start decoder jobs. Measuring older candidates may use cached
    // thumbnails, but must not decode the entire history or churn the bounded cache.
    if app.config.show_image_previews {
        app.image_previews.request_visible(
            ats[start..end]
                .iter()
                .filter_map(|arrived_at| window.get(*arrived_at))
                .filter(|message| !message.deleted)
                .flat_map(|message| {
                    message
                        .attachments
                        .iter()
                        .map(|attachment| attachment.filename.as_path())
                }),
        );
    }
    let visible = items[start..end].to_vec();
    let list = List::new(visible)
        .block(Block::default().title(title.clone()).borders(Borders::ALL))
        .highlight_style(Style::default().reversed())
        .direction(ListDirection::BottomToTop);
    let mut state = ListState::default();
    state.select(selected_pos.map(|s| s - start));
    f.render_stateful_widget(list, area, &mut state);

    // update the bottom anchor but only when we are not sticking to the bottom
    let new_bottom = ats[start];
    let at_tail = window.at_newest() && window.newest() == Some(new_bottom);
    let channel_id = window.channel_id();
    app.positions.entry(channel_id).or_default().viewport_bottom = (!at_tail).then_some(new_bottom);
    if app.terminal_focused
        && app.file_picker.is_none()
        && app.message_search.is_none()
        && app.forward_dialog.is_none()
        && app.hidden_dialog.is_none()
        && app.alias_editor.is_none()
        && !app.select_channel.is_shown
    {
        app.mark_visible_reads(
            ats[start..end]
                .iter()
                .map(|timestamp| MessageId::new(channel_id, *timestamp))
                .collect(),
        );
    }
}

fn build_candidates(
    app: &App,
    bottom: u64,
    width: usize,
    height: usize,
    until: Option<u64>,
) -> (Vec<ListItem<'static>>, Vec<u64>, Vec<usize>) {
    let Some(window) = app.window.as_ref() else {
        return Default::default();
    };
    let channel_id = window.channel_id();

    let unread_messages = app
        .channel(channel_id)
        .map(|c| c.unread_messages as usize)
        .unwrap_or(0);

    // collect messages >= height and far enough to include `until`
    let mut candidates: Vec<&Message> = Vec::new();
    let mut seen_until = until.is_none();
    for message in window.iter_rev_from(Some(bottom)) {
        if Some(message.storage_id()) == until {
            seen_until = true;
        }
        candidates.push(message);
        if candidates.len() >= height && seen_until {
            break;
        }
    }

    // precalculate names for candidates
    let names = NameResolver::compute(
        app,
        candidates
            .iter()
            .map(|message| MessageId::new(channel_id, message.storage_id())),
    );

    // message display options
    const TIME_WIDTH: usize = 6; // width of "00:00 "
    let mut prefix_width = TIME_WIDTH;
    if app.config.show_receipts {
        prefix_width += RECEIPT_WIDTH;
    }
    let prefix = " ".repeat(prefix_width);

    // Build items bottom-first, parallel arrived_at and height
    let mut items: Vec<ListItem<'static>> = Vec::new();
    let mut items_ats: Vec<u64> = Vec::new();
    let mut heights: Vec<usize> = Vec::new();

    let mut candidates = candidates.iter().enumerate().peekable();
    while let Some((idx, msg)) = candidates.next() {
        // the iterator is reversed, so the peeked item is the older, *previous* message
        let prev_msg = candidates.peek();
        let date_division = display_date_line(
            msg.arrived_at,
            prev_msg.map(|(_, msg)| msg.arrived_at),
            width,
        );

        let new_messages_division =
            (unread_messages > 0 && unread_messages == idx + 1).then(|| {
                "-".repeat(prefix_width)
                    + "new messages"
                    + &"-".repeat(width.saturating_sub(prefix_width))
            });
        let show_receipt = ShowReceipt::from_msg(msg, app.user_id, app.config.show_receipts);

        let focus = app
            .search_focus
            .as_ref()
            .filter(|focus| focus.channel_id == channel_id && focus.local_id == msg.storage_id());
        let focused_item = focus.and_then(|focus| {
            display_search_message(
                &names,
                msg,
                &prefix,
                width,
                height,
                show_receipt_for_focus(msg, app),
                focus,
            )
        });
        if let Some(item) = focused_item.or_else(|| {
            display_message(
                &names,
                msg,
                &prefix,
                width,
                height,
                show_receipt,
                date_division,
                new_messages_division,
                app.config.colored_messages,
                app.config
                    .show_image_previews
                    .then_some(&app.image_previews),
            )
        }) {
            let h = item.height();
            items.push(item);
            items_ats.push(msg.storage_id());
            heights.push(h);
        }
    }

    (items, items_ats, heights)
}

fn fit_window(heights: &[usize], height: usize, selected: Option<usize>) -> (usize, usize) {
    if heights.is_empty() {
        return (0, 0);
    }

    let mut rows = 0;
    let mut end = 0;
    while end < heights.len() && rows + heights[end] <= height {
        rows += heights[end];
        end += 1;
    }
    match selected {
        Some(selected) if end <= selected => {
            // selection is above the fold -> pin it to the top, fit downward
            let mut rows = 0;
            let mut start = selected;
            let mut i = selected;
            loop {
                if height < rows + heights[i] && i != selected {
                    break;
                }
                rows += heights[i];
                start = i;
                if i == 0 {
                    break;
                }
                i -= 1;
            }
            (start, selected.saturating_add(1))
        }
        _ => (0, end),
    }
}

fn display_time(timestamp: u64) -> String {
    utc_timestamp_msec_to_local(timestamp)
        .format("%R ")
        .to_string()
}

const RECEIPT_WIDTH: usize = 2;

/// Ternary state whether to show receipt for a message
enum ShowReceipt {
    // show receipt for this message
    Yes,
    // don't show receipt for this message
    No,
    // don't show receipt for any message
    Never,
}

impl ShowReceipt {
    fn from_msg(msg: &Message, user_id: Uuid, config_show_receipts: bool) -> Self {
        if config_show_receipts {
            if user_id == msg.from_id {
                Self::Yes
            } else {
                Self::No
            }
        } else {
            Self::Never
        }
    }
}

fn display_receipt(receipt: Receipt, show: ShowReceipt) -> &'static str {
    use ShowReceipt::*;
    match (show, receipt) {
        (Yes, Receipt::Nothing) => "  ",
        (Yes, Receipt::Sent) => "○ ",
        (Yes, Receipt::Delivered) => "◉ ",
        (Yes, Receipt::Read) => "● ",
        (No, _) => "  ",
        (Never, _) => "",
    }
}

// Search uses normalized body offsets, before author/quote/attachment decorations.
fn show_receipt_for_focus(msg: &Message, app: &App) -> ShowReceipt {
    ShowReceipt::from_msg(msg, app.user_id, app.config.show_receipts)
}

#[allow(clippy::too_many_arguments)]
fn display_search_message(
    names: &NameResolver,
    msg: &Message,
    prefix: &str,
    width: usize,
    height: usize,
    show_receipt: ShowReceipt,
    focus: &crate::app::search::SearchFocus,
) -> Option<ListItem<'static>> {
    if width == 0 || height == 0 || msg.deleted {
        return None;
    }
    let body = crate::search::normalize_body(msg.message.as_deref().unwrap_or_default());
    let regex = crate::search::literal_regex(&focus.query).ok().flatten()?;
    let ranges: Vec<_> = regex.find_iter(&body).map(|m| m.range()).collect();
    // Revalidate after a live edit; stored offsets are only hints for the same occurrence.
    let active_range = match ranges.get(focus.occurrence) {
        Some(range) if *range == focus.range => &focus.range,
        Some(range) => range,
        None => return None,
    };
    let (name, color) = names.resolve(msg.from_id);
    let mut first_prefix = vec![
        Span::styled(
            display_receipt(msg.receipt, show_receipt),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            display_time(msg.arrived_at),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(name.into_owned(), Style::default().fg(color)),
        Span::raw(": "),
    ];
    if first_prefix.iter().map(Span::width).sum::<usize>() >= width {
        first_prefix.clear();
    }
    let continuation = if prefix.width() < width { prefix } else { "" };
    // Display replacements carry their original source range. Search continues
    // to use literal normalized body text rather than resolved mention names.
    let mut tokens: Vec<(String, Option<std::ops::Range<usize>>, Style)> = Vec::new();
    let quote_style = Style::default().fg(Color::Rgb(150, 150, 150));
    if let Some(quote) = msg
        .quote
        .as_ref()
        .and_then(|quote| displayed_quote(names, quote))
    {
        tokens.push((format!("> {quote}\n"), None, quote_style));
    }
    let mut mentions = msg
        .body_ranges
        .iter()
        .filter_map(|range| match range.value {
            AssociatedValue::MentionUuid(id) => Some(id),
            AssociatedValue::Style(_) => None,
        });
    for (offset, grapheme) in body.grapheme_indices(true) {
        let source = offset..offset + grapheme.len();
        let display = if grapheme == "￼" {
            mentions
                .next()
                .map(|id| format!("@{}", names.resolve(id).0))
                .unwrap_or_else(|| grapheme.to_owned())
        } else {
            grapheme.to_owned()
        };
        tokens.push((display, Some(source), Style::default()));
    }
    // Keep all normal body decorations, with no searchable offsets.
    let mut decorated = body.clone();
    add_attachments(
        msg,
        &mut decorated,
        names.file_icon_setting(),
        names.show_attachment_paths(),
    );
    add_reactions(msg, &mut decorated);
    add_edited(msg, &mut decorated);
    add_expire_countdown(msg, &mut decorated);
    tokens.push((decorated[body.len()..].to_owned(), None, Style::default()));
    if let Some(reason) = msg.send_failed.as_deref() {
        tokens.push((
            format!("\n[Could not send: {reason}]"),
            None,
            Style::default().fg(Color::Red),
        ));
    }
    let mut lines = vec![Line::from(first_prefix.clone())];
    let mut used = lines[0].width();
    let mut active_line = 0;
    let mut found_active = false;
    for (display, source, base_style) in tokens {
        let overlaps = |range: &std::ops::Range<usize>| {
            source
                .as_ref()
                .is_some_and(|source| source.start < range.end && range.start < source.end)
        };
        let active = overlaps(active_range);
        let matched = source.as_ref().is_some_and(|source| {
            let index = ranges.partition_point(|range| range.end <= source.start);
            ranges
                .get(index)
                .is_some_and(|range| range.start < source.end)
        });
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightCyan)
                .add_modifier(Modifier::BOLD)
        } else if matched {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        } else {
            base_style
        };
        for grapheme in display.graphemes(true) {
            let cell_width = if grapheme == "\t" {
                4
            } else {
                grapheme.width()
            };
            if grapheme == "\n" || (used + cell_width > width && used > 0) {
                lines.push(Line::from(Span::raw(continuation.to_owned())));
                used = continuation.width();
                if grapheme == "\n" {
                    continue;
                }
            }
            if active && !found_active {
                active_line = lines.len() - 1;
                found_active = true;
            }
            lines.last_mut().unwrap().spans.push(Span::styled(
                if grapheme == "\t" {
                    "    ".to_owned()
                } else {
                    grapheme.to_owned()
                },
                style,
            ));
            used += cell_width;
        }
    }
    if lines.len() > height {
        let start = active_line
            .saturating_sub(height / 2)
            .min(lines.len() - height);
        lines = lines.into_iter().skip(start).take(height).collect();
        // Every fragment retains the sender/time header. The body keeps the same
        // continuation indentation, and the active row stays within the window.
        if start > 0 && height > 1 && active_line > start {
            lines[0] = Line::from(first_prefix);
        }
    }
    Some(ListItem::new(Text::from(lines)))
}

#[allow(clippy::too_many_arguments)]
fn display_message(
    names: &NameResolver,
    msg: &Message,
    prefix: &str,
    width: usize,
    height: usize,
    show_receipt: ShowReceipt,
    date_division: Option<String>,
    unread_messages_division: Option<String>,
    colored_messages: bool,
    previews: Option<&PreviewCache>,
) -> Option<ListItem<'static>> {
    if height == 0 || width == 0 {
        return None;
    }
    let receipt = Span::styled(
        display_receipt(msg.receipt, show_receipt),
        Style::default().fg(Color::Yellow),
    );

    let time = Span::styled(
        display_time(msg.arrived_at),
        Style::default().fg(Color::Yellow),
    );

    let (from, from_color) = names.resolve(msg.from_id);

    let from_width = from.width();
    let from = Span::styled(from.into_owned(), Style::default().fg(from_color));
    let delimiter = Span::from(": ");

    // collect message text
    let mut text = if msg.deleted {
        String::from("[message deleted]")
    } else {
        let text = strip_ansi_escapes::strip_str(msg.message.as_deref().unwrap_or_default());
        let mut text = replace_mentions(msg, names, text);
        add_attachments(
            msg,
            &mut text,
            names.file_icon_setting(),
            names.show_attachment_paths(),
        );
        if text.is_empty() {
            return None; // no text => nothing to render
        }
        add_reactions(msg, &mut text);
        add_edited(msg, &mut text);
        text
    };
    add_expire_countdown(msg, &mut text);

    let mut spans: Vec<Line> = vec![];
    if let Some(date_division) = date_division {
        spans.push(Line::from(date_division));
    }
    if let Some(unread_messages_division) = unread_messages_division {
        spans.push(Line::from(unread_messages_division));
    }

    // prepend quote if any
    let quote_text = msg
        .quote
        .as_ref()
        .and_then(|quote| displayed_quote(names, quote));
    if let Some(quote_text) = quote_text.as_ref() {
        let quote_prefix = format!("{prefix}> ");
        let quote_wrap_opts = textwrap::Options::new(width.saturating_sub(2))
            .initial_indent(&quote_prefix)
            .subsequent_indent(&quote_prefix);
        let quote_style = Style::default().fg(Color::Rgb(150, 150, 150));
        spans.extend(
            textwrap::wrap(quote_text, quote_wrap_opts)
                .into_iter()
                .enumerate()
                .map(|(idx, line)| {
                    let res = if idx == 0 {
                        vec![
                            receipt.clone(),
                            time.clone(),
                            from.clone(),
                            delimiter.clone(),
                            Span::styled(
                                line.strip_prefix(prefix).unwrap().to_owned(),
                                quote_style,
                            ),
                        ]
                    } else {
                        vec![Span::styled(line.into_owned(), quote_style)]
                    };
                    Line::from(res)
                }),
        );
    }

    let add_time = quote_text.is_none();
    let message_style = if msg.deleted {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC)
    } else if colored_messages {
        Style::default().fg(from_color)
    } else {
        Style::default()
    };

    const DELIMITER_WIDTH: usize = 2;
    let first_line_prefix = " ".repeat(prefix.len() + from_width + DELIMITER_WIDTH);
    let wrap_opts = textwrap::Options::new(width)
        .initial_indent(if add_time { &first_line_prefix } else { prefix })
        .subsequent_indent(prefix);
    let mut wrapped_text = textwrap::wrap(&text, &wrap_opts).into_iter();

    if add_time && let Some(first_line) = wrapped_text.next() {
        let line = first_line
            .strip_prefix(&first_line_prefix)
            .expect("logic error")
            .to_owned();
        spans.push(Line::from(vec![
            receipt,
            time,
            from,
            delimiter,
            Span::styled(line, message_style),
        ]));
    }
    spans.extend(
        wrapped_text.map(|line| Line::from(Span::styled(line.into_owned(), message_style))),
    );

    if let Some(reason) = msg.send_failed.as_deref() {
        let error = format!("[Could not send: {reason}]");
        let error_style = Style::default().fg(Color::Red);
        spans.extend(
            textwrap::wrap(&error, &wrap_opts)
                .into_iter()
                .map(|line| Span::styled(line.into_owned(), error_style).into()),
        );
    }

    let images: Vec<_> = if !msg.deleted && width > prefix.width() && height > 1 {
        previews
            .into_iter()
            .flat_map(|cache| {
                msg.attachments
                    .iter()
                    .filter_map(|attachment| cache.cached(&attachment.filename))
            })
            .take(height)
            .collect()
    } else {
        Vec::new()
    };
    let preview_budget = if images.is_empty() {
        0
    } else {
        (height / 2).max(1).min(height - 1).min(12 * images.len())
    };
    let text_height = height - preview_budget;
    if spans.len() > text_height {
        spans.resize(text_height.saturating_sub(1), Line::from(""));
        spans.push(Line::from(format!("{prefix}[...]")));
    }
    for (idx, image) in images.iter().enumerate() {
        let rows = (height - spans.len()) / (images.len() - idx);
        let lines = render_preview(image, width - prefix.width(), rows);
        spans.extend(lines.into_iter().map(|mut line| {
            line.spans.insert(0, Span::raw(prefix.to_owned()));
            line
        }));
    }
    Some(ListItem::new(Text::from(spans)))
}

fn replace_mentions(msg: &Message, names: &NameResolver, text: String) -> String {
    if msg.body_ranges.is_empty() {
        return text;
    }

    let ac = aho_corasick::AhoCorasickBuilder::new()
        .build(std::iter::repeat_n("￼", msg.body_ranges.len())) // TODO: cache
        .expect("failed to build obj replacer");
    let mut buf = String::with_capacity(text.len());
    let mut ranges = msg.body_ranges.iter();
    ac.replace_all_with(&text, &mut buf, |_, _, dst| {
        // TODO: check ranges?
        for range in &mut ranges {
            let (name, _color) = match range.value {
                AssociatedValue::MentionUuid(id) => names.resolve(id),
                AssociatedValue::Style(_) => continue, // not supported yet
            };
            dst.push('@');
            dst.push_str(&name);
            return true;
        }
        false
    });

    buf
}

fn display_date_line(
    msg_timestamp: u64,
    previous_msg_timestamp: Option<u64>,
    width: usize,
) -> Option<String> {
    let local_time = utc_timestamp_msec_to_local(msg_timestamp);
    let current_msg_day = local_time.num_days_from_ce();
    let previous_msg_day =
        previous_msg_timestamp.map(|t| utc_timestamp_msec_to_local(t).num_days_from_ce());

    if Some(current_msg_day) != previous_msg_day {
        let date = format!("{:=^width$}", local_time.format(" %A, %x "));
        Some(date)
    } else {
        None
    }
}

fn add_attachments(msg: &Message, out: &mut String, icons: Option<bool>, show_paths: bool) {
    for attachment in &msg.attachments {
        if !out.is_empty() {
            out.push('\n');
        }
        if let Some(nerd) = icons {
            out.push_str(super::file_picker::file_icon(
                &attachment.filename,
                false,
                nerd,
            ));
            out.push(' ');
        }
        if attachment.content_type.starts_with("audio/") {
            out.push_str("[Voice Note] ");
        }
        let label = if show_paths {
            attachment.filename.as_os_str()
        } else {
            attachment
                .filename
                .file_name()
                .unwrap_or_else(|| attachment.filename.as_os_str())
        };
        let label = super::file_picker::terminal_text(&label.to_string_lossy());
        if show_paths {
            out.push_str(&format!("<file://{label}>"));
        } else {
            out.push_str(&label);
        }
    }
    if msg.attachment_downloads.failed > 0 {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "<failed to download attachments ({})>",
            msg.attachment_downloads.failed
        ));
    }
    if msg.attachment_downloads.pending > 0 {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "<downloading attachments ({})...>",
            msg.attachment_downloads.pending
        ));
    }
}

fn add_reactions(msg: &Message, out: &mut dyn fmt::Write) {
    if !msg.reactions.is_empty() {
        fmt::write(
            out,
            format_args!(
                " [{}]",
                msg.reactions.iter().map(|(_, emoji)| emoji).format("")
            ),
        )
        .expect("formatting reactions failed");
    }
}

fn add_edited(msg: &Message, out: &mut dyn fmt::Write) {
    if msg.edited {
        write!(out, " [edited]").expect("formatting edited failed")
    }
}

fn format_duration_short(seconds: u64) -> String {
    if seconds >= 604800 {
        format!("{}w", seconds / 604800)
    } else if seconds >= 86400 {
        format!("{}d", seconds / 86400)
    } else if seconds >= 3600 {
        format!("{}h", seconds / 3600)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

fn add_expire_countdown(msg: &Message, out: &mut dyn fmt::Write) {
    if msg.expire_timer.is_some_and(|t| t > 0) {
        if let Some(expires_at) = msg.expires_at {
            let now_ms = crate::util::utc_now_timestamp_msec();
            if now_ms < expires_at {
                let remaining_secs = (expires_at - now_ms) / 1000;
                write!(out, " [{}]", format_duration_short(remaining_secs))
            } else {
                write!(out, " [expired]")
            }
            .expect("formatting countdown failed");
        } else {
            write!(out, " [pending]").expect("formatting countdown failed");
        }
    }
}

fn help_commands<'a>() -> Vec<Line<'a>> {
    let commands = <Command as strum::IntoEnumIterator>::iter()
        .map(|cmd| {
            (
                strum::EnumProperty::get_str(&cmd, "usage")
                    .unwrap_or(&cmd.to_string())
                    .to_string(),
                strum::EnumProperty::get_str(&cmd, "desc")
                    .unwrap_or("Undocumented")
                    .to_string(),
            )
        })
        .collect_vec();
    let usage_len = commands.iter().map(|inf| inf.0.len()).max().unwrap_or(0);
    let commands = commands
        .iter()
        .map(|inf| Line::raw(format!("{: <usage_len$}   {}", inf.0, inf.1)));
    let mut v = vec![
        Line::styled("Commands", Style::default().add_modifier(Modifier::BOLD)),
        Line::default(),
    ];
    v.extend(commands);
    v
}

fn bindings(app: &App) -> Vec<Line<'_>> {
    [
        WindowMode::Normal,
        WindowMode::Anywhere,
        WindowMode::Help,
        WindowMode::ChannelModal,
        WindowMode::Multiline,
        WindowMode::MessageSelected,
    ]
    .iter()
    .map(|mode| bindings_mode(app, mode))
    .concat()
}

fn bindings_mode<'a>(app: &App, mode: &WindowMode) -> Vec<Line<'a>> {
    let bindings = if let Some(kb) = app.mode_keybindings.get(mode) {
        kb.iter()
            .map(|(kc, cmd)| {
                (
                    kc.to_string(),
                    cmd.to_string(),
                    strum::EnumProperty::get_str(cmd, "desc")
                        .unwrap_or("Undocumented")
                        .to_string(),
                )
            })
            .sorted()
            .collect_vec()
    } else {
        Vec::default()
    };
    let kc_len = bindings.iter().map(|inf| inf.0.len()).max().unwrap_or(0);
    let cmd_len = bindings.iter().map(|inf| inf.1.len()).max().unwrap_or(0);
    let bindings = bindings.iter().map(|inf| {
        Line::raw(format!(
            "{: <kc_len$}  {: <cmd_len$}  {}",
            inf.0, inf.1, inf.2
        ))
    });
    let mut v = vec![
        Line::default(),
        Line::styled(
            format!("Bindings for {mode} mode"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    v.extend(bindings);
    v
}

fn help_indicators<'a>() -> Vec<Line<'a>> {
    let indicators: &[(&str, &str)] = &[
        ("(N)", "N unread messages in channel"),
        ("[M]", "Channel is muted (notifications silenced)"),
        ("○", "Message sent"),
        ("◉", "Message delivered"),
        ("●", "Message read"),
    ];
    let label_len = indicators.iter().map(|i| i.0.len()).max().unwrap_or(0);
    let mut v = vec![
        Line::default(),
        Line::styled("Indicators", Style::default().add_modifier(Modifier::BOLD)),
        Line::default(),
    ];
    v.extend(
        indicators
            .iter()
            .map(|i| Line::raw(format!("{: <label_len$}   {}", i.0, i.1))),
    );
    v
}

fn quick_help(app: &App) -> Vec<Line<'static>> {
    let keys = |command: Command| {
        let keys = [
            WindowMode::Anywhere,
            WindowMode::Normal,
            WindowMode::MessageSelected,
            WindowMode::Multiline,
        ]
        .iter()
        .filter_map(|mode| app.mode_keybindings.get(mode))
        .flat_map(|bindings| bindings.iter())
        .filter(|(_, cmd)| **cmd == command)
        .map(|(key, _)| key.to_string())
        .sorted()
        .dedup()
        .join(", ");
        if keys.is_empty() {
            "unbound".to_owned()
        } else {
            keys
        }
    };
    vec![
        Line::raw(format!(
            "{}: toggle contextual shortcut bar",
            keys(Command::ToggleShortcutBar)
        )),
        Line::raw(format!(
            "{}: hide/silence current conversation | {}: hidden conversations",
            keys(Command::HideChannel),
            keys(Command::ShowHiddenChannels)
        )),
        Line::raw("Hidden list: Up/Down select | Enter restore | Esc close"),
        Line::raw(format!(
            "{}: toggle local favorite [*]; favorites stay above the separator",
            keys(Command::ToggleFavorite)
        )),
        Line::raw(format!(
            "{}: forward highlighted message without source attribution",
            keys(Command::ForwardMessage)
        )),
        Line::raw("Forward: Up/Down choose visible recipient | Enter sends | Esc cancels"),
        Line::raw(format!(
            "{}: keyboard text selection",
            keys(Command::SelectText)
        )),
        Line::raw(
            "Copy text: arrows move | Space starts/clears selection | Enter copies | Esc cancels",
        ),
        Line::raw(
            "Copy text: Home/End line bounds | PgUp/PgDown scroll | new messages do not shift selection",
        ),
        Line::raw("PgDown on newest message: unselect and stop replying. Esc also unselects."),
        Line::default(),
        Line::styled(
            "Local aliases",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(format!(
            "{}: edit current channel alias",
            keys(Command::EditChannelAlias)
        )),
        Line::raw(format!(
            "{}: edit selected author / direct contact alias",
            keys(Command::EditPersonAlias)
        )),
        Line::raw("Enter: save | Esc: cancel | Empty: remove alias"),
        Line::default(),
        Line::raw(format!(
            "{}: search current conversation (/ is literal inside a draft)",
            keys(Command::SearchMessages)
        )),
        Line::raw(format!(
            "{}: oldest | {}: newest/follow",
            keys(Command::JumpOldest),
            keys(Command::JumpNewest)
        )),
        Line::raw("Search: Up/Down or F3/Shift+F3 | Enter accepts | Esc cancels"),
        Line::raw(format!(
            "Long results show the match; {} opens full text",
            keys(Command::SelectText)
        )),
        Line::default(),
        Line::styled(
            "Image viewer",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(format!(
            "{}: view selected message image",
            keys(Command::ViewImage)
        )),
        Line::raw("Esc / q: return from fullscreen image to conversation"),
        Line::default(),
        Line::styled(
            "Files and screenshots",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(format!(
            "{}: choose attachment; {}: capture screenshot",
            keys(Command::AttachFiles),
            keys(Command::CaptureScreenshot)
        )),
        Line::raw("Picker Enter: add only. Chat Enter: send. Esc: remove attachments."),
        Line::raw("Picker Space: mark/unmark file (up to 10); counter shows marked files."),
        Line::raw("Picker Up/Down, PgUp/PgDown, Home/End: highlight file."),
        Line::raw("Picker Left/Backspace: parent; Right/Enter: open directory."),
        Line::raw("Picker .: hidden files; t: name / modified newest first."),
        Line::raw("Picker /: type/paste path; ~: home; p: ~/Pictures; s: screenshot directory."),
        Line::raw("Picker Alt+P: highlighted image fullscreen; Esc/q: return to picker."),
        Line::raw("Picker Alt+O: highlighted file in default MIME application."),
        Line::raw("Picker Esc: cancel; path entry Esc: return to file list."),
        Line::default(),
    ]
}

fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
    let mut lines = quick_help(app);
    lines.extend(help_commands());
    lines.extend(help_indicators());
    lines.extend(bindings(app));
    let block = Block::bordered().title("Available commands and configured shortcuts");
    let inner = block.inner(area);
    let lines: Vec<_> = lines
        .into_iter()
        .flat_map(|line| {
            let style = line.style;
            textwrap::wrap(&line.to_string(), inner.width.max(1) as usize)
                .into_iter()
                .map(|line| Line::styled(line.into_owned(), style))
                .collect::<Vec<_>>()
        })
        .collect();
    app.help_scroll_max = if inner.width == 0 || inner.height == 0 {
        0
    } else {
        lines
            .len()
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16
    };
    let paragraph = Paragraph::new(Text::from(lines));
    app.help_scroll.0 = app.help_scroll.0.min(app.help_scroll_max);
    app.help_scroll.1 = 0;
    f.render_widget(paragraph.block(block).scroll(app.help_scroll), area);
}

fn displayed_quote(names: &NameResolver, quote: &Message) -> Option<String> {
    let (name, _) = names.resolve(quote.from_id);
    let text = format!("({}) {}", name, quote.message.as_ref()?);
    Some(replace_mentions(quote, names, text))
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use crate::data::{AssociatedValue, BodyRange, ChannelId};
    use crate::signal::Attachment;

    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn favorites_separator_mouse_scroll_and_bindings() {
        use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::{Terminal, backend::TestBackend};
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let favorite = app.selected_channel_id().unwrap();
        app.on_key(KeyCode::F(5).into()).await.unwrap();
        let mut other = app.channel(favorite).unwrap().clone();
        other.id = ChannelId::User(Uuid::new_v4());
        other.name = "Other".into();
        let regular = other.id;
        app.store_channel(other);
        let area = Rect::new(0, 0, 24, 8);
        let mut terminal = Terminal::new(TestBackend::new(24, 8)).unwrap();
        terminal.draw(|f| draw_channels(f, &mut app, area)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(1, 2)].symbol(), "─");
        assert!(!buffer[(1, 2)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buffer[(1, 3)].symbol(), "O");
        let mouse = |row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(mouse(2), Rect::new(0, 0, 72, 8));
        assert_eq!(app.selected_channel_id(), Some(favorite));
        app.on_mouse(mouse(3), Rect::new(0, 0, 72, 8));
        assert_eq!(app.selected_channel_id(), Some(regular));
        app.select_previous_channel();
        assert_eq!(app.selected_channel_id(), Some(favorite));
        app.select_next_channel();
        assert_eq!(app.selected_channel_id(), Some(regular));
        *app.channels.state.offset_mut() = 1;
        terminal.draw(|f| draw_channels(f, &mut app, area)).unwrap();
        assert_eq!(app.channel_index_at_row(0), Some(1));
        assert_eq!(terminal.backend().buffer()[(1, 1)].symbol(), "O");
        let bindings = toml::from_str("[normal]\nF10 = \"toggle_favorite\"\n").unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&bindings, false).unwrap();
        app.on_key(KeyCode::F(10).into()).await.unwrap();
        assert_eq!(app.favorite_channel_count(), 2);
        *app.channels.state.offset_mut() = 0;
        terminal.draw(|f| draw_channels(f, &mut app, area)).unwrap();
        assert_eq!(app.channel_index_at_row(1), Some(1));
        assert!((1..7).all(|row| terminal.backend().buffer()[(1, row)].symbol() != "─"));
        app.on_key(KeyCode::F(10).into()).await.unwrap();
        app.channels.state.select(Some(0));
        app.toggle_favorite().unwrap();
        assert_eq!(app.favorite_channel_count(), 0);
        terminal.draw(|f| draw_channels(f, &mut app, area)).unwrap();
        assert!((1..7).all(|row| terminal.backend().buffer()[(1, row)].symbol() != "─"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_alias_renders_in_narrow_channel_list_and_editor() {
        use crate::storage::AliasTarget;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let id = app.selected_channel_id().unwrap();
        app.set_local_alias(
            AliasTarget::Channel(id),
            "Żółw 🐢 bardzo długa nazwa rozmowy",
        )
        .unwrap();
        // A narrow pane forces clipping of the displayed Unicode alias with an unread suffix.
        let mut terminal = Terminal::new(TestBackend::new(48, 18)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        app.on_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE))
            .await
            .unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(rendered.contains("Local channel alias"));
        assert!(rendered.contains("Enter"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn help_scroll_stops_at_bottom_and_clamps_after_resize() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.toggle_help();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        for _ in 0..1000 {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
                .await
                .unwrap();
        }
        assert!(app.help_scroll.0 < 250, "scroll escaped help content");
        let bottom = app.help_scroll.0;
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .await
            .unwrap();
        assert_eq!(app.help_scroll.0, bottom);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        terminal.backend_mut().resize(100, 300);
        terminal.autoresize().unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert_eq!(app.help_scroll.0, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn help_shows_alias_instructions_near_top_and_mouse_does_not_change_chat() {
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        use ratatui::{Terminal, backend::TestBackend};
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let channel = app.selected_channel_id();
        app.toggle_help();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("F2"));
        assert!(rendered.contains("F3"));
        assert!(rendered.contains("Empty"));
        app.on_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 50,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, 100, 20),
        );
        assert_eq!(app.help_scroll.0, 1);
        assert_eq!(app.selected_channel_id(), channel);
        assert!(
            app.positions
                .get(&channel.unwrap())
                .and_then(|p| p.selected)
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn image_preview_is_opt_in_fits_chat_after_resize_and_hides_deleted_images() {
        use ratatui::{Terminal, backend::TestBackend};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.data");
        image::RgbaImage::from_pixel(320, 160, image::Rgba([250, 10, 20, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let id = app.selected_channel_id().unwrap();
        let mut message = app.storage.message(MessageId::new(id, 0)).unwrap();
        message.from_id = id.user().unwrap();
        message.message = Some("Photo caption ".repeat(50));
        // MIME and extension intentionally do not claim this is an image.
        message.attachments = vec![crate::signal::Attachment {
            id: "photo".into(),
            content_type: "text/plain".into(),
            filename: path,
            size: 0,
        }];
        app.store_message(id, message.clone());
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "▀")
        );
        app.config.show_image_previews = true;
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                terminal.draw(|f| draw(f, &mut app)).unwrap();
                if terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|cell| cell.symbol() == "▀")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("enabled preview never appeared");
        for (idx, cell) in terminal.backend().buffer().content.iter().enumerate() {
            if cell.symbol() == "▀" {
                assert!((21..79).contains(&(idx % 80)));
                assert!((1..16).contains(&(idx / 80)));
            }
        }
        terminal.backend_mut().resize(40, 12);
        terminal.autoresize().unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "▀")
        );
        message.deleted = true;
        app.store_message(id, message);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "▀")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn alias_help_respects_custom_and_disabled_bindings() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let config = toml::from_str("[normal]\nF8 = \"edit_channel_alias\"\n").unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&config, false).unwrap();
        let text = quick_help(&app)
            .into_iter()
            .map(|line| line.to_string())
            .join("\n");
        assert!(text.contains("F8"));
        assert!(text.contains("unbound"));
        assert!(!text.contains("F2"));
        assert!(!text.contains("F3: edit selected author"));
        assert!(text.contains("unbound: edit selected author"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn alias_help_does_not_advertise_bindings_limited_to_other_windows() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let config = toml::from_str("[help]\nF9 = \"edit_channel_alias\"\n").unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&config, false).unwrap();
        let text = quick_help(&app)
            .into_iter()
            .map(|line| line.to_string())
            .join("\n");
        assert!(!text.contains("F9"));
        assert!(text.contains("unbound"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn help_covers_session_picker_controls_and_aliases() {
        let (app, _, _) = crate::app::tests::test_app().await;
        let text = quick_help(&app)
            .into_iter()
            .map(|line| line.to_string())
            .join("\n");
        for expected in [
            "F2",
            "F3",
            "Space",
            "Alt+P",
            "Alt+O",
            "PgUp",
            "Home/End",
            "Backspace",
            "Picker .",
            "t:",
            "~:",
            "p:",
            "s:",
            "capture screenshot",
            "Esc",
        ] {
            assert!(text.contains(expected), "Missing help: {expected}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hidden_shortcuts_and_footer_reserve_bottom_row_and_modal_paste() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.input.put_char('x');
        app.on_key(key(KeyCode::F(4))).await.unwrap();
        assert!(app.shortcut_bar_visible);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        for x in 0..100 {
            assert_eq!(
                terminal.backend().buffer()[(x, 23)].bg,
                Color::Rgb(211, 211, 211)
            );
        }
        app.on_key(key(KeyCode::F(6))).await.unwrap();
        assert!(app.channels.items.is_empty());
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.on_key(key(KeyCode::F(7))).await.unwrap();
        assert!(app.hidden_dialog.is_some());
        app.on_paste("must not enter composer");
        assert!(app.input.data.is_empty());
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert_eq!(app.input.data, "x");
        app.on_key(key(KeyCode::F(4))).await.unwrap();
        assert!(!app.shortcut_bar_visible);
        terminal.backend_mut().resize(1, 1);
        terminal.autoresize().unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn received_order_renderer_uses_display_key_for_selection_anchor() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.config.message_order = crate::config::MessageOrder::Received;
        app.storage.set_message_order(app.config.message_order);
        let channel = app.selected_channel_id().unwrap();
        let sender = channel.user().unwrap();
        for (timestamp, text) in [(9000, "first skewed clock"), (100, "later despite clock")] {
            app.store_message(channel, Message::text(sender, timestamp, text.into()));
        }
        app.on_channel_changed();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.on_pgup();
        assert_eq!(app.positions[&channel].selected, Some(100));
        app.on_pgup();
        assert_eq!(app.positions[&channel].selected, Some(9000));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.on_pgdn();
        assert_eq!(app.positions[&channel].selected, Some(100));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.on_pgdn();
        assert_eq!(app.positions[&channel].selected, None);
    }

    const USER_ID: Uuid = Uuid::nil();

    // formatting test options
    const PREFIX: &str = "                  ";
    const WIDTH: usize = 60;
    const HEIGHT: usize = 10;

    #[test]
    fn attachment_labels_use_sanitized_filenames_and_preserve_paths() {
        let mut msg = Message::text(USER_ID, 0, String::new());
        let mut attachment = test_attachment();
        attachment.filename = "/private/downloads/photo\nname.jpg".into();
        let original_path = attachment.filename.clone();
        msg.attachments.push(attachment);
        let mut label = String::new();
        add_attachments(&msg, &mut label, Some(false), false);
        assert!(label.ends_with("photo�name.jpg"), "{label:?}");
        assert!(!label.contains("/private/"));
        assert!(!label.contains("file://"));
        assert!(!label.contains('\n'));
        assert_eq!(msg.attachments[0].filename, original_path);
        label.clear();
        add_attachments(&msg, &mut label, None, true);
        assert_eq!(label, "<file:///private/downloads/photo�name.jpg>");
    }

    fn test_attachment() -> Attachment {
        Attachment {
            id: "2022-01-16T11:59:58.405665+00:00".to_string(),
            content_type: "image/jpeg".into(),
            filename: "/tmp/gurk/signal-2022-01-16T11:59:58.405665+00:00.jpg".into(),
            size: 238987,
        }
    }

    fn test_voice_note() -> Attachment {
        Attachment {
            id: "2022-01-16T12:00:00.000000+00:00".to_string(),
            content_type: "audio/aac".into(),
            filename: "/tmp/gurk/signal-2022-01-16T12:00:00.000000+00:00.aac".into(),
            size: 12345,
        }
    }

    fn name_resolver() -> NameResolver<'static> {
        NameResolver::single_user(USER_ID, "boxdot".to_string(), Color::Green)
    }

    fn test_message() -> Message {
        Message {
            from_id: USER_ID,
            message: None,
            arrived_at: 1642334397421,
            local_id: None,
            is_edit_snapshot: false,
            quote: None,
            attachments: Default::default(),
            reactions: Default::default(),
            receipt: Receipt::Sent,
            body_ranges: Default::default(),
            send_failed: Default::default(),
            attachment_downloads: Default::default(),
            edit: Default::default(),
            edited: Default::default(),
            deleted: Default::default(),
            expire_timer: None,
            expires_at: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_render_wraps_unicode_and_keeps_final_oversized_match_visible() {
        use ratatui::{Terminal, backend::TestBackend};
        let (app, _, _) = crate::app::tests::test_app().await;
        let body = format!("{}Żółw ŻÓŁW", "earlier words ".repeat(100));
        let offset = body.rfind("ŻÓŁW").unwrap();
        let msg = Message {
            message: Some(body),
            ..test_message()
        };
        let focus = crate::app::search::SearchFocus {
            channel_id: app.selected_channel_id().unwrap(),
            local_id: msg.storage_id(),
            query: "żółw".to_owned(),
            range: offset..offset + "ŻÓŁW".len(),
            occurrence: 1,
        };
        for (width, height) in [(23, 5), (18, 4), (2, 1)] {
            let item = display_search_message(
                &name_resolver(),
                &msg,
                PREFIX,
                width,
                height,
                ShowReceipt::Never,
                &focus,
            )
            .unwrap();
            assert!(item.height() <= height);
            let mut terminal =
                Terminal::new(TestBackend::new(width as u16, height as u16)).unwrap();
            terminal
                .draw(|frame| frame.render_widget(List::new(vec![item]), frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert!(
                buffer
                    .content
                    .iter()
                    .any(|cell| cell.bg == Color::LightCyan),
                "active match missing in {width}x{height}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn focused_message_preserves_decorations_mentions_and_graphemes() {
        use ratatui::{Terminal, backend::TestBackend};
        let (app, _, _) = crate::app::tests::test_app().await;
        let body = "￼ 👩‍💻 é needle";
        let start = body.find("needle").unwrap();
        let msg = Message {
            message: Some(body.to_owned()),
            quote: Some(Box::new(Message {
                message: Some("quoted needle".to_owned()),
                ..test_message()
            })),
            body_ranges: vec![BodyRange {
                start: 0,
                end: 1,
                value: AssociatedValue::MentionUuid(USER_ID),
            }],
            attachments: vec![test_attachment()],
            reactions: vec![(USER_ID, "👍".to_owned())],
            edited: true,
            ..test_message()
        };
        let focus = crate::app::search::SearchFocus {
            channel_id: app.selected_channel_id().unwrap(),
            local_id: msg.storage_id(),
            query: "needle".to_owned(),
            range: start..body.len(),
            occurrence: 0,
        };
        let item = display_search_message(
            &name_resolver(),
            &msg,
            PREFIX,
            120,
            15,
            ShowReceipt::Never,
            &focus,
        )
        .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(120, 15)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(List::new(vec![item]), frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        for expected in [
            "@boxdot",
            "quoted needle",
            "👩‍💻",
            "é",
            "[edited]",
            "👍",
            "signal-2022",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert_eq!(
            buffer
                .content
                .iter()
                .filter(|cell| cell.bg == Color::LightCyan)
                .count(),
            "needle".len()
        );
        assert!(buffer.content.iter().any(|cell| cell.symbol() == "👩‍💻"));
    }

    #[test]
    fn test_display_attachment_only_message() {
        let names = name_resolver();
        let msg = Message {
            attachments: vec![test_attachment()],
            ..test_message()
        };
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("<file:///tmp/gurk/signal-2022-01-"),
            ]),
            Line::from(vec![Span::raw(
                "                  16T11:59:58.405665+00:00.jpg>",
            )]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_text_and_attachment_message() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            attachments: vec![test_attachment()],
            ..test_message()
        };
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("Hello, World!"),
            ]),
            Line::from(vec![Span::raw(
                "                  <file:///tmp/gurk/signal-2022-01-",
            )]),
            Line::from(vec![Span::raw(
                "                  16T11:59:58.405665+00:00.jpg>",
            )]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_voice_note_message() {
        let names = name_resolver();
        let msg = Message {
            attachments: vec![test_voice_note()],
            ..test_message()
        };
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("[Voice Note] <file:///"),
            ]),
            Line::from(vec![Span::raw(
                "                  tmp/gurk/signal-2022-01-",
            )]),
            Line::from(vec![Span::raw(
                "                  16T12:00:00.000000+00:00.aac>",
            )]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_sent_receipt() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            receipt: Receipt::Sent,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("○ ", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_received_receipt() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            receipt: Receipt::Delivered,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("◉ ", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_delivered_receipt() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            receipt: Receipt::Read,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_show_receipts_disabled() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            receipt: Receipt::Read,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, false);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_receipts_for_incoming_message() {
        let user_id = Uuid::from_u128(1);
        let names = NameResolver::single_user(user_id, "boxdot".to_string(), Color::Green);
        let msg = Message {
            from_id: user_id,
            message: Some("Hello, World!".into()),
            receipt: Receipt::Read,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("  ", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_mention() {
        let user_id = Uuid::from_u128(1);
        let names = NameResolver::single_user(user_id, "boxdot".to_string(), Color::Green);
        let msg = Message {
            from_id: user_id,
            message: Some("Mention ￼  and even more ￼ . End".into()),
            receipt: Receipt::Read,
            body_ranges: vec![
                BodyRange {
                    start: 8,
                    end: 9,
                    value: AssociatedValue::MentionUuid(user_id),
                },
                BodyRange {
                    start: 25,
                    end: 26,
                    value: AssociatedValue::MentionUuid(user_id),
                },
            ],
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("  ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("Mention @boxdot  and even more"),
            ]),
            Line::from(vec![Span::raw("                  @boxdot . End")]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_long_message_wraps() {
        let names = name_resolver();
        let msg = Message {
            message: Some(
                "This is a very long message that should wrap across multiple lines in the display"
                    .into(),
            ),
            ..test_message()
        };
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            None,
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("This is a very long message that"),
            ]),
            Line::from(vec![Span::raw(
                "                  should wrap across multiple lines in the",
            )]),
            Line::from(vec![Span::raw("                  display")]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_unread_messages_division() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            ..test_message()
        };
        let division = "--new messages--".to_owned();
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            Some(division.clone()),
            false,
            None,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(division),
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("Hello, World!"),
            ]),
        ]));
        assert_eq!(rendered, Some(expected));
    }
}

#[cfg(test)]
mod spelling_render_tests {
    use super::*;
    #[test]
    fn unicode_error_style_survives_inserted_wrap_newlines() {
        let original = "żółw błąd";
        let (wrapped, _, _) = wrap(original, Cursor::default(), 3);
        let start = original.find("błąd").unwrap();
        let text = spelling_text(
            &wrapped,
            original,
            &[crate::spellcheck::Misspelling {
                range: start..original.len(),
                word: "błąd".to_owned(),
            }],
            &[],
        );
        let marked: String = text
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .filter(|span| span.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(marked, "błąd");
    }
}
