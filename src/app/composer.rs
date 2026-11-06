//! Durable per-conversation composers and explicit attachment/screenshot staging.
use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Semaphore;
static PREPARATION_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

use anyhow::{Context, Result, bail};
use chrono::Local;
use presage::libsignal_service::sender::AttachmentSpec;

use super::App;
use crate::{
    data::ChannelId,
    event::Event,
    input::Input,
    storage::{Draft, MessageId},
    ui::file_picker::{FilePicker, PickerAction},
};

type Prepared = Vec<(AttachmentSpec, Vec<u8>)>;

pub(crate) struct SendJob {
    id: u64,
    channel: ChannelId,
    text: crate::mentions::MessageText,
    paths: Vec<PathBuf>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for SendJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

fn input_from_draft(draft: &Draft) -> Input {
    let mut input = Input::default();
    input.set_message_text(crate::mentions::MessageText {
        text: draft.text.clone(),
        mentions: draft.mentions.clone(),
    });
    while input.cursor.idx > draft.cursor.min(input.data.len()) {
        input.on_left();
    }
    input
}

impl App {
    fn current_draft(&self) -> Draft {
        let mut input = self.input.clone();
        let payload = input.message_text();
        Draft {
            text: self.input.data.clone(),
            cursor: self.input.cursor.idx,
            attachments: self.pending_attachments.clone(),
            editing: self.editing.map(|id| id.arrived_at),
            mentions: payload.mentions,
        }
    }

    pub(super) fn restore_channel_draft(&mut self, channel: Option<ChannelId>) {
        self.input = Input::default();
        self.pending_attachments.clear();
        self.editing = None;
        if let Some(id) = channel
            && let Some(draft) = self.drafts.remove(&id)
        {
            self.input = input_from_draft(&draft);
            self.pending_attachments = draft.attachments;
            self.editing = draft
                .editing
                .map(|arrived_at| MessageId::new(id, arrived_at));
        }
    }

    pub fn swap_channel_draft(&mut self, old: Option<ChannelId>, new: Option<ChannelId>) {
        if old == new {
            return;
        }
        if let Some(id) = old {
            let draft = self.current_draft();
            if draft.text.is_empty() && draft.attachments.is_empty() {
                self.drafts.remove(&id);
            } else {
                self.drafts.insert(id, draft);
            }
        }
        self.restore_channel_draft(new);
        self.save_composer_or_report();
    }

    /// Save changed composers only, including the active one and selected conversation.
    pub fn persist_composer(&mut self) -> Result<()> {
        let mut drafts = self.drafts.clone();
        let channel = self.selected_channel_id();
        if let Some(id) = channel {
            let draft = self.current_draft();
            if draft.text.is_empty() && draft.attachments.is_empty() {
                drafts.remove(&id);
            } else {
                drafts.insert(id, draft);
            }
        }
        let keys: BTreeSet<_> = drafts
            .keys()
            .chain(self.saved_drafts.keys())
            .copied()
            .collect();
        for id in keys {
            if drafts.get(&id) == self.saved_drafts.get(&id) {
                continue;
            }
            let empty = Draft::default();
            self.storage
                .store_draft(id, drafts.get(&id).unwrap_or(&empty))?;
            if let Some(draft) = drafts.get(&id) {
                self.saved_drafts.insert(id, draft.clone());
            } else {
                self.saved_drafts.remove(&id);
            }
        }
        if self.saved_draft_channel != channel {
            self.storage.store_last_draft_channel(channel)?;
            self.saved_draft_channel = channel;
        }
        Ok(())
    }

    pub fn report_composer_save_error(&mut self, error: anyhow::Error) {
        self.composer_error = Some(format!("Could not save draft: {error}"));
    }

    pub(super) fn save_composer_or_report(&mut self) {
        if let Err(error) = self.persist_composer() {
            self.composer_error = Some(format!("Could not save draft: {error}"));
        }
    }

    pub(crate) fn stage_attachment_for(&mut self, channel: ChannelId, path: PathBuf) -> Result<()> {
        self.stage_attachments_for(channel, vec![path])
    }

    fn stage_attachments_for(&mut self, channel: ChannelId, paths: Vec<PathBuf>) -> Result<()> {
        let paths = paths
            .into_iter()
            .map(|path| {
                let path = path.canonicalize().context("Attachment file is missing")?;
                anyhow::ensure!(path.is_file(), "Select a regular file");
                Ok(path)
            })
            .collect::<Result<BTreeSet<_>>>()?;
        let attachments = if self.selected_channel_id() == Some(channel) {
            anyhow::ensure!(
                self.editing.is_none(),
                "Finish editing the existing message before attaching files"
            );
            &mut self.pending_attachments
        } else {
            let draft = self.drafts.entry(channel).or_default();
            anyhow::ensure!(
                draft.editing.is_none(),
                "Finish editing the existing message before attaching files"
            );
            &mut draft.attachments
        };
        let additional = paths
            .iter()
            .filter(|path| !attachments.contains(path))
            .count();
        anyhow::ensure!(
            attachments.len() + additional <= 10,
            "At most 10 files per message ({} already attached)",
            attachments.len()
        );
        for path in paths {
            if !attachments.contains(&path) {
                attachments.push(path);
            }
        }
        self.composer_error = None;
        self.save_composer_or_report();
        Ok(())
    }

