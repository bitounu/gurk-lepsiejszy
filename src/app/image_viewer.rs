use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_image::picker::Picker;

use crate::command::Command;
use crate::config::ImageViewerProtocol;
use crate::ui::image_viewer::ImageViewer;

use super::App;

impl App {
    /// Called after raw/alternate screen setup, before the input reader starts.
    pub fn initialize_image_viewer(&mut self) {
        if self.config.image_viewer.protocol != ImageViewerProtocol::Halfblocks {
            self.image_picker = Picker::from_query_stdio().unwrap_or_else(|error| {
                tracing::debug!(%error, "terminal image detection failed; using halfblocks");
                Picker::halfblocks()
            });
        }
    }

    pub(super) fn open_image_viewer(&mut self) {
        if self.is_help() || self.select_channel.is_shown || self.alias_editor.is_some() {
            return;
        }
        let Some(message) = self.selected_message().filter(|message| !message.deleted) else {
            return;
        };
        let paths = message
            .attachments
            .iter()
            .map(|attachment| attachment.filename.clone())
            .collect();
        self.image_viewer = Some(ImageViewer::new(
            paths,
            self.config.image_viewer.clone(),
            self.image_picker.clone(),
            self.event_tx.clone(),
        ));
        self.should_clear = true;
    }

    pub(super) fn close_image_viewer(&mut self) {
        self.editor_preview_requested = None;
        self.image_viewer = None;
        self.should_clear = true;
    }

    pub fn report_editor_error(&mut self, error: String) {
        self.composer_error = Some(format!("Editor failed: {error}"));
    }

