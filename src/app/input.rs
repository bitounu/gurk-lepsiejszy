use std::borrow::Cow;
use std::io::Cursor;
use std::path::Path;

use anyhow::Context as _;
use arboard::ImageData;
use chrono::{DateTime, Local, TimeZone};
use crokey::Combiner;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use image::codecs::png::PngEncoder;
use image::{ImageBuffer, ImageEncoder, Rgba};
use presage::libsignal_service::sender::AttachmentSpec;
use tracing::{error, info};

use crate::command::{
    Command, DirectionVertical, MoveAmountText, MoveAmountVisual, MoveDirection, Widget, WindowMode,
};
use crate::data::{ChannelId, Message};
use crate::storage::MessageId;
use crate::util::URL_REGEX;

use super::{App, HandleReactionOptions, open_file, open_url, to_emoji};

impl App {
    /// Dispatch mouse input only when no alias modal owns the interaction.
    pub fn on_mouse(&mut self, event: MouseEvent, area: ratatui::layout::Rect) {
        if self.spelling_dialog.is_some()
            || self.message_search.is_some()
            || self.send_job.is_some()
            || self.text_selection.is_some()
            || self.file_picker.is_some()
            || self.image_viewer.is_some()
            || self.forward_dialog.is_some()
            || self.hidden_dialog.is_some()
            || self.is_alias_editor_shown()
        {
            return;
        }
        if self.shortcut_bar_visible && event.row >= area.y + area.height.saturating_sub(1) {
            return;
        }
        if self.is_help() {
            match event.kind {
                MouseEventKind::ScrollDown => {
                    self.help_scroll.0 = self
                        .help_scroll
                        .0
                        .saturating_add(1)
                        .min(self.help_scroll_max)
                }
                MouseEventKind::ScrollUp => {
                    self.help_scroll.0 = self.help_scroll.0.saturating_sub(1)
                }
                _ => {}
            }
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let col = event.column;
                let row = event.row;
                if let Some(channel_idx) = crate::ui::coords_within_channels_view(area, col, row)
                    .map(|(_, row)| row as usize)
                    .filter(|&idx| idx < self.channels.items.len())
                {
                    let old = self.selected_channel_id();
                    self.channels.state.select(Some(channel_idx));
                    let new = self.selected_channel_id();
                    self.swap_channel_draft(old, new);
                    self.on_channel_changed();
                    self.reset_unread_messages();
                }
            }
            MouseEventKind::ScrollUp => {
                if event.column < area.width / crate::ui::CHANNEL_VIEW_RATIO as u16 {
                    self.select_previous_channel()
                } else {
                    self.on_pgup()
                }
            }
            MouseEventKind::ScrollDown => {
                if event.column < area.width / crate::ui::CHANNEL_VIEW_RATIO as u16 {
                    self.select_next_channel()
                } else {
                    self.on_pgdn()
                }
            }
            _ => {}
        }
    }

    async fn on_command(&mut self, command: Command) -> anyhow::Result<()> {
        match command {
            Command::SearchMessages => self.open_message_search(),
            Command::JumpOldest => self.jump_conversation_boundary(false),
            Command::JumpNewest => self.jump_conversation_boundary(true),
            Command::SelectText => self.open_text_selection(),
            Command::ForwardMessage => self.open_forward_dialog(),
            Command::ToggleShortcutBar => {
                self.shortcut_bar_visible = !self.shortcut_bar_visible;
                self.should_clear = true;
            }
            Command::HideChannel => {
                if let Err(error) = self.hide_current_channel() {
                    self.composer_error = Some(format!("Cannot hide conversation: {error:#}"));
                }
            }
            Command::ShowHiddenChannels => self.open_hidden_channels(),
            Command::Help => self.toggle_help(),
            Command::MoveText(MoveDirection::Previous, MoveAmountText::Word) => {
                self.get_input().move_back_word()
            }
            Command::MoveText(MoveDirection::Previous, MoveAmountText::Character) => {
                self.get_input().on_left()
            }
            Command::MoveText(MoveDirection::Previous, MoveAmountText::Line) => {
                self.get_input().move_line_up()
            }
            Command::MoveText(MoveDirection::Next, MoveAmountText::Word) => {
                self.get_input().move_forward_word()
            }
            Command::MoveText(MoveDirection::Next, MoveAmountText::Character) => {
                self.get_input().on_right()
            }
            Command::MoveText(MoveDirection::Next, MoveAmountText::Line) => {
                self.get_input().move_line_down()
            }
            Command::SelectMessage(MoveDirection::Previous, MoveAmountVisual::Entry) => {
                self.on_pgup()
            }
            Command::SelectMessage(MoveDirection::Next, MoveAmountVisual::Entry) => self.on_pgdn(),
            Command::KillBackwardLine => self.get_input().on_delete_line(),
            Command::KillWord => self.get_input().on_delete_word(),
            Command::CopyMessage(_) => self.copy_selection(),
            Command::KillLine => self.get_input().on_delete_suffix(),
            Command::SelectChannel(MoveDirection::Previous) => self.select_previous_channel(),
            Command::SelectChannel(MoveDirection::Next) => self.select_next_channel(),
            Command::SelectChannelModal(MoveDirection::Previous) => self.select_channel_prev(),
            Command::SelectChannelModal(MoveDirection::Next) => self.select_channel_next(),
            Command::KillWholeLine => self.get_input().on_delete_line(),
            Command::BeginningOfLine => self.get_input().on_home(),
            Command::EndOfLine => self.get_input().on_end(),
            Command::CheckSpelling => self.open_spelling().await,
            Command::EditMessage => {
                self.start_editing();
            }
            // Command::ReplyMessage => unimplemented!("{command:?}"),
            Command::DeleteMessage => {
                self.delete_selected_message();
            }
            Command::ToggleChannelModal => {
                if !self.select_channel.is_shown {
                    let items = self
                        .channels
                        .items
                        .iter()
                        .map(|channel| crate::channels::ItemData {
                            channel_id: channel.id,
                            name: self.channel_name(channel).into_owned(),
                        })
                        .collect();
                    self.select_channel.reset(&*self.storage, items);
                }
                self.select_channel.is_shown = !self.select_channel.is_shown;
            }
            Command::ToggleMultiline => {
                self.is_multiline_input = !self.is_multiline_input;
            }
            Command::React(reaction) => {
                if let Some(channel_id) = self.selected_channel_id() {
                    self.add_reaction(channel_id, reaction).await;
                }
            }
            Command::OpenUrl => {
                self.try_open_url();
            }
            Command::AttachFiles => self.open_attachment_picker(),
            Command::CaptureScreenshot => self.start_screenshot(),
            Command::ViewImage => self.open_image_viewer(),
            Command::OpenFile => {
                self.try_open_file();
            }
            Command::DeleteCharacter(MoveDirection::Previous) => {
                self.get_input().on_backspace();
            }
            Command::DeleteCharacter(MoveDirection::Next) => {
                self.get_input().on_delete();
            }
            Command::Quit => {
                self.should_quit = true;
            }
            Command::Redraw => {
                self.should_clear = true;
            }
            Command::Scroll(Widget::Help, DirectionVertical::Up, MoveAmountVisual::Entry) => {
                if self.help_scroll.0 >= 1 {
                    self.help_scroll.0 -= 1
                }
            }
            Command::Scroll(Widget::Help, DirectionVertical::Down, MoveAmountVisual::Entry) => {
                self.help_scroll.0 = self
                    .help_scroll
                    .0
                    .saturating_add(1)
                    .min(self.help_scroll_max)
            }
            Command::EditChannelAlias => self.edit_channel_alias(),
            Command::EditPersonAlias => self.edit_person_alias(),
            Command::ToggleMuteChannel => self.toggle_mute_channel(),
            Command::ToggleChannelList => self.toggle_channel_list(),
            Command::OpenEditor => {
                self.open_editor_requested = true;
            }
            Command::NoOp => {}
        }
        Ok(())
    }

    pub async fn on_key(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        if matches!(
            self.event_to_command(&key),
            Some(Command::ToggleShortcutBar)
        ) {
            self.shortcut_bar_visible = !self.shortcut_bar_visible;
            self.should_clear = true;
            return Ok(());
        }
        if self.spelling_dialog.is_some() {
            self.on_spelling_key(key).await;
            return Ok(());
        }
        if self.text_selection.is_some() {
            self.on_text_selection_key(key);
            return Ok(());
        }
        if self.send_job.is_some() {
            if key.code == KeyCode::Esc {
                self.send_job = None;
                self.composer_error = None;
            }
            return Ok(());
        }
        if self.image_viewer.is_some() {
            self.on_image_viewer_key(key);
            return Ok(());
        }
        if self.file_picker.is_some() {
            self.on_attachment_picker_key(key);
            return Ok(());
        }
        if self.forward_dialog.is_some() {
            self.on_forward_key(key);
            return Ok(());
        }
        if self.hidden_dialog.is_some() {
            self.on_hidden_channels_key(key)?;
            return Ok(());
        }
        if self.alias_editor.is_some() {
            self.on_alias_editor_key(key);
            return Ok(());
        }
        if self.message_search.is_some() {
            self.on_message_search_key(key);
            return Ok(());
        }
        // A visible notice owns Escape before normal composer bindings. Higher
        // priority modals above (and help/channel picker) keep their own Escape.
        if key.code == KeyCode::Esc
            && self.composer_error.is_some()
            && !self.is_help()
            && !self.select_channel.is_shown
        {
            self.composer_error = None;
            self.should_clear = true;
            return Ok(());
        }
        if self.on_mention_key(key) {
            return Ok(());
        }
        // A slash inside a draft remains literal; F8 can search without changing the draft.
        if key.code == KeyCode::Char('/')
            && !key.modifiers.intersects(
                crossterm::event::KeyModifiers::CONTROL
                    | crossterm::event::KeyModifiers::ALT
                    | crossterm::event::KeyModifiers::SUPER,
            )
            && !self.get_input().data.is_empty()
            && matches!(self.event_to_command(&key), Some(Command::SearchMessages))
        {
            self.get_input().put_char('/');
            return Ok(());
        }
        if key.code == KeyCode::Tab
            && !self.select_channel.is_shown
            && self.input.try_complete_emoji()
        {
            return Ok(());
        }
        // Tab not consumed or different key, stop any active completion cycle.
        self.input.completion_partial = None;

        if let Some(cmd) = self.event_to_command(&key) {
            self.on_command(cmd.clone()).await?;
        } else {
            match key.code {
                KeyCode::Char('\r') => {
                    self.get_input().put_char('\n');
                    self.convert_composer_emoticons();
                }
                KeyCode::Enter => {
                    if !self.select_channel.is_shown {
                        if !self.pending_attachments.is_empty() {
                            if let Some(channel_id) = self.selected_channel_id() {
                                self.send_input(channel_id);
                            }
                        } else if self.is_multiline_input {
                            self.get_input().new_line();
                            self.convert_composer_emoticons();
                        } else if !self.input.data.is_empty() {
                            if let Some(channel_id) = self.selected_channel_id() {
                                self.send_input(channel_id);
                            }
                        } else {
                            // input is empty
                            self.try_open_url_or_file();
                        }
                    } else if self.select_channel.is_shown
                        && let Some(channel_id) = self.select_channel.selected_channel_id().copied()
                    {
                        self.select_channel.is_shown = false;
                        let old = self.selected_channel_id();
                        let idx = self
                            .channels
                            .items
                            .iter()
                            .position(|channel| channel.id == channel_id)
                            .context("channel disappeared during channel select popup")?;
                        self.channels.state.select(Some(idx));
                        let new = self.selected_channel_id();
                        self.swap_channel_draft(old, new);
                        self.on_channel_changed();
                    }
                }
                KeyCode::Esc if !self.pending_attachments.is_empty() => {
                    self.cancel_pending_attachments()
                }
                KeyCode::Esc if !self.reset_editing() => {
                    self.reset_message_selection();
                }
                KeyCode::Char(c)
                    if !key.modifiers.intersects(
                        crossterm::event::KeyModifiers::CONTROL
                            | crossterm::event::KeyModifiers::ALT
                            | crossterm::event::KeyModifiers::SUPER,
                    ) || !c.is_ascii() =>
                {
                    self.get_input().put_char(c);
                    if c == ':' {
                        self.get_input().convert_emoji_on_colon();
                    }
                    if c.is_whitespace() {
                        self.convert_composer_emoticons();
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn try_open_url_or_file(&mut self) -> Option<()> {
        self.try_open_url().or_else(|| self.try_open_file())
    }

    /// Tries to open the first url in the selected message.
    ///
    /// Does nothing if no message is selected and no url is contained in the message.
    fn try_open_url(&mut self) -> Option<()> {
        let message = self.selected_message()?;
        open_url(&message, &URL_REGEX)?;
        self.reset_message_selection();
        Some(())
    }

    /// Tries to open the first file attachment in the selected message.
    ///
    /// Does nothing if no message is selected and the message contains no attachments.
    fn try_open_file(&mut self) -> Option<()> {
        let message = self.selected_message()?;
        open_file(&message)?;
        self.reset_message_selection();
        Some(())
    }

    fn selected_message_id(&self) -> Option<MessageId> {
        let channel_id = self.selected_channel_id()?;
        let pos = self.positions.get(&channel_id)?;
        self.window
            .as_ref()?
            .get(pos.selected?)
            .map(|m| MessageId::new(channel_id, m.storage_id()))
    }

    pub(super) fn selected_message(&self) -> Option<Cow<'_, Message>> {
        let channel_id = self.selected_channel_id()?;
        let pos = self.positions.get(&channel_id)?;
        self.window.as_ref()?.get(pos.selected?).map(Cow::Borrowed)
    }

    /// Returns Some(_) reaction if input is a reaction.
    ///
    /// Inner is None, if the reaction should be removed.
    fn take_reaction(&mut self) -> Option<Option<String>> {
        let input_box = self.get_input();
        if input_box.data.is_empty() {
            Some(None)
        } else {
            let emoji = to_emoji(&input_box.data)?.to_string();
            self.take_input();
            Some(Some(emoji))
        }
    }

    pub async fn add_reaction(
        &mut self,
        channel_id: ChannelId,
        reaction: Option<String>,
    ) -> Option<()> {
        let reaction = reaction.or_else(|| self.take_reaction()?);
        let channel = self.channel(channel_id)?;
        let message = self.selected_message()?;
        let remove = reaction.is_none();
        let emoji = reaction.or_else(|| {
            // find emoji which should be removed
            // if no emoji found => there is no reaction from us => nothing to remove
            message.reactions.iter().find_map(|(id, emoji)| {
                if id == &self.signal_manager.user_id() {
                    Some(emoji.clone())
                } else {
                    None
                }
            })
        })?;

        self.signal_manager
            .send_reaction(channel, &message, emoji.clone(), remove);

        let channel_id = channel.id;
        let arrived_at = message.arrived_at;
        let target_author = message.from_id;
        self.handle_reaction(
            channel_id,
            arrived_at,
            target_author,
            self.signal_manager.user_id(),
            emoji,
            HandleReactionOptions::new().remove(remove),
        )
        .await;

        self.reset_unread_messages();
        self.bubble_up_channel(channel_id);
        self.reset_message_selection();

        Some(())
    }

    fn convert_composer_emoticons(&mut self) {
        if self.config.auto_ascii_emoticons && !self.select_channel.is_shown && !self.is_help() {
            self.input.convert_ascii_emoticons();
        }
    }

    fn take_input(&mut self) -> String {
        self.get_input().take()
    }

    pub(super) fn send_input(&mut self, channel_id: ChannelId) {
        if let Some(id) = self.editing
            && let Err(error) = self.check_edit_allowed(id)
        {
            self.composer_error = Some(error);
            return;
        }
        self.convert_composer_emoticons();
        self.input.sync_mentions();
        if !self.pending_attachments.is_empty()
            || crate::mentions::attachment_matches(&self.input.data, &self.input.mentions)
                .next()
                .is_some()
        {
            self.begin_attachment_send(channel_id);
            return;
        }
        let payload = self.input.message_text();
        let input = self.take_input();
        let (input, attachments) = Self::extract_attachments(
            &input,
            Local::now(),
            || self.clipboard.as_mut().map(|c| c.get_image()),
            &payload.mentions,
        );
        self.send_payload(channel_id, payload.caption(input), attachments);
        self.save_composer_or_report();
    }

    pub(super) fn send_payload(
        &mut self,
        channel_id: ChannelId,
        input: crate::mentions::MessageText,
        attachments: Vec<(AttachmentSpec, Vec<u8>)>,
    ) {
        let editing = self.editing.take();
        let edit_timestamp = editing
            .and_then(|id| self.storage.message(id))
            .map(|message| message.arrived_at);
        let mut quote = editing
            .is_none()
            .then(|| self.selected_message().map(Cow::into_owned))
            .flatten();
        let mut text = Some(input);
        let mut edit = edit_timestamp;
        for files in crate::signal::attachment_batch::compatible_batches(attachments) {
            self.queue_payload_part(
                channel_id,
                text.take().unwrap_or_default(),
                files,
                quote.take(),
                edit.take(),
            );
        }
        self.reset_message_selection();
        self.reset_unread_messages();
        self.bubble_up_channel(channel_id);
    }

    /// Queue one protocol-compatible message without reading or resetting composer context.
    pub(super) fn queue_payload_part(
        &mut self,
        channel_id: ChannelId,
        input: crate::mentions::MessageText,
        attachments: Vec<(AttachmentSpec, Vec<u8>)>,
        quote: Option<Message>,
        edit_timestamp: Option<u64>,
    ) {
        let channel = self.channel(channel_id).expect("non-existent channel");
        let (sent_message, response) = self.signal_manager.send_text(
            channel,
            input,
            quote.as_ref(),
            edit_timestamp,
            attachments,
        );

        let (sender, timestamp) = (sent_message.from_id, sent_message.arrived_at);
        if let Some(target) = edit_timestamp {
            self.store_edited_message(channel_id, target, sent_message);
        } else {
            self.store_message(channel_id, sent_message);
        }
        let Some(saved_message) = self
            .storage
            .message_by_protocol(channel_id, sender, timestamp)
        else {
            return;
        };
        let message_id = MessageId::new(channel_id, saved_message.storage_id());
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            if let Ok(result) = response.await {
                tx.send(crate::event::Event::SentTextResult { message_id, result })
                    .expect("event sender gone");
            } else {
                error!(?message_id, "response for sending message was lost");
            }
        });
    }

    pub fn copy_selection(&mut self) {
        if let Some(message) = self.selected_message()
            && let Some(text) = message.message.as_ref()
        {
            let text = text.clone();
            if let Some(clipboard) = self.clipboard.as_mut() {
                if let Err(error) = clipboard.set_text(text) {
                    error!(%error, "failed to copy text to clipboard");
                } else {
                    info!("copied selected text to clipboard");
                }
            }
        }
    }

    /// Returns `true` if editing was reset, otherwise `false`
    fn reset_editing(&mut self) -> bool {
        let is_reset = self.editing.take().is_some();
        if is_reset {
            self.take_input();
        }
        is_reset
    }

    fn check_edit_allowed(&self, id: MessageId) -> Result<(), String> {
        let message = self.storage.message(id).ok_or("Message no longer exists")?;
        let original = if let Some(original_id) = message.edit {
            self.storage
                .message(MessageId::new(id.channel_id, original_id))
                .ok_or("Original message no longer exists")?
        } else {
            message
        };
        if original.from_id != self.user_id || original.deleted {
            return Err("Only your own non-deleted messages can be edited".into());
        }
        if id.channel_id != ChannelId::User(self.user_id)
            && crate::util::utc_now_timestamp_msec().saturating_sub(original.arrived_at)
                >= 24 * 60 * 60 * 1000
        {
            return Err("Messages can only be edited within 24 hours of sending".into());
        }
        if self
            .storage
            .edits(MessageId::new(id.channel_id, original.storage_id()))
            .filter(|revision| !revision.is_edit_snapshot)
            .count()
            >= 10
        {
            return Err("This message has reached the limit of 10 edits".into());
        }
        Ok(())
    }

    fn start_editing(&mut self) -> Option<()> {
        if !self.input.is_empty() || !self.pending_attachments.is_empty() {
            return None;
        }

        let message_id = self.selected_message_id()?;
        let message = self.storage.message(message_id)?;
        if let Err(error) = self.check_edit_allowed(message_id) {
            self.composer_error = Some(error);
            return None;
        }
        self.composer_error = None;

        let target_sent_timestamp = self
            .storage
            .edits(message_id)
            .last()
            .map(|last_edit| last_edit.storage_id())
            .unwrap_or(message.storage_id());
        let message_id = MessageId::new(message_id.channel_id, target_sent_timestamp);
        let text = message.message.clone()?;

        self.editing.replace(message_id);
        let payload =
            crate::mentions::expand(&text, &message.body_ranges, |id| self.name_by_id_cached(id));
        self.input.set_message_text(payload);

        Some(())
    }

    fn delete_selected_message(&mut self) -> Option<()> {
        let message_id = self.selected_message_id()?;
        let message = self.storage.message(message_id)?;

        // Only allow deleting own messages
        if message.from_id != self.user_id {
            return None;
        }

        let channel_id = message_id.channel_id;
        let channel = self.storage.channel(channel_id)?;

        if message.deleted || channel_id == ChannelId::User(self.user_id) {
            // Tombstone or Note to Self: fully remove, sync via deleteForMe
            self.signal_manager.send_delete_for_me(&channel, &message);
            self.storage.remove_message(message_id);
            self.remove_message_from_view(channel_id, message.storage_id());
        } else {
            // Regular channel: delete for everyone (tombstone)
            self.signal_manager
                .send_delete(&channel, message.arrived_at);
            self.storage.delete_message(message_id);
        }

        Some(())
    }

    pub fn event_to_command<'r>(&'r self, event: &KeyEvent) -> Option<&'r Command> {
        let mut combiner = Combiner::default();
        let keys_pressed = combiner.transform(*event)?;
        let modes = if self.alias_editor.is_some() {
            vec![WindowMode::Anywhere, WindowMode::Normal]
        } else if self.is_help() {
            vec![WindowMode::Anywhere, WindowMode::Help]
        } else if self.is_select_channel_shown() {
            vec![WindowMode::Anywhere, WindowMode::ChannelModal]
        } else if self.is_multiline_input {
            let mut modes = vec![WindowMode::Anywhere, WindowMode::Multiline];
            if self.input.is_empty() {
                modes.push(WindowMode::MessageSelected);
            }
            modes.push(WindowMode::Normal);
            modes
        } else if self.input.is_empty() {
            vec![
                WindowMode::Anywhere,
                WindowMode::MessageSelected,
                WindowMode::Normal,
            ]
        } else {
            vec![WindowMode::Anywhere, WindowMode::Normal]
        };
        for mode in modes {
            if let Some(kb) = self.mode_keybindings.get(&mode)
                && let Some(cmd) = kb.get(&keys_pressed)
            {
                // A bare letter remains available for starting a draft when no
                // message is selected. Explicit modifier shortcuts keep their behavior.
                if matches!(cmd, Command::EditMessage)
                    && event.code == KeyCode::Char('e')
                    && event.modifiers.is_empty()
                    && (self.selected_message_id().is_none() || self.editing.is_some())
                {
                    continue;
                }
                return Some(cmd);
            }
        }
        if self.is_help() {
            // Swallow event
            Some(&Command::NoOp)
        } else {
            None
        }
    }

    pub(super) fn extract_attachments<Tz: TimeZone>(
        input: &str,
        at: DateTime<Tz>,
        mut get_clipboard_img: impl FnMut() -> Option<Result<ImageData<'static>, arboard::Error>>,
        mentions: &[crate::mentions::DraftMention],
    ) -> (String, Vec<(AttachmentSpec, Vec<u8>)>)
    where
        Tz::Offset: std::fmt::Display,
    {
        let mut offset = 0;
        let mut clean_input = String::new();

        let attachments = crate::mentions::attachment_matches(input, mentions).filter_map(|m| {
            let path_str = m.as_str().strip_prefix("file://")?;

            clean_input.push_str(input[offset..m.start()].trim_end());
            offset = m.end();

            Some(if path_str.starts_with("clip") {
                // clipboard
                let img = get_clipboard_img()?
                    .inspect_err(|error| error!(%error, "failed to get clipboard image"))
                    .ok()?;

                let width: u32 = img.width.try_into().ok()?;
                let height: u32 = img.height.try_into().ok()?;

                let mut bytes = Vec::new();
                let mut cursor = Cursor::new(&mut bytes);
                let encoder = PngEncoder::new(&mut cursor);

                let png: ImageBuffer<Rgba<_>, _> = ImageBuffer::from_raw(width, height, img.bytes)?;
                let data: Vec<_> = png.into_raw().iter().map(|b| b.swap_bytes()).collect();
                encoder
                    .write_image(
                        &data,
                        img.width as _,
                        img.height as _,
                        image::ExtendedColorType::Rgba8,
                    )
                    .inspect_err(|error| error!(%error, "failed to encode image"))
                    .ok()?;

                let file_name = format!("screenshot-{}.png", at.format("%Y-%m-%dT%H:%M:%S%z"));

                let spec = AttachmentSpec {
                    content_type: "image/png".to_owned(),
                    length: bytes.len(),
                    file_name: Some(file_name),
                    width: Some(width),
                    height: Some(height),
                    ..Default::default()
                };
                (spec, bytes)
            } else {
                // path

                // TODO: Show error to user if the file does not exist. This would prevent not
                // sending the attachment in the end.

                let path = Path::new(path_str);
                let file = std::fs::File::open(path).ok()?;
                if !file.metadata().ok()?.is_file() {
                    return None;
                }
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(
                    &mut std::io::Read::take(file, 100 * 1024 * 1024 + 1),
                    &mut bytes,
                )
                .ok()?;
                if bytes.len() > 100 * 1024 * 1024 {
                    return None;
                }
                let content_type = mime_guess::from_path(path)
                    .first()
                    .map(|mime| mime.essence_str().to_string())
                    .unwrap_or_default();
                let file_name = path.file_name().map(|f| f.to_string_lossy().into());
                let spec = AttachmentSpec {
                    content_type,
                    length: bytes.len(),
                    file_name,
                    ..Default::default()
                };

                (spec, bytes)
            })
        });

        let attachments = attachments.collect();
        clean_input.push_str(&input[offset..]);
        let clean_input = clean_input.trim().to_string();

        (clean_input, attachments)
    }
}