    pub(super) fn open_attachment_picker(&mut self) {
        if self.selected_channel_id().is_none() || self.is_help() || self.select_channel.is_shown {
            return;
        }
        if self.editing.is_some() {
            self.composer_error =
                Some("Finish editing the existing message before attaching files".into());
            return;
        }
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut picker = FilePicker::new(home, self.event_tx.clone());
        picker.set_screenshot_dir(self.config.screenshot_dir.as_deref());
        self.file_picker_generation = self.file_picker_generation.wrapping_add(1);
        self.file_picker = Some(picker);
    }

    pub(super) fn on_attachment_picker_key(&mut self, key: crossterm::event::KeyEvent) {
        let action = self.file_picker.as_mut().expect("picker open").on_key(key);
        match action {
            PickerAction::None => {}
            PickerAction::Cancel => self.file_picker = None,
            PickerAction::Preview(path) => {
                let mut config = self.config.image_viewer.clone();
                config.mode = crate::config::ImageViewerMode::Internal;
                self.image_viewer = Some(crate::ui::image_viewer::ImageViewer::new(
                    vec![path],
                    config,
                    self.image_picker.clone(),
                    self.event_tx.clone(),
                ));
                self.should_clear = true;
            }
            PickerAction::OpenExternal(path) => {
                let tx = self.event_tx.clone();
                let generation = self.file_picker_generation;
                tokio::spawn(async move {
                    let result = crate::ui::file_opener::open_file(path).await;
                    let _ = tx.send(Event::FileOpened { generation, result });
                });
            }
            PickerAction::Selected(paths) => {
                if let Some(channel) = self.selected_channel_id() {
                    match self.stage_attachments_for(channel, paths) {
                        Ok(()) => self.file_picker = None,
                        Err(error) => {
                            self.file_picker.as_mut().expect("picker open").error =
                                Some(error.to_string())
                        }
                    }
                }
            }
        }
    }

    pub(super) fn cancel_pending_attachments(&mut self) {
        self.pending_attachments.clear();
        self.composer_error = None;
        self.save_composer_or_report();
    }

    pub(super) fn begin_attachment_send(&mut self, channel: ChannelId) {
        let (text, extra) = match self.extract_outgoing_attachments() {
            Ok(payload) => payload,
            Err(error) => {
                self.composer_error = Some(format!("Attachment not sent: {error:#}"));
                return;
            }
        };
        let payload = self.input.message_text().caption(text);
        let clean = self.config.clean_image_metadata;
        self.next_send_job = self.next_send_job.wrapping_add(1);
        let id = self.next_send_job;
        let paths = self.pending_attachments.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.send_job = Some(SendJob {
            id,
            channel,
            text: payload,
            paths: paths.clone(),
            cancelled: Arc::clone(&cancelled),
        });
        self.composer_error = None;
        self.save_composer_or_report();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            let Ok(_permit) = Arc::clone(&PREPARATION_SLOTS).acquire_owned().await else {
                return;
            };
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let failure_tx = tx.clone();
            let result = tokio::task::spawn_blocking(move || {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let result = prepare_attachments(&paths, false).and_then(|mut files| {
                    files.extend(extra);
                    sanitize_attachments(files, clean)
                });
                if !cancelled.load(Ordering::Relaxed) {
                    let _ = tx.send(Event::AttachmentsPrepared { job_id: id, result });
                }
            })
            .await;
            if let Err(error) = result {
                let _ = failure_tx.send(Event::AttachmentsPrepared {
                    job_id: id,
                    result: Err(anyhow::anyhow!("attachment preparation failed: {error}")),
                });
            }
        });
    }

    pub(super) fn finish_attachment_send(&mut self, id: u64, result: Result<Prepared>) {
        if self.send_job.as_ref().is_none_or(|job| job.id != id) {
            return;
        }
        let job = self.send_job.take().expect("send job exists");
        match result {
            Err(error) => self.composer_error = Some(format!("Attachment not sent: {error}")),
            Ok(files) => {
                if self.selected_channel_id() != Some(job.channel) {
                    self.composer_error =
                        Some("Conversation changed; attachments remain unsent".into());
                    return;
                }
                self.input.take();
                self.pending_attachments
                    .retain(|path| !job.paths.contains(path));
                self.send_payload(job.channel, job.text.clone(), files);
                self.save_composer_or_report();
            }
        }
    }

    pub(super) fn start_screenshot(&mut self) {
        let Some(channel_id) = self.selected_channel_id() else {
            return;
        };
        if self.screenshot_in_progress || self.is_help() || self.select_channel.is_shown {
            return;
        }
        if self.editing.is_some() {
            self.composer_error =
                Some("Finish editing the existing message before capturing an attachment".into());
            return;
        }
        let Some(home) = dirs::home_dir() else {
            self.composer_error = Some("Cannot determine home directory".into());
            return;
        };
        let directory =
            crate::screenshot::resolve_screenshot_dir(self.config.screenshot_dir.as_deref(), &home);
        let command = self.config.screenshot_command.clone();
        self.screenshot_in_progress = true;
        self.composer_error = None;
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            let result = crate::screenshot::capture_screenshot(directory, command).await;
            let _ = tx.send(Event::ScreenshotCaptured { channel_id, result });
        });
    }
}

