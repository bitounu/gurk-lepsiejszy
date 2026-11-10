use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use anyhow::Context as _;
use itertools::Itertools;
use regex::Regex;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::data::{Channel, ChannelId, Message, TypingSet};
use crate::event::Event;
use crate::input::Input;
use crate::receipt::ReceiptHandler;
use crate::signal::SignalManager;
use crate::storage::{AliasTarget, Draft, MessageId, Storage};
use crate::util::StatefulList;
use crate::{
    app::channel::ChannelPosition,
    command::{ModeKeybinding, get_keybindings},
};
use crate::{app::window::MessageWindow, channels::SelectChannel};

use presage::proto::data_message::Sticker;

pub(crate) mod aliases;
mod channel;
mod composer;
mod forward;
mod hidden;
mod image_viewer;
mod input;
mod mentions;
mod message;
mod read_sync;
pub(crate) mod search;
mod spelling;
mod text_copy;
mod window;

pub struct App {
    pub(crate) spelling: crate::spellcheck::Spellchecker,
    pub(crate) spelling_dialog: Option<spelling::SpellingDialog>,
    pub(crate) message_search: Option<search::MessageSearch>,
    pub(crate) search_focus: Option<search::SearchFocus>,
    pub config: Config,
    signal_manager: Box<dyn SignalManager>,
    pub storage: Box<dyn Storage>,
    /// List of channels in memory
    ///
    /// Additionally used as in-memory cache. All writes to the storage must update the data in this
    /// list.
    pub channels: StatefulList<Channel>,
    hidden_channels: BTreeSet<ChannelId>,
    hidden_channel_data: BTreeMap<ChannelId, Channel>,
    pub(crate) text_selection: Option<crate::ui::text_selection::TextSelection>,
    pub(crate) forward_dialog: Option<forward::ForwardDialog>,
    forward_generation: u64,
    read_sync: read_sync::ReadSyncQueue,
    pub(crate) hidden_dialog: Option<hidden::HiddenDialog>,
    pub(crate) shortcut_bar_visible: bool,
    pub(crate) positions: BTreeMap<ChannelId, ChannelPosition>,
    pub(crate) window: Option<MessageWindow>,
    pub help_scroll: (u16, u16),
    pub(crate) help_scroll_max: u16,
    pub(crate) image_previews: crate::ui::image_preview::PreviewCache,
    pub(crate) image_viewer: Option<crate::ui::image_viewer::ImageViewer>,
    pub(crate) image_picker: ratatui_image::picker::Picker,
    pub user_id: Uuid,
    pub terminal_focused: bool,
    pub should_quit: bool,
    pub should_clear: bool,
    pub open_editor_requested: bool,
    pub editor_preview_requested: Option<std::path::PathBuf>,
    display_help: bool,
    show_channel_list: bool,
    receipt_handler: ReceiptHandler,
    pub input: Input,
    pub(crate) mention_completion: Option<mentions::MentionCompletion>,
    pub(crate) mention_dismissed: Option<(ChannelId, String, usize)>,
    drafts: BTreeMap<ChannelId, Draft>,
    saved_drafts: BTreeMap<ChannelId, Draft>,
    saved_draft_channel: Option<ChannelId>,
    pub(crate) pending_attachments: Vec<std::path::PathBuf>,
    pub(crate) composer_error: Option<String>,
    pub(crate) file_picker_generation: u64,
    pub(crate) file_picker: Option<crate::ui::file_picker::FilePicker>,
    pub(crate) send_job: Option<composer::SendJob>,
    next_send_job: u64,
    pub(crate) screenshot_in_progress: bool,
    pub is_multiline_input: bool,
    editing: Option<MessageId>,
    pub(crate) select_channel: SelectChannel,
    pub(crate) local_aliases: BTreeMap<AliasTarget, String>,
    pub(crate) alias_editor: Option<aliases::AliasEditor>,
    clipboard: Option<arboard::Clipboard>,
    event_tx: mpsc::UnboundedSender<Event>,
    // It is expensive to hit the signal manager contacts storage, so we cache it
    names_cache: Cell<Option<BTreeMap<Uuid, String>>>,
    /// Users whose profile name is being resolved via Signal server
    resolving_profile_names: HashSet<Uuid>,
    pub mode_keybindings: ModeKeybinding,
    /// When the current channel was selected (for dwell-based timer activation)
    channel_selected_at: std::time::Instant,
    /// Channel whose message timers have been activated (avoids repeated scanning)
    timers_activated_for: Option<ChannelId>,
    /// Points to the next message that will expire
    next_expiring_at: Option<u64>,
    /// The height of the message view in the renderer
    ///
    /// Written by the renderer, used to load enough messages to fill the view
    pub(crate) message_view_height: usize,
}

