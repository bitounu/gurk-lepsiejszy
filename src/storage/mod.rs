mod copy;
mod forgetful;
mod sql;

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::data::{Channel, ChannelId, Message};

pub use copy::sync_from_signal;
pub use forgetful::ForgetfulStorage;
pub use sql::SqliteStorage;

/// A local display override, independent of names synchronized with Signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AliasTarget {
    Person(Uuid),
    Channel(ChannelId),
}

/// Unsent composer contents for one person or group.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    pub cursor: usize,
    #[serde(default)]
    pub editing: Option<u64>,
    pub attachments: Vec<std::path::PathBuf>,
    #[serde(default)]
    pub mentions: Vec<crate::mentions::DraftMention>,
}

/// Storage of channels, messages, names and metadata.
///
/// Used to persist the data to disk.
pub trait Storage {
    /// Whether a read has already been confirmed synchronized to linked devices.
    fn read_sync_completed(&self, _sender: Uuid, _timestamp: u64) -> anyhow::Result<bool> {
        Ok(false)
    }

    /// Durable queue of local reads awaiting synchronization to linked devices.
    fn pending_read_sync(&self) -> anyhow::Result<Vec<(Uuid, u64)>> {
        Ok(Vec::new())
    }

    fn set_read_sync_pending(
        &mut self,
        _sender: Uuid,
        _timestamp: u64,
        _pending: bool,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Read synchronization requires persistent storage")
    }

    /// Select the UI ordering; protocol message IDs remain unchanged.
    fn set_message_order(&mut self, _order: crate::config::MessageOrder) {}

    /// Comparable UI position for a message ID in the selected ordering.
    fn message_sort_key(&self, id: MessageId) -> u64 {
        id.arrived_at
    }

    /// Load local favorites independently of synchronized conversation data.
    fn favorite_channels(&self) -> anyhow::Result<BTreeSet<ChannelId>>;

    fn set_channel_favorite(&mut self, channel: ChannelId, favorite: bool) -> anyhow::Result<()>;

    /// Load local conversation visibility overrides, reporting read or decode failures.
    fn hidden_channels(&self) -> anyhow::Result<BTreeSet<ChannelId>>;

    /// Hide or reveal a conversation locally without changing its messages or metadata.
    fn set_channel_hidden(&mut self, channel: ChannelId, hidden: bool) -> anyhow::Result<()>;

    /// Load unsent drafts, reporting corrupt data or read failures.
    fn drafts(&self) -> anyhow::Result<BTreeMap<ChannelId, Draft>>;

    /// Save a draft, removing it when both text and attachments are empty.
    fn store_draft(&mut self, channel_id: ChannelId, draft: &Draft) -> anyhow::Result<()>;

    /// Restore the last selected composer context, even if its draft is empty.
    fn last_draft_channel(&self) -> anyhow::Result<Option<ChannelId>>;

    fn store_last_draft_channel(&mut self, channel_id: Option<ChannelId>) -> anyhow::Result<()>;

    /// Load local aliases. Errors must not silently discard saved overrides.
    fn aliases(&self) -> anyhow::Result<BTreeMap<AliasTarget, String>>;

    /// Persist an alias, or remove it when `None`. Report write failures to the editor.
    fn set_alias(&mut self, target: AliasTarget, alias: Option<&str>) -> anyhow::Result<()>;

    /// Returns the list of channels
    ///
    /// The order is from most recent to least recent, that is, descending by arrived_at.
    fn channels(&self) -> Vec<Channel>;

    /// Gets the channel by id
    fn channel(&self, channel_id: ChannelId) -> Option<Channel>;

    /// Stores the given `channel`
    fn store_channel(&mut self, channel: &Channel);

    /// The newest `limit` messages, ascending in the configured display order.
    fn messages_tail(&self, channel_id: ChannelId, limit: usize) -> Vec<Message>;

    /// Up to `limit` messages before the timestamp ID `anchor` in display order.
    fn messages_before(&self, channel_id: ChannelId, anchor: u64, limit: usize) -> Vec<Message>;

    /// Up to `limit` messages after the timestamp ID `anchor` in display order.
    fn messages_after(&self, channel_id: ChannelId, anchor: u64, limit: usize) -> Vec<Message>;

    // Messages window functions

    /// Messages sorted by arrived_at in ascending order
    ///
    /// No edited messages must be included.
    fn messages(&self, channel_id: ChannelId) -> Box<dyn DoubleEndedIterator<Item = Message> + '_>;
    /// Gets the message by id
    fn message(&self, message_id: MessageId) -> Option<Message>;

    /// Resolve Signal's author + original sent timestamp, never a UI storage key.
    fn message_by_protocol(
        &self,
        channel_id: ChannelId,
        sender: Uuid,
        timestamp: u64,
    ) -> Option<Message> {
        self.message(MessageId::new(channel_id, timestamp))
            .filter(|m| m.from_id == sender && m.arrived_at == timestamp)
    }

    fn edits(&self, message_id: MessageId) -> Box<dyn DoubleEndedIterator<Item = Message> + '_>;