pub(super) fn prepare_attachments(paths: &[PathBuf], clean: bool) -> Result<Prepared> {
    const MAX_BYTES: u64 = 100 * 1024 * 1024;
    let mut total = 0u64;
    let mut prepared = Vec::new();
    for path in paths {
        let file =
            std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
        let metadata = file.metadata()?;
        anyhow::ensure!(metadata.is_file(), "Attachment is not a regular file");
        anyhow::ensure!(
            metadata.len() <= MAX_BYTES.saturating_sub(total),
            "Attachments exceed 100 MiB per message"
        );
        let mut bytes = Vec::new();
        file.take(MAX_BYTES.saturating_sub(total) + 1)
            .read_to_end(&mut bytes)?;
        total += bytes.len() as u64;
        if total > MAX_BYTES {
            bail!("Attachments exceed 100 MiB per message");
        }
        let mut reader = image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let format = reader.format();
        let dimensions = reader.into_dimensions().ok();
        let guessed = mime_guess::from_path(path)
            .first_raw()
            .unwrap_or("application/octet-stream");
        let content_type = match (format, dimensions) {
            (Some(format), Some(_)) => format.to_mime_type(),
            _ if guessed.starts_with("image/") => "application/octet-stream",
            _ => guessed,
        }
        .to_owned();
        let (width, height) = dimensions.map_or((None, None), |(w, h)| (Some(w), Some(h)));
        let spec = AttachmentSpec {
            content_type,
            length: bytes.len(),
            file_name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            width,
            height,
            ..Default::default()
        };
        prepared.push((spec, bytes));
    }
    sanitize_attachments(prepared, clean)
}

pub(super) fn sanitize_attachments(mut files: Prepared, clean: bool) -> Result<Prepared> {
    anyhow::ensure!(files.len() <= 10, "At most 10 attachments per message");
    anyhow::ensure!(
        files
            .iter()
            .map(|(_, bytes)| bytes.len() as u64)
            .sum::<u64>()
            <= 100 * 1024 * 1024,
        "Attachments exceed 100 MiB per message"
    );
    for (spec, bytes) in &mut files {
        if clean {
            *bytes = crate::image_metadata::clean(bytes)?;
        }
        // Detection uses the actual outgoing bytes, never a filename or claimed MIME type.
        if bytes.starts_with(b"\xff\xd8") {
            spec.content_type = "image/jpeg".into();
        }
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            spec.content_type = "image/png".into();
        }
        spec.length = bytes.len();
    }
    Ok(files)
}