impl App {
    pub fn try_new(
        config: Config,
        signal_manager: Box<dyn SignalManager>,
        mut storage: Box<dyn Storage>,
    ) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        storage.set_message_order(config.message_order);
        let pending_reads = storage
            .pending_read_sync()
            .context("failed to load private read sync outbox")?;
        let mut read_sync = read_sync::ReadSyncQueue::default();
        for (sender, timestamp) in pending_reads {
            read_sync.queue(sender, timestamp);
        }
        let user_id = signal_manager.user_id();
        let drafts = storage.drafts().context("failed to load message drafts")?;
        let last_draft_channel = storage
            .last_draft_channel()
            .context("failed to load composer context")?;
        let local_aliases = storage.aliases().context("failed to load local aliases")?;

        let hidden_channels = storage
            .hidden_channels()
            .context("failed to load hidden conversations")?;
        let (hidden, visible): (Vec<_>, Vec<_>) = storage
            .channels()
            .into_iter()
            .partition(|channel| hidden_channels.contains(&channel.id));
        let hidden_channel_data = hidden
            .into_iter()
            .map(|channel| (channel.id, channel))
            .collect();

        // build the list of channels
        let mut channels = StatefulList {
            items: visible,
            ..Default::default()
        };
        channels.next();
        if let Some(index) = last_draft_channel
            .and_then(|id| channels.items.iter().position(|channel| channel.id == id))
        {
            channels.state.select(Some(index));
        }

        let clipboard = arboard::Clipboard::new()
            .map_err(|error| warn!(%error, "clipboard disabled"))
            .ok();

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let mode_keybindings = get_keybindings(&config.keybindings, config.default_keybindings)
            .expect("keybinding configuration failed");

        let next_expiring_at = storage.next_expiring_at();

        let spelling = crate::spellcheck::Spellchecker::new(&config.spellcheck);
        let mut app = Self {
            spelling,
            spelling_dialog: None,
            message_search: None,
            search_focus: None,
            shortcut_bar_visible: config.shortcut_bar.enabled,
            hidden_channels,
            hidden_channel_data,
            text_selection: None,
            forward_dialog: None,
            forward_generation: 0,
            read_sync,
            hidden_dialog: None,
            config,
            signal_manager,
            user_id,
            storage,
            channels,
            positions: Default::default(),
            window: None,
            help_scroll: (0, 0),
            help_scroll_max: 0,
            image_previews: Default::default(),
            image_viewer: None,
            image_picker: ratatui_image::picker::Picker::halfblocks(),
            terminal_focused: true,
            should_quit: false,
            should_clear: false,
            open_editor_requested: false,
            editor_preview_requested: None,
            display_help: false,
            show_channel_list: true,
            receipt_handler: ReceiptHandler::new(),
            input: Default::default(),
            mention_completion: None,
            mention_dismissed: None,
            saved_drafts: drafts.clone(),
            drafts,
            saved_draft_channel: last_draft_channel,
            pending_attachments: Vec::new(),
            composer_error: None,
            file_picker_generation: 0,
            file_picker: None,
            send_job: None,
            next_send_job: 0,
            screenshot_in_progress: false,
            is_multiline_input: false,
            editing: None,
            select_channel: Default::default(),
            local_aliases,
            alias_editor: None,
            clipboard,
            event_tx,
            names_cache: Default::default(),
            resolving_profile_names: Default::default(),
            mode_keybindings,
            channel_selected_at: std::time::Instant::now(),
            timers_activated_for: None,
            next_expiring_at,
            message_view_height: 0,
        };

        app.restore_channel_draft(app.selected_channel_id());
        app.on_channel_changed();