    /// Called after the main loop restores the terminal following EDITOR preview.
    pub fn finish_editor_preview(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                if let Some(viewer) = self
                    .image_viewer
                    .as_ref()
                    .filter(|viewer| viewer.attachment_count() > 1)
                {
                    viewer.editor_finished();
                } else {
                    self.close_image_viewer();
                }
            }
            Err(error) => {
                if let Some(viewer) = self.image_viewer.as_ref() {
                    viewer.editor_error(error);
                }
            }
        }
        self.should_clear = true;
    }

    pub(super) fn on_image_viewer_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('q')
                && !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL))
            || matches!(self.event_to_command(&key), Some(Command::ViewImage))
        {
            self.close_image_viewer();
            return;
        }
        if key.modifiers.contains(KeyModifiers::ALT)
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            if let Some(viewer) = self.image_viewer.as_mut() {
                let changed = match key.code {
                    KeyCode::Left => viewer.pan(-1, 0),
                    KeyCode::Right => viewer.pan(1, 0),
                    KeyCode::Up => viewer.pan(0, -1),
                    KeyCode::Down => viewer.pan(0, 1),
                    _ => false,
                };
                self.should_clear |= changed;
            }
            return;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return;
        }
        if let Some(viewer) = self.image_viewer.as_mut() {
            let changed = match key.code {
                KeyCode::Left => viewer.navigate(-1),
                KeyCode::Right => viewer.navigate(1),
                KeyCode::Char('+' | '=') => viewer.change_zoom(true),
                KeyCode::Char('-') => viewer.change_zoom(false),
                _ => false,
            };
            self.should_clear |= changed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::tests::test_app, signal::Attachment, storage::MessageId};
    use ratatui::{Terminal, backend::TestBackend};

    fn text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn alt_arrows_pan_pixels_without_changing_attachment_draft_or_reply_selection() {
        let (mut app, mut events, sent) = test_app().await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gradient.png");
        image::RgbImage::from_fn(100, 100, |x, y| image::Rgb([x as u8, y as u8, 30]))
            .save(&path)
            .unwrap();
        let channel = app.selected_channel_id().unwrap();
        let mut message = app.storage.message(MessageId::new(channel, 0)).unwrap();
        message.attachments = (0..2)
            .map(|n| Attachment {
                id: format!("photo{n}"),
                content_type: "image/png".into(),
                filename: path.clone(),
                size: 0,
            })
            .collect();
        app.store_message(channel, message);
        app.config.image_viewer.protocol = ImageViewerProtocol::Halfblocks;
        app.shortcut_bar_visible = true;
        app.on_key(KeyCode::PageUp.into()).await.unwrap();
        let selected = app.positions[&channel].selected;
        for c in "draft text".chars() {
            app.input.put_char(c);
        }
        let cursor = app.input.cursor.clone();
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT))
            .await
            .unwrap();
        for _ in 0..4 {
            app.on_key(KeyCode::Char('+').into()).await.unwrap();
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        async fn pixels(
            app: &mut App,
            terminal: &mut Terminal<TestBackend>,
            events: &mut tokio::sync::mpsc::UnboundedReceiver<crate::event::Event>,
        ) -> Vec<ratatui::style::Color> {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
                    while let Ok(event) = events.try_recv() {
                        app.handle_event(event).unwrap();
                    }
                    let colors: Vec<_> = terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .filter(|cell| matches!(cell.symbol(), "▀" | "▄"))
                        .map(|cell| cell.fg)
                        .collect();
                    if !colors.is_empty() {
                        break colors;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap()
        }
        let before = pixels(&mut app, &mut terminal, &mut events).await;
        assert!(text(&terminal).contains("Alt+arrows"));
        app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT))
            .await
            .unwrap();
        let after = pixels(&mut app, &mut terminal, &mut events).await;
        assert_ne!(before, after);
        assert!(text(&terminal).contains("1/2"));
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT))
            .await
            .unwrap();
        let after_down = pixels(&mut app, &mut terminal, &mut events).await;
        assert_ne!(after, after_down);
        app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT))
            .await
            .unwrap();
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT))
            .await
            .unwrap();
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert_eq!(app.positions[&channel].selected, selected);
        assert_eq!(app.input.data, "draft text");
        assert_eq!(app.input.cursor, cursor);
        assert!(sent.borrow().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn gallery_keyboard_preserves_composer_and_selection_and_never_sends() {
        let (mut app, mut events, sent) = test_app().await;
        let directory = tempfile::tempdir().unwrap();
        let channel = app.selected_channel_id().unwrap();
        let mut message = app.storage.message(MessageId::new(channel, 0)).unwrap();
        for (name, color) in [
            ("one.unknown", [240, 20, 30]),
            ("two.unknown", [20, 30, 240]),
        ] {
            let path = directory.path().join(name);
            image::RgbImage::from_pixel(80, 40, image::Rgb(color))
                .save_with_format(&path, image::ImageFormat::Png)
                .unwrap();
            message.attachments.push(Attachment {
                id: name.into(),
                content_type: "application/octet-stream".into(),
                filename: path,
                size: 0,
            });
        }
        app.store_message(channel, message);
        app.config.image_viewer.protocol = ImageViewerProtocol::Halfblocks;
        app.on_key(KeyCode::PageUp.into()).await.unwrap();
        let selected = app.positions[&channel].selected;
        for c in "draft żółw".chars() {
            app.input.put_char(c);
        }
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT))
            .await
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                terminal
                    .draw(|frame| crate::ui::draw(frame, &mut app))
                    .unwrap();
                while let Ok(event) = events.try_recv() {
                    app.handle_event(event).unwrap();
                }
                if text(&terminal).contains('▀') {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(text(&terminal).contains("1/2  one.unknown"));
        app.on_key(KeyCode::Char('+').into()).await.unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(text(&terminal).contains("125%"));
        app.on_key(KeyCode::Char('-').into()).await.unwrap();
        app.on_key(KeyCode::Right.into()).await.unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(text(&terminal).contains("2/2  two.unknown  100%"));
        app.on_key(KeyCode::Left.into()).await.unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(text(&terminal).contains("1/2  one.unknown"));
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert!(app.image_viewer.is_none());
        assert_eq!(app.positions[&channel].selected, selected);
        assert_eq!(app.input.data, "draft żółw");
        assert!(app.editor_preview_requested.is_none());
        assert!(sent.borrow().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unknown_attachment_requests_editor_and_editor_failure_stays_visible_without_retry() {
        let (mut app, mut events, sent) = test_app().await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.png");
        std::fs::write(&path, b"plain text despite extension and claimed MIME").unwrap();
        let channel = app.selected_channel_id().unwrap();
        let mut message = app.storage.message(MessageId::new(channel, 0)).unwrap();
        message.attachments.push(Attachment {
            id: "document".into(),
            content_type: "image/png".into(),
            filename: path.clone(),
            size: 0,
        });
        message.from_id = uuid::Uuid::new_v4();
        message.arrived_at = 1;
        message.local_id = None;
        app.store_message(channel, message);
        app.on_key(KeyCode::PageUp.into()).await.unwrap();
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT))
            .await
            .unwrap();
        assert!(
            app.composer_error.is_none(),
            "previewing an incoming file must not validate message editing"
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while app.editor_preview_requested.is_none() {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(
            app.editor_preview_requested.take(),
            Some(path.canonicalize().unwrap())
        );
        app.finish_editor_preview(Err("Set EDITOR".into()));
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(text(&terminal).contains("Set EDITOR"));
        assert!(app.editor_preview_requested.is_none());
        app.finish_editor_preview(Ok(()));
        assert!(app.image_viewer.is_none());
        assert!(sent.borrow().is_empty());
    }
}