impl App {
    fn extract_outgoing_attachments(&mut self) -> Result<(String, Prepared)> {
        let payload = self.input.message_text();
        let input = payload.text;
        let matches: Vec<_> =
            crate::mentions::attachment_matches(&input, &payload.mentions).collect();
        anyhow::ensure!(
            matches.len() + self.pending_attachments.len() <= 10,
            "At most 10 attachments per message"
        );
        let mut total = 0u64;
        for capture in &matches {
            let path = capture
                .as_str()
                .strip_prefix("file://")
                .context("Invalid attachment path")?;
            if path == "clip" {
                continue;
            }
            let metadata =
                std::fs::metadata(path).with_context(|| format!("Cannot read {path}"))?;
            anyhow::ensure!(metadata.is_file(), "Attachment is not a regular file");
            total = total.saturating_add(metadata.len());
            anyhow::ensure!(
                total <= 100 * 1024 * 1024,
                "Attachments exceed 100 MiB per message"
            );
        }
        let (text, files) = Self::extract_attachments(
            &input,
            Local::now(),
            || {
                self.clipboard
                    .as_mut()
                    .map(|clipboard| clipboard.get_image())
            },
            &payload.mentions,
        );
        anyhow::ensure!(
            files.len() == matches.len(),
            "Could not read every attachment; nothing was sent"
        );
        Ok((text, files))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::tests::test_app, command::get_keybindings, data::ChannelId};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn recent_edit_fixture(app: &mut App) {
        let channel = app.selected_channel_id().unwrap();
        let id = crate::storage::MessageId::new(channel, 0);
        let mut message = app.storage.message(id).unwrap();
        app.storage.remove_message(id);
        message.local_id = None;
        message.arrived_at = crate::util::utc_now_timestamp_msec() - 1000;
        app.store_message(channel, message);
        app.on_channel_changed();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn edit_limits_reject_old_messages_and_the_eleventh_edit() {
        let (mut app, _, _) = test_app().await;
        app.on_pgup();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(app.editing.is_none(), "1970 message must be too old");
        assert!(app.composer_error.as_deref().unwrap().contains("24"));

        let (mut app, _, _) = test_app().await;
        recent_edit_fixture(&mut app);
        let channel = app.selected_channel_id().unwrap();
        let original = app.storage.last_message(channel).unwrap();
        for n in 1..=10 {
            app.storage
                .store_edited_message(
                    channel,
                    original.arrived_at,
                    crate::data::Message::text(
                        app.user_id,
                        original.arrived_at + n,
                        format!("edit {n}"),
                    ),
                )
                .unwrap();
        }
        app.on_pgup();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(app.editing.is_none(), "eleventh edit must be blocked");
        assert!(app.composer_error.as_deref().unwrap().contains("10"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn edit_limit_is_rechecked_before_sending_and_preserves_draft() {
        let (mut app, _, sent) = test_app().await;
        recent_edit_fixture(&mut app);
        let channel = app.selected_channel_id().unwrap();
        let original = app.storage.last_message(channel).unwrap();
        for n in 1..=9 {
            app.storage
                .store_edited_message(
                    channel,
                    original.arrived_at,
                    crate::data::Message::text(
                        app.user_id,
                        original.arrived_at + n,
                        format!("edit {n}"),
                    ),
                )
                .unwrap();
        }
        app.on_pgup();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(
            app.editing.is_some(),
            "ten edits are allowed, original snapshot is not an edit"
        );
        let draft = app.input.data.clone();
        app.storage
            .store_edited_message(
                channel,
                original.arrived_at,
                crate::data::Message::text(
                    app.user_id,
                    original.arrived_at + 10,
                    "from another device".into(),
                ),
            )
            .unwrap();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        assert!(sent.borrow().is_empty());
        assert_eq!(app.input.data, draft);
        assert!(app.editing.is_some());
        assert!(app.composer_error.as_deref().unwrap().contains("10"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn note_to_self_allows_old_edits_but_keeps_count_limit() {
        let (mut app, _, _) = test_app().await;
        let mut channel = app
            .channel(app.selected_channel_id().unwrap())
            .unwrap()
            .clone();
        channel.id = ChannelId::User(app.user_id);
        let id = channel.id;
        app.store_channel(channel);
        app.storage.store_message(
            id,
            &crate::data::Message::text(app.user_id, 1, "old note".into()),
        );
        app.channels
            .state
            .select(app.channels.items.iter().position(|c| c.id == id));
        app.on_channel_changed();
        app.on_pgup();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(app.editing.is_some());
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        for timestamp in 2..=11 {
            app.storage
                .store_edited_message(
                    id,
                    1,
                    crate::data::Message::text(app.user_id, timestamp, "updated note".into()),
                )
                .unwrap();
        }
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(app.editing.is_none());
        assert!(app.composer_error.as_deref().unwrap().contains("10"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plain_e_edits_selected_message_and_stays_literal_in_composer() {
        let (mut app, mut events, sent) = test_app().await;
        recent_edit_fixture(&mut app);
        app.is_multiline_input = false;
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert_eq!(app.input.data, "e");
        app.input.take();
        app.on_pgup();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(app.editing.is_some());
        assert_eq!(app.input.data, "First message");
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert_eq!(app.input.data, "First messagee");
        app.input.take();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert_eq!(app.input.data, "e");
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.editing.is_none());
        assert!(app.input.is_empty());
        assert!(sent.borrow().is_empty());
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(app.editing.is_some());
        app.input.put_char('!');
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        let event = events.recv().await.unwrap();
        app.handle_event(event).unwrap();
        let channel = app.selected_channel_id().unwrap();
        let messages: Vec<_> = app.storage.messages(channel).collect();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message.as_deref(), Some("First message!"));
        assert!(app.editing.is_none());
        assert_eq!(sent.borrow().len(), 1);
        assert!(sent.borrow()[0].quote.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plain_e_can_start_editing_in_multiline_mode() {
        let (mut app, _, _) = test_app().await;
        recent_edit_fixture(&mut app);
        app.is_multiline_input = true;
        app.on_pgup();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(app.editing.is_some());
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert_eq!(app.input.data, "First messagee");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plain_e_cannot_edit_other_authors_and_can_be_disabled() {
        let (mut app, _, sent) = test_app().await;
        let channel = app.selected_channel_id().unwrap();
        let other = crate::data::Message::text(uuid::Uuid::new_v4(), 1, "Other author".into());
        app.store_message(channel, other);
        app.on_pgup();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(app.editing.is_none());
        assert!(app.input.is_empty());
        assert!(sent.borrow().is_empty());
        let config = toml::from_str("[message_selected]\ne = \"\"\n").unwrap();
        app.mode_keybindings = get_keybindings(&config, true).unwrap();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert_eq!(app.input.data, "e");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mixed_attachment_submission_keeps_all_six_files_in_compatible_messages() {
        use crate::signal::test::SignalManagerMock;
        let (mut app, mut events, _) = test_app().await;
        let mock = SignalManagerMock::new();
        let sent_files = mock.sent_files.clone();
        let sent = mock.sent_messages.clone();
        app.signal_manager = Box::new(mock);
        let channel = app.selected_channel_id().unwrap();
        app.on_pgup();
        let files = [
            "image/jpeg",
            "image/jpeg",
            "image/jpeg",
            "application/octet-stream",
            "image/jpeg",
            "image/png",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, mime)| {
            (
                AttachmentSpec {
                    content_type: mime.into(),
                    length: 1,
                    file_name: Some(format!("file{index}")),
                    ..Default::default()
                },
                vec![index as u8],
            )
        })
        .collect();
        app.send_payload(channel, "caption".into(), files);
        for _ in 0..3 {
            app.handle_event(events.recv().await.unwrap()).unwrap();
        }
        let files = sent_files.borrow();
        assert_eq!(files.iter().map(Vec::len).collect::<Vec<_>>(), [3, 1, 2]);
        assert_eq!(
            files
                .iter()
                .flatten()
                .map(|file| file.1.clone().unwrap())
                .collect::<Vec<_>>(),
            (0..6).map(|n| format!("file{n}")).collect::<Vec<_>>()
        );
        assert_eq!(
            files
                .iter()
                .flatten()
                .map(|file| file.2[0])
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        let sent = sent.borrow();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0].message.as_deref(), Some("caption"));
        assert!(sent[0].quote.is_some());
        assert!(
            sent[1..]
                .iter()
                .all(|message| message.message.as_deref() == Some("") && message.quote.is_none())
        );
        assert!(
            sent.windows(2)
                .all(|pair| pair[0].arrived_at < pair[1].arrived_at)
        );
        assert_eq!(app.storage.messages(channel).count(), 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn edit_error_escape_dismisses_without_losing_draft_or_selected_message() {
        use ratatui::{Terminal, backend::TestBackend};
        let (mut app, _, sent) = test_app().await;
        let channel = app.selected_channel_id().unwrap();
        app.store_message(
            channel,
            crate::data::Message::text(uuid::Uuid::new_v4(), 1, "incoming".into()),
        );
        app.on_key(key(KeyCode::PageUp)).await.unwrap();
        app.on_key(key(KeyCode::Char('e'))).await.unwrap();
        assert!(
            app.composer_error
                .as_deref()
                .unwrap()
                .contains("Only your own")
        );
        assert!(app.editing.is_none());
        let selected = app.positions[&channel].selected;
        app.input.put_char('x');
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        // Even a normal Esc remapping cannot make the visible dismiss hint unsafe.
        let bindings = toml::from_str("[normal]\nesc = \"quit\"\n").unwrap();
        app.mode_keybindings = get_keybindings(&bindings, true).unwrap();
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.composer_error.is_none());
        assert!(!app.should_quit);
        assert!(rendered.contains("Notice — Esc: dismiss"));
        assert_eq!(app.positions[&channel].selected, selected);
        assert_eq!(app.input.data, "x");
        assert!(sent.borrow().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn error_escape_keeps_pending_files_until_second_escape() {
        let (mut app, _, sent) = test_app().await;
        let channel = app.selected_channel_id().unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        app.stage_attachment_for(channel, file.path().to_owned())
            .unwrap();
        app.input.put_char('x');
        app.composer_error = Some("Fixture upload failure".into());
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.composer_error.is_none());
        assert_eq!(app.pending_attachments.len(), 1);
        assert_eq!(app.input.data, "x");
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.pending_attachments.is_empty());
        assert_eq!(app.input.data, "x");
        assert!(sent.borrow().is_empty());
    }

    fn png_with_private_text(path: &std::path::Path) -> Vec<u8> {
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
            .save(path)
            .unwrap();
        let mut bytes = std::fs::read(path).unwrap();
        let payload = b"Comment\0private-camera-location";
        let mut chunk = (payload.len() as u32).to_be_bytes().to_vec();
        chunk.extend_from_slice(b"tEXt");
        chunk.extend_from_slice(payload);
        chunk.extend_from_slice(&crc32fast::hash(&chunk[4..]).to_be_bytes());
        let at = bytes.len() - 12;
        bytes.splice(at..at, chunk);
        std::fs::write(path, &bytes).unwrap();
        bytes
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn image_privacy_cleans_picker_and_legacy_uploads_without_modifying_original() {
        use crate::signal::test::SignalManagerMock;
        let (mut app, mut events, _) = test_app().await;
        let mock = SignalManagerMock::new();
        let sent_files = mock.sent_files.clone();
        app.signal_manager = Box::new(mock);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("picture.png");
        let original = png_with_private_text(&path);
        let channel = app.selected_channel_id().unwrap();
        for legacy in [false, true] {
            if legacy {
                for c in format!("file://{}", path.display()).chars() {
                    app.input.put_char(c);
                }
            } else {
                app.stage_attachment_for(channel, path.clone()).unwrap();
            }
            app.on_key(key(KeyCode::Enter)).await.unwrap();
            assert!(app.send_job.is_some());
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while app.send_job.is_some() {
                    app.handle_event(events.recv().await.unwrap()).unwrap();
                }
                // Drain the send-result event before the next iteration.
                app.handle_event(events.recv().await.unwrap()).unwrap();
            })
            .await
            .unwrap();
        }
        let sent = sent_files.borrow();
        assert_eq!(sent.len(), 2);
        for files in sent.iter() {
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].0, "image/png");
            assert!(!files[0].2.windows(7).any(|w| w == b"private"));
            assert_eq!(
                image::load_from_memory(&files[0].2)
                    .unwrap()
                    .to_rgb8()
                    .as_raw(),
                &[1, 2, 3].repeat(4)
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let unchanged = prepare_attachments(&[path], false).unwrap();
        assert_eq!(unchanged[0].1, original);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn broken_image_metadata_prevents_all_uploads_and_keeps_composer() {
        let (mut app, mut events, sent) = test_app().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.jpg");
        std::fs::write(&path, [0xff, 0xd8, 0xff, 0xe1, 0xff, 0xff]).unwrap();
        app.input.put_char('x');
        let channel = app.selected_channel_id().unwrap();
        app.stage_attachment_for(channel, path.clone()).unwrap();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while app.send_job.is_some() {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert!(sent.borrow().is_empty());
        assert_eq!(app.input.data, "x");
        assert_eq!(app.pending_attachments, [path]);
        assert!(
            app.composer_error
                .as_deref()
                .unwrap()
                .contains("Attachment not sent")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn composers_restore_active_person_group_cursor_and_pending_files() {
        let (mut app, _, _) = test_app().await;
        app.is_multiline_input = false;
        let first = app.selected_channel_id().unwrap();
        let mut group = app.channel(first).unwrap().clone();
        group.id = ChannelId::Group([21; 32]);
        group.name = "Group".into();
        let second = group.id;
        app.store_channel(group);
        let file = tempfile::NamedTempFile::new().unwrap();
        app.channels
            .state
            .select(app.channels.items.iter().position(|c| c.id == first));
        for c in "Żółw\nfirst draft".chars() {
            app.input.put_char(c);
        }
        app.input.on_left();
        let cursor = app.input.cursor.idx;
        app.stage_attachment_for(first, file.path().to_owned())
            .unwrap();
        app.channels
            .state
            .select(app.channels.items.iter().position(|c| c.id == second));
        app.swap_channel_draft(Some(first), Some(second));
        for c in "group draft".chars() {
            app.input.put_char(c);
        }
        app.persist_composer().unwrap();
        let (mut restored, _) = App::try_new(app.config, app.signal_manager, app.storage).unwrap();
        assert_eq!(restored.selected_channel_id(), Some(second));
        assert_eq!(restored.input.data, "group draft");
        restored
            .channels
            .state
            .select(restored.channels.items.iter().position(|c| c.id == first));
        restored.swap_channel_draft(Some(second), Some(first));
        assert_eq!(restored.input.data, "Żółw\nfirst draft");
        assert_eq!(restored.input.cursor.idx, cursor);
        assert!(!restored.is_multiline_input);
        assert_eq!(
            restored.pending_attachments,
            vec![file.path().canonicalize().unwrap()]
        );
        restored.input.take();
        restored.pending_attachments.clear();
        restored.persist_composer().unwrap();
        assert!(!restored.storage.drafts().unwrap().contains_key(&first));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn staged_attachment_requires_confirmation_and_escape_preserves_text() {
        let (mut app, mut events, sent) = test_app().await;
        let id = app.selected_channel_id().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file with spaces.txt");
        std::fs::write(&path, b"hello").unwrap();
        app.input.put_char('x');
        app.stage_attachment_for(id, path.clone()).unwrap();
        assert!(sent.borrow().is_empty());
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.pending_attachments.is_empty());
        assert_eq!(app.input.data, "x");
        app.stage_attachment_for(id, path).unwrap();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while sent.borrow().is_empty() {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(sent.borrow().len(), 1);
        assert_eq!(sent.borrow()[0].message.as_deref(), Some("x"));
        assert!(app.input.is_empty());
        assert!(app.pending_attachments.is_empty());
        assert!(!app.storage.drafts().unwrap().contains_key(&id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn changed_missing_attachment_prevents_any_send_and_keeps_draft() {
        let (mut app, mut events, sent) = test_app().await;
        let id = app.selected_channel_id().unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        app.stage_attachment_for(id, file.path().to_owned())
            .unwrap();
        app.input.put_char('x');
        drop(file);
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while app.send_job.is_some() {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert!(sent.borrow().is_empty());
        assert_eq!(app.input.data, "x");
        assert_eq!(app.pending_attachments.len(), 1);
        assert!(app.composer_error.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn picker_shortcut_is_configurable_and_does_not_modify_draft() {
        let (mut app, _, sent) = test_app().await;
        let bindings = toml::from_str("[normal]\nalt-a = \"\"\nF6 = \"attach_files\"\n").unwrap();
        app.mode_keybindings = get_keybindings(&bindings, true).unwrap();
        app.input.put_char('x');
        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT))
            .await
            .unwrap();
        assert!(app.file_picker.is_none());
        app.on_key(key(KeyCode::F(6))).await.unwrap();
        assert_eq!(
            app.file_picker.as_ref().unwrap().directory,
            dirs::home_dir().unwrap()
        );
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.file_picker.is_none());
        assert_eq!(app.input.data, "x");
        assert!(sent.borrow().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn screenshot_completion_is_staged_in_original_conversation_and_never_sent() {
        let (mut app, _, sent) = test_app().await;
        let original = app.selected_channel_id().unwrap();
        let mut other = app.channel(original).unwrap().clone();
        other.id = ChannelId::Group([41; 32]);
        let other_id = other.id;
        app.store_channel(other);
        app.channels
            .state
            .select(app.channels.items.iter().position(|c| c.id == other_id));
        app.swap_channel_draft(Some(original), Some(other_id));
        let file = tempfile::NamedTempFile::new().unwrap();
        app.handle_event(crate::event::Event::ScreenshotCaptured {
            channel_id: original,
            result: Ok(Some(file.path().to_owned())),
        })
        .unwrap();
        assert!(app.pending_attachments.is_empty());
        assert!(sent.borrow().is_empty());
        assert_eq!(
            app.storage.drafts().unwrap()[&original].attachments.len(),
            1
        );
        app.channels
            .state
            .select(app.channels.items.iter().position(|c| c.id == original));
        app.swap_channel_draft(Some(other_id), Some(original));
        assert_eq!(app.pending_attachments.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_preparation_result_cannot_send_and_edit_context_survives_restart() {
        let (mut app, _, sent) = test_app().await;
        recent_edit_fixture(&mut app);
        let id = app.selected_channel_id().unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        app.stage_attachment_for(id, file.path().to_owned())
            .unwrap();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        let old_id = app.send_job.as_ref().unwrap().id;
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        app.handle_event(Event::AttachmentsPrepared {
            job_id: old_id,
            result: Ok(Vec::new()),
        })
        .unwrap();
        assert!(sent.borrow().is_empty());
        assert_eq!(app.pending_attachments.len(), 1);
        app.cancel_pending_attachments();
        app.on_pgup();
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert!(app.editing.is_some());
        app.input.put_char('!');
        let editing = app.editing;
        let text = app.input.data.clone();
        app.persist_composer().unwrap();
        let (mut restored, _) = App::try_new(app.config, app.signal_manager, app.storage).unwrap();
        assert_eq!(restored.editing, editing);
        assert_eq!(restored.input.data, text);
        assert!(
            restored
                .stage_attachment_for(id, file.path().to_owned())
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn picker_enter_stages_image_and_previews_it_without_sending() {
        use ratatui::{Terminal, backend::TestBackend};
        let (mut app, mut events, sent) = test_app().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.data");
        image::RgbImage::from_fn(40, 20, |x, y| {
            image::Rgb([(x * 5) as u8, (y * 10) as u8, 120])
        })
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
        app.file_picker = Some(FilePicker::new(dir.path().to_owned(), app.event_tx.clone()));
        app.handle_event(events.recv().await.unwrap()).unwrap();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.file_picker.is_none());
        assert_eq!(app.pending_attachments.len(), 1);
        assert!(sent.borrow().is_empty());
        assert!(!app.config.show_image_previews);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
                if terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|c| c.symbol() == "▀")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.pending_attachments.is_empty());
        assert!(sent.borrow().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn screenshot_shortcut_saves_png_and_stages_without_sending() {
        use std::os::unix::fs::PermissionsExt;
        let (mut app, mut events, sent) = test_app().await;
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture.png");
        image::RgbImage::from_pixel(8, 4, image::Rgb([1, 2, 3]))
            .save(&fixture)
            .unwrap();
        let script = dir.path().join("capture fixture");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ncp '{}' \"$1\"\n", fixture.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = dir.path().join("Saved screenshots");
        app.config.screenshot_dir = Some(output.clone());
        app.config.screenshot_command = vec![script.display().to_string()];
        app.input.put_char('x');
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT))
            .await
            .unwrap();
        assert!(app.screenshot_in_progress);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while app.screenshot_in_progress {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(app.pending_attachments.len(), 1);
        let path = &app.pending_attachments[0];
        assert!(path.starts_with(output));
        assert!(path.is_file());
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(sent.borrow().is_empty());
        assert_eq!(app.input.data, "x");
    }

    #[test]
    fn preparation_preserves_filename_and_detects_image_by_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("picture with spaces.data");
        image::RgbImage::from_pixel(8, 4, image::Rgb([200, 30, 80]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let prepared = prepare_attachments(&[path], true).unwrap();
        assert_eq!(
            prepared[0].0.file_name.as_deref(),
            Some("picture with spaces.data")
        );
        assert_eq!(prepared[0].0.content_type, "image/png");
        assert_eq!(prepared[0].0.width, Some(8));
        assert_eq!(prepared[0].0.length, prepared[0].1.len());
        assert!(!prepared[0].1.is_empty());
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn batch_limit_and_validation_are_atomic() {
        let (mut app, _, sent) = test_app().await;
        let id = app.selected_channel_id().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..11)
            .map(|n| {
                let path = dir.path().join(format!("{n}.txt"));
                std::fs::write(&path, b"test").unwrap();
                path
            })
            .collect();
        app.stage_attachments_for(id, paths[..9].to_vec()).unwrap();
        let before = app.pending_attachments.clone();
        assert!(app.stage_attachments_for(id, paths[9..].to_vec()).is_err());
        assert_eq!(app.pending_attachments, before);
        assert!(
            app.stage_attachments_for(id, vec![paths[9].clone(), dir.path().join("missing")])
                .is_err()
        );
        assert_eq!(app.pending_attachments, before);
        app.stage_attachments_for(id, vec![paths[0].clone(), paths[9].clone()])
            .unwrap();
        assert_eq!(app.pending_attachments.len(), 10);
        assert!(sent.borrow().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn picker_preview_uses_highlight_and_returns_without_creating_files() {
        use ratatui::{Terminal, backend::TestBackend, style::Color};
        let (mut app, mut events, sent) = test_app().await;
        let dir = tempfile::tempdir().unwrap();
        for (name, color) in [("a.png", [255, 0, 0]), ("b.png", [0, 0, 255])] {
            image::RgbImage::from_pixel(20, 20, image::Rgb(color))
                .save(dir.path().join(name))
                .unwrap();
        }
        app.config.image_viewer.mode = crate::config::ImageViewerMode::External;
        app.config.image_viewer.protocol = crate::config::ImageViewerProtocol::Halfblocks;
        app.config.image_viewer.command = vec!["no-program-should-run".into()];
        app.file_picker = Some(FilePicker::new(dir.path().to_owned(), app.event_tx.clone()));
        app.handle_event(events.recv().await.unwrap()).unwrap();
        app.on_key(key(KeyCode::Char(' '))).await.unwrap();
        app.on_key(key(KeyCode::Down)).await.unwrap();
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT))
            .await
            .unwrap();
        assert!(app.image_viewer.is_some());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
                if terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|c| c.fg == Color::Rgb(0, 0, 255) || c.bg == Color::Rgb(0, 0, 255))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.image_viewer.is_none());
        assert!(app.file_picker.is_some());
        assert!(app.should_clear);
        terminal.clear().unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|c| c.symbol() == "▀")
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.file_picker.is_none());
        assert_eq!(
            app.pending_attachments,
            vec![dir.path().join("a.png").canonicalize().unwrap()]
        );
        assert!(sent.borrow().is_empty());
    }
}