    /// Search the full conversation, returning every occurrence in display order.
    fn search_messages(
        &self,
        channel: ChannelId,
        query: &str,
    ) -> anyhow::Result<Vec<crate::search::SearchHit>> {
        let Some(regex) = crate::search::literal_regex(query)? else {
            return Ok(Vec::new());
        };
        let mut messages: Vec<_> = self
            .messages(channel)
            .filter(|message| {
                !message.deleted && message.edit.is_none() && !message.is_edit_snapshot
            })
            .collect();
        messages.sort_by_key(|message| {
            (
                self.message_sort_key(MessageId::new(channel, message.storage_id())),
                message.storage_id(),
            )
        });
        let mut hits = Vec::new();
        for message in messages {
            let Some(body) = message.message.as_deref() else {
                continue;
            };
            let body = crate::search::normalize_body(body);
            hits.extend(
                regex
                    .find_iter(&body)
                    .enumerate()
                    .map(|(occurrence, found)| crate::search::SearchHit {
                        message_id: MessageId::new(channel, message.storage_id()),
                        range: found.range(),
                        occurrence,
                    }),
            );
        }
        Ok(hits)
    }

    fn oldest_message(&self, channel: ChannelId) -> Option<Message> {
        self.messages(channel).min_by_key(|message| {
            (
                self.message_sort_key(MessageId::new(channel, message.storage_id())),
                message.storage_id(),
            )
        })
    }

    fn last_message(&self, channel_id: ChannelId) -> Option<Message> {
        self.messages_tail(channel_id, 1).pop()
    }

    fn messages_count_after(&self, channel_id: ChannelId, arrived_at: u64) -> usize;

    fn remove_expired(&self, now_ms: u64) -> Vec<MessageId>;

    fn next_expiring_at(&self) -> Option<u64>;

    /// Stores the message for the given `channel_id`.
    ///
    /// The channel with the given `channel_id` must already exist.
    fn store_message(&mut self, channel_id: ChannelId, message: &Message);

    /// Applies an edit and returns the updated original message, or `None` if the message pointed
    /// to by `target_sent_timestamp` does not exist.
    fn store_edited_message(
        &mut self,
        channel_id: ChannelId,
        target_sent_timestampt: u64,
        message: Message,
    ) -> Option<Message> {
        // Note: target_sent_timestamp points to the previous edit or the original message
        let prev_edited =
            self.message_by_protocol(channel_id, message.from_id, target_sent_timestampt)?;

        // get original message
        let mut original = if let Some(arrived_at) = prev_edited.edit {
            // previous edit => get original message
            self.message(MessageId::new(channel_id, arrived_at))?
        } else {
            // original message => first edit
            let original = prev_edited;

            // preserve body of the original message; it is replaced below
            if !original.edited {
                let mut preserved = original.clone();
                preserved.local_id = None;
                preserved.is_edit_snapshot = true;
                preserved.edit = Some(original.storage_id());
                self.store_message(channel_id, &preserved);
            }

            original
        };

        // store the incoming edit
        let body = message.message.clone();
        self.store_message(
            channel_id,
            &Message {
                edit: Some(original.storage_id()),
                ..message
            },
        );

        // override the body of the original message
        original.message = body;
        original.edited = true;
        self.store_message(channel_id, &original);
        Some(original)
    }

    /// Marks a message as deleted (remote delete / delete for everyone).
    ///
    /// Clears the message body and attachments, and sets the `deleted` flag.
    /// Returns `true` if the message existed.
    fn delete_message(&mut self, message_id: MessageId) -> bool {
        let Some(mut message) = self.message(message_id) else {
            return false;
        };
        message.message = None;
        message.attachments.clear();
        message.body_ranges.clear();
        message.deleted = true;
        self.store_message(message_id.channel_id, &message);
        true
    }

    /// Fully removes a message from storage (delete for me)
    fn remove_message(&mut self, message_id: MessageId);

    /// Names of contacts
    fn names(&self) -> Box<dyn Iterator<Item = (Uuid, String)> + '_>;
    /// Gets the name for the given contact `id`
    fn name(&self, id: Uuid) -> Option<String>;
    /// Stores a name for the given contact `id`
    ///
    /// If the name with this `id` already exists in the storage, it is overridden. Otherwise, it
    /// the name is added to the storage.
    fn store_name(&mut self, id: Uuid, name: &str);

    /// Returns the metadata containing persisted flags and settings
    fn metadata(&self) -> Metadata;
    /// Stores the new metadata in the storage overriding the previous one
    fn store_metadata(&mut self, metadata: &Metadata);

    /// Persists the data in the storage
    ///
    /// ## Implementation note
    ///
    /// The implementers of this trait, can persist for each store call, if it is efficient enough.
    /// This methods must guarantee that the data is persisted in any case.
    fn save(&mut self);

    // /// Returns `true` if this storage does not contains any channels and no names
    // fn is_empty(&self) -> bool {
    //     self.channels().next().is_none() && self.names().next().is_none()
    // }
}

/// Local row identity: channel plus opaque local key (usually the original timestamp).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MessageId {
    pub channel_id: ChannelId,
    pub arrived_at: u64,
}

impl MessageId {
    pub fn new(channel_id: ChannelId, arrived_at: u64) -> Self {
        Self {
            channel_id,
            arrived_at,
        }
    }
}

/// Persisted metadata
#[derive(Debug, Default, Clone)]
pub struct Metadata {
    /// The time of the last request to synchronize contacts
    ///
    /// Used to amortize calls to the backend.
    pub contacts_sync_request_at: Option<DateTime<Utc>>,
    pub fully_migrated: Option<bool>,
}