#[cfg(test)]
mod ascii_emoticon_tests {
    use super::*;
    use crate::app::tests::test_app;

    async fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(KeyCode::Char(c).into()).await.unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ascii_emoticons_are_opt_in_and_convert_on_boundary_or_send() {
        let (mut app, _events, sent) = test_app().await;
        type_text(&mut app, "hej :) ").await;
        assert_eq!(app.input.data, "hej :) ");
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        assert_eq!(sent.borrow()[0].message.as_deref(), Some("hej :)"));
        app.config.auto_ascii_emoticons = true;
        type_text(&mut app, "hej :) ").await;
        assert_eq!(app.input.data, "hej 🙂 ");
        app.on_paste(";) :/ :|");
        app.input.on_home();
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        assert_eq!(sent.borrow()[1].message.as_deref(), Some("hej 🙂 😉 😕 😐"));
        assert!(app.input.data.is_empty());
        app.select_channel.is_shown = true;
        type_text(&mut app, ":) ").await;
        assert_eq!(app.select_channel.input.data, ":) ");
        assert!(app.input.data.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ascii_emoticons_convert_attachment_caption_before_preparation() {
        let (mut app, mut events, sent) = test_app().await;
        app.config.auto_ascii_emoticons = true;
        let channel = app.selected_channel_id().unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"attachment").unwrap();
        app.stage_attachment_for(channel, file.path().to_owned())
            .unwrap();
        app.on_paste(":/ :|");
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while sent.borrow().is_empty() {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(sent.borrow()[0].message.as_deref(), Some("😕 😐"));
    }
}