        Ok((app, event_rx))
    }

    /// Resolve and cache all user names for the known user channels
    pub async fn populate_names_cache(&self) {
        let mut names_cache = BTreeMap::new();
        for user_id in self.all_channels().filter_map(|channel| channel.id.user()) {
            if let Some(name) = self.resolve_name(user_id).await {
                names_cache.insert(user_id, name);
            }
        }
        let mut cache = self.names_cache.take().unwrap_or_default();
        cache.extend(names_cache);
        self.names_cache.replace(Some(cache));
    }

    pub fn get_input(&mut self) -> &mut Input {
        if let Some(editor) = self.alias_editor.as_mut() {
            &mut editor.input
        } else if self.select_channel.is_shown {
            &mut self.select_channel.input
        } else {
            &mut self.input
        }
    }

    pub fn writing_people(&self, channel: &Channel) -> Option<String> {
        if channel.is_writing() {
            let uuids: Box<dyn Iterator<Item = Uuid>> = match &channel.typing {
                TypingSet::GroupTyping(map) => Box::new(map.keys().copied()),
                TypingSet::SingleTyping(Some(_)) => {
                    Box::new(std::iter::once(channel.user_id().unwrap()))
                }
                TypingSet::SingleTyping(None) => Box::new(std::iter::empty()),
            };
            Some(format!(
                "[{}] writing...",
                uuids.map(|id| self.name_by_id_cached(id)).format(", ")
            ))
        } else {
            None
        }
    }

    /// Returns the name of a user by their ID from the cache without resolving it.
    pub fn name_by_id_cached(&self, id: Uuid) -> String {
        if let Some(alias) = self.local_aliases.get(&AliasTarget::Person(id)) {
            return alias.clone();
        }
        if self.user_id == id {
            // it's me
            return self.config.user.display_name.clone();
        };

        let cache = self.names_cache.take().unwrap_or_default();
        let name = cache
            .get(&id)
            .cloned()
            .or_else(|| self.storage.name(id))
            .unwrap_or_else(|| id.to_string());
        self.names_cache.replace(Some(cache));
        name
    }

    /// Resolves name of a user by their id
    ///
    /// The resolution is done from the following places:
    ///
    /// 1. signal's profile name storage
    /// 2. signal's contacts storage
    /// 3. internal gurk's user name table
    async fn resolve_name(&self, user_id: Uuid) -> Option<String> {
        if let Some(name) = self.signal_manager.profile_name(user_id).await {
            debug!(name, "resolved name as profile name");
            return Some(name);
        }
        if let Some(contact) = self.signal_manager.contact(user_id).await {
            if !contact.name.trim().is_empty() {
                debug!(name = contact.name, "resolved name from contacts");
                return Some(contact.name);
            } else {
                debug!(%user_id, "resolved empty name from contacts, skipping");
            }
        }
        if let Some(name) = self
            .storage
            .name(user_id)
            .filter(|name| !name.trim().is_empty())
        {
            debug!(%name, "resolved name from storage");
            return Some(name);
        }
        None
    }

    // Resolves name of a user by their id
    pub async fn name_by_id(&self, id: Uuid) -> String {
        if self.user_id == id {
            // it's me
            self.config.user.display_name.clone()
        } else {
            self.name_by_id_or_cache(id, |id| self.resolve_name(id))
                .await
        }
    }

    async fn name_by_id_or_cache<F>(&self, id: Uuid, on_miss: impl FnOnce(Uuid) -> F) -> String
    where
        F: Future<Output = Option<String>>,
    {
        let cache = self.names_cache.take().unwrap_or_default();
        let name = cache.get(&id).cloned();
        self.names_cache.replace(Some(cache));

        if let Some(name) = name {
            name
        } else if let Some(name) = on_miss(id).await {
            let mut cache = self.names_cache.take().unwrap_or_default();
            cache.insert(id, name.clone());
            self.names_cache.replace(Some(cache));
            name
        } else {
            id.to_string()
        }
    }

    pub fn channel_name<'a>(&self, channel: &'a Channel) -> Cow<'a, str> {
        if let Some(alias) = self.local_aliases.get(&AliasTarget::Channel(channel.id)) {
            return Cow::Owned(alias.clone());
        }
        if let Some(id) = channel.user_id() {
            let name = self.name_by_id_cached(id);
            if !self.local_aliases.contains_key(&AliasTarget::Person(id)) && name == id.to_string()
            {
                Cow::Borrowed(&channel.name)
            } else {
                Cow::Owned(name)
            }
        } else {
            (&channel.name).into()
        }
    }

    pub fn toggle_help(&mut self) {
        self.display_help = !self.display_help;
    }

    pub fn is_help(&self) -> bool {
        self.display_help
    }

    pub fn is_select_channel_shown(&self) -> bool {
        self.select_channel.is_shown
    }

    pub fn is_channel_list_shown(&self) -> bool {
        self.show_channel_list
    }

    pub fn toggle_channel_list(&mut self) {
        self.show_channel_list = !self.show_channel_list;
    }

    pub fn handle_event(&mut self, event: Event) -> anyhow::Result<()> {
        match event {
            Event::ForwardPrepared {
                generation,
                target,
                result,
            } => self.finish_forward(generation, target, result),
            Event::FilePickerUpdated => {}
            Event::AttachmentsPrepared { job_id, result } => {
                self.finish_attachment_send(job_id, result)
            }
            Event::ScreenshotCaptured { channel_id, result } => {
                self.screenshot_in_progress = false;
                match result {
                    Ok(Some(path)) => {
                        if let Err(error) = self.stage_attachment_for(channel_id, path) {
                            self.composer_error = Some(error.to_string());
                        }
                    }
                    Ok(None) => {}
                    Err(error) => self.composer_error = Some(format!("Screenshot failed: {error}")),
                }
            }
            Event::FileOpened { generation, result } => {
                if generation == self.file_picker_generation
                    && let Some(picker) = self.file_picker.as_mut()
                    && let Err(error) = result
                {
                    picker.set_error(format!("Cannot open file: {error:#}"));
                }
            }
            Event::ImageViewerReady => {
                if self
                    .image_viewer
                    .as_ref()
                    .is_some_and(|viewer| viewer.external_done())
                {
                    self.close_image_viewer();
                } else if let Some(viewer) = self.image_viewer.as_mut() {
                    self.editor_preview_requested = viewer.take_editor_request();
                }
            }
            Event::SentTextResult { message_id, result } => {
                if let Err(error) = result {
                    let mut message = self.storage.message(message_id).context("no message")?;
                    message.send_failed = Some(format!("{error:#}"));
                    self.store_message(message_id.channel_id, message);
                }
            }
            Event::AttachmentSaved {
                message_id,
                attachment,
            } => {
                self.on_attachment_saved(message_id, attachment);
            }
            Event::ProfileNameResolved { user_id, name } => {
                self.resolving_profile_names.remove(&user_id);
                if let Some(name) = name.filter(|name| !name.is_empty()) {
                    self.on_profile_name_resolved(user_id, name);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.editing.is_some()
    }
}

#[derive(Debug, Default)]
pub(super) struct HandleReactionOptions {
    pub(super) remove: bool,
    pub(super) notify: bool,
    pub(super) bell: bool,
}

impl HandleReactionOptions {
    pub(super) fn new() -> Self {
        Default::default()
    }

    pub(super) fn remove(self, remove: bool) -> Self {
        Self { remove, ..self }
    }

    pub(super) fn notify(self, notify: bool) -> Self {
        Self { notify, ..self }
    }

    pub(super) fn bell(self, bell: bool) -> Self {
        Self { bell, ..self }
    }
}

/// Returns an emoji string if `s` is an emoji or if `s` is a GitHub emoji shortcode.
pub fn to_emoji(s: &str) -> Option<&str> {
    let s = s.trim();
    if emojis::get(s).is_some() {
        Some(s)
    } else {
        let s = s.strip_prefix(':')?.strip_suffix(':')?;
        Some(emojis::get_by_shortcode(s)?.as_str())
    }
}

pub(super) fn open_url(message: &Message, url_regex: &Regex) -> Option<()> {
    let text = message.message.as_ref()?;
    let m = url_regex.find(text)?;
    let url = m.as_str();
    let result = if let Some(path) = url.strip_prefix("file://") {
        opener::open(Path::new(path))
    } else {
        opener::open(url)
    };
    if let Err(error) = result {
        error!(url, %error, "failed to open");
    }
    Some(())
}

pub(super) fn open_file(message: &Message) -> Option<()> {
    let attachment = message.attachments.first()?;
    let file: &Path = attachment.filename.as_ref();
    if let Err(error) = opener::open(file) {
        let path = file.display().to_string();
        error!(path, %error, "failed to open");
    }
    Some(())
}

pub(super) fn notification_text_for_attachments(num_attachments: usize) -> Option<String> {
    match num_attachments {
        0 => None,
        1 => Some("<attachment>".into()),
        n => Some(format!("<attachments ({n})>")),
    }
}

pub(super) fn add_emoji_from_sticker(body: &mut Option<String>, sticker: Option<Sticker>) {
    if let Some(Sticker { emoji: Some(e), .. }) = sticker {
        *body = Some(format!("<{e}>"));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use chrono::{DateTime, FixedOffset};

    use arboard::ImageData;

    use crate::data::GroupData;
    use crate::signal::GroupMasterKeyBytes;
    use crate::signal::test::SignalManagerMock;
    use crate::{config::User, storage::SqliteStorage};

    use super::*;

    pub(crate) async fn test_app() -> (
        App,
        mpsc::UnboundedReceiver<Event>,
        Rc<RefCell<Vec<Message>>>,
    ) {
        let url = "sqlite::memory:".parse().unwrap();
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();

        let signal_manager = SignalManagerMock::new();
        let sent_messages = signal_manager.sent_messages.clone();

        let channel_id = ChannelId::User(Uuid::new_v4());
        let channel = Channel {
            id: channel_id,
            name: "test".to_string(),
            group_data: Some(GroupData {
                master_key_bytes: GroupMasterKeyBytes::default(),
                members: vec![signal_manager.user_id()],
                revision: 1,
            }),
            unread_messages: 1,
            muted: false,
            typing: TypingSet::GroupTyping(Default::default()),
            expire_timer: None,
        };
        storage.store_channel(&channel);
        storage.store_message(
            channel_id,
            &Message {
                from_id: signal_manager.user_id(),
                message: Some("First message".to_string()),
                arrived_at: 0,
                local_id: None,
                is_edit_snapshot: false,
                quote: Default::default(),
                attachments: Default::default(),
                reactions: Default::default(),
                receipt: Default::default(),
                body_ranges: Default::default(),
                send_failed: Default::default(),
                attachment_downloads: Default::default(),
                edit: Default::default(),
                edited: Default::default(),
                deleted: Default::default(),
                expire_timer: None,
                expires_at: None,
            },
        );

        let user = User {
            display_name: "Tyler Durden".to_string(),
        };
        let (mut app, events) = App::try_new(
            Config::with_user(user),
            Box::new(signal_manager),
            Box::new(storage),
        )
        .unwrap();
        app.channels.state.select(Some(0));

        (app, events, sent_messages)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_alias_precedence_and_original_name_updates() {
        use crate::storage::AliasTarget;
        let (mut app, _, _) = test_app().await;
        let channel_id = app.selected_channel_id().unwrap();
        let person = channel_id.user().unwrap();
        app.on_profile_name_resolved(person, "Original Person".into());
        app.set_local_alias(AliasTarget::Person(person), "  Żółw 🐢  ")
            .unwrap();
        assert_eq!(app.name_by_id_cached(person), "Żółw 🐢");
        assert_eq!(
            app.channel_name(app.channel(channel_id).unwrap()),
            "Żółw 🐢"
        );
        assert_eq!(app.name_by_id(person).await, "Original Person");
        app.set_local_alias(AliasTarget::Channel(channel_id), "Praca")
            .unwrap();
        assert_eq!(app.channel_name(app.channel(channel_id).unwrap()), "Praca");
        assert_eq!(app.name_by_id_cached(person), "Żółw 🐢");
        app.ensure_contact_channel_exists(person, "New Signal Name")
            .await;
        assert_eq!(app.channel_name(app.channel(channel_id).unwrap()), "Praca");
        assert_eq!(
            app.storage.channel(channel_id).unwrap().name,
            "New Signal Name"
        );
        app.set_local_alias(AliasTarget::Channel(channel_id), "  ")
            .unwrap();
        assert_eq!(
            app.channel_name(app.channel(channel_id).unwrap()),
            "Żółw 🐢"
        );
        app.set_local_alias(AliasTarget::Person(person), "")
            .unwrap();
        assert_eq!(app.name_by_id_cached(person), "Original Person");
        assert!(app.storage.aliases().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_input() {
        let (mut app, mut events, sent_messages) = test_app().await;
        let input = "Hello, World!";
        for c in input.chars() {
            app.get_input().put_char(c);
        }
        let channel_id = app.channels().first().unwrap().id;
        app.send_input(channel_id);

        assert_eq!(sent_messages.borrow().len(), 1);
        let msg = sent_messages.borrow()[0].clone();
        assert_eq!(msg.message.as_ref().unwrap(), input);

        let channel = app.channel(channel_id).unwrap();
        assert_eq!(channel.unread_messages, 0);

        assert_eq!(app.get_input().data, "");

        match events.recv().await.unwrap() {
            Event::SentTextResult { message_id, result } => {
                assert_eq!(message_id.arrived_at, msg.arrived_at);
                assert!(result.is_ok());
            }
            event => panic!("unexpected event: {event:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_input_with_emoji() {
        let (mut app, mut events, sent_messages) = test_app().await;
        let input = "\u{1F47B}";
        for c in input.chars() {
            app.get_input().put_char(c);
        }

        let channel_id = app.channels().first().unwrap().id;
        app.send_input(channel_id);

        assert_eq!(sent_messages.borrow().len(), 1);
        let msg = sent_messages.borrow()[0].clone();
        assert_eq!(msg.message.as_ref().unwrap(), input);

        assert_eq!(app.get_input().data, "");

        match events.recv().await.unwrap() {
            Event::SentTextResult { message_id, result } => {
                assert_eq!(message_id.arrived_at, msg.arrived_at);
                assert!(result.is_ok());
            }
            event => panic!("unexpected event: {event:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_input_with_emoji_codepoint() {
        let (mut app, mut events, sent_messages) = test_app().await;
        let input = ":thumbsup:";
        for c in input.chars() {
            app.get_input().put_char(c);
        }

        let channel_id = app.channels().first().unwrap().id;
        app.send_input(channel_id);

        assert_eq!(sent_messages.borrow().len(), 1);
        let msg = sent_messages.borrow()[0].clone();
        assert_eq!(msg.message.as_ref().unwrap(), "\u{1F44D}");

        match events.recv().await.unwrap() {
            Event::SentTextResult { message_id, result } => {
                assert_eq!(message_id.arrived_at, msg.arrived_at);
                assert!(result.is_ok());
            }
            event => panic!("unexpected event: {event:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_reaction_with_emoji() {
        let (mut app, _events, _sent_messages) = test_app().await;

        let channel_id = app.channels().first().unwrap().id;
        app.positions.entry(channel_id).or_default().selected =
            Some(app.storage.messages_tail(channel_id, 1)[0].arrived_at);

        app.get_input().put_char('\u{1F44D}');

        let channel_id = app.channels().first().unwrap().id;
        app.add_reaction(channel_id, None).await;

        let arrived_at = app.storage.messages_tail(channel_id, 1)[0].arrived_at;
        let reactions = &app
            .storage
            .message(MessageId::new(channel_id, arrived_at))
            .unwrap()
            .reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0], (app.user_id, "\u{1F44D}".to_string()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_reaction_with_emoji_codepoint() {
        let (mut app, _events, _sent_messages) = test_app().await;

        let channel_id = app.channels().first().unwrap().id;
        app.positions.entry(channel_id).or_default().selected =
            Some(app.storage.messages_tail(channel_id, 1)[0].arrived_at);

        for c in ":thumbsup:".chars() {
            app.get_input().put_char(c);
        }
        app.add_reaction(channel_id, None).await;

        let arrived_at = app.storage.messages_tail(channel_id, 1)[0].arrived_at;
        let reactions = &app
            .storage
            .message(MessageId::new(channel_id, arrived_at))
            .unwrap()
            .reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0], (app.user_id, "\u{1F44D}".to_string()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_remove_reaction() {
        let (mut app, _events, _sent_messages) = test_app().await;

        let channel_id = app.channels().first().unwrap().id;
        app.positions.entry(channel_id).or_default().selected =
            Some(app.storage.messages_tail(channel_id, 1)[0].arrived_at);

        let arrived_at = app.storage.messages_tail(channel_id, 1)[0].arrived_at;
        let mut message = app
            .storage
            .message(MessageId::new(channel_id, arrived_at))
            .unwrap();
        message
            .reactions
            .push((app.user_id, "\u{1F44D}".to_string()));
        app.store_message(channel_id, message);
        app.add_reaction(channel_id, None).await;

        let reactions = &app
            .storage
            .message(MessageId::new(channel_id, arrived_at))
            .unwrap()
            .reactions;
        assert!(reactions.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_invalid_reaction() {
        let (mut app, _events, _sent_messages) = test_app().await;

        let channel_id = app.channels().first().unwrap().id;
        app.positions.entry(channel_id).or_default().selected =
            Some(app.storage.messages_tail(channel_id, 1)[0].arrived_at);

        for c in ":thumbsup".chars() {
            app.get_input().put_char(c);
        }
        app.add_reaction(channel_id, None).await;

        assert_eq!(app.get_input().data, ":thumbsup");
        let arrived_at = app.storage.messages_tail(channel_id, 1)[0].arrived_at;
        let reactions = &app
            .storage
            .message(MessageId::new(channel_id, arrived_at))
            .unwrap()
            .reactions;
        assert!(reactions.is_empty());
    }

    #[test]
    fn test_to_emoji() {
        assert_eq!(to_emoji("\u{1F680}"), Some("\u{1F680}"));
        assert_eq!(to_emoji("  \u{1F680}   "), Some("\u{1F680}")); // trimmed
        assert_eq!(to_emoji(":rocket:"), Some("\u{1F680}"));
        assert_eq!(to_emoji("\u{261D}\u{1F3FF}"), Some("\u{261D}\u{1F3FF}"));
        assert_eq!(to_emoji("a"), None);
    }

    #[test]
    fn test_extract_attachments() {
        let tempdir = tempfile::tempdir().unwrap();
        let image_png = tempdir.path().join("image.png");
        let image_jpg = tempdir.path().join("image.jpg");

        std::fs::write(&image_png, b"some png data").unwrap();
        std::fs::write(&image_jpg, b"some jpg data").unwrap();

        let clipboard_image = ImageData {
            width: 1,
            height: 1,
            bytes: vec![0, 0, 0, 0].into(), // RGBA single pixel
        };

        let message = format!(
            "Hello, file://{} file://{} World! file://clip",
            image_png.display(),
            image_jpg.display(),
        );

        let at_str = "2023-01-01T00:00:00+0200";
        let at: DateTime<FixedOffset> = at_str.parse().unwrap();

        let (cleaned_message, specs) =
            App::extract_attachments(&message, at, || Some(Ok(clipboard_image.clone())), &[]);
        assert_eq!(cleaned_message, "Hello, World!");
        dbg!(&specs);

        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].0.content_type, "image/png");
        assert_eq!(specs[0].0.file_name, Some("image.png".into()));
        assert_eq!(specs[1].0.content_type, "image/jpeg");
        assert_eq!(specs[1].0.file_name, Some("image.jpg".into()));
        assert_eq!(specs[2].0.content_type, "image/png");
        assert_eq!(
            specs[2].0.file_name,
            Some(format!("screenshot-{at_str}.png"))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_ensure_user_is_known_resolves_in_background() {
        let (mut app, mut events, _sent_messages) = test_app().await;
        let uuid = Uuid::new_v4();

        app.ensure_user_is_known(uuid, Some([0; 32])).await;
        assert_eq!(app.storage.name(uuid), Some(uuid.to_string()));
        let name = app.name_by_id(uuid).await;
        let channel_id = app.ensure_contact_channel_exists(uuid, &name).await;

        match events.recv().await.unwrap() {
            Event::ProfileNameResolved { user_id, name } => {
                assert_eq!(user_id, uuid);
                assert_eq!(name, None);
            }
            event => panic!("unexpected event: {event:?}"),
        }

        app.on_profile_name_resolved(uuid, "ellie".to_string());
        assert_eq!(app.storage.name(uuid).as_deref(), Some("ellie"));
        assert_eq!(app.name_by_id(uuid).await, "ellie");
        assert_eq!(app.channel(channel_id).unwrap().name, "ellie");
    }
}
