use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use sqlx::{query, query_as, query_scalar};
use tokio::{runtime::Handle, task};
use tracing::{error, info};
use url::Url;
use uuid::Uuid;

use crate::receipt::Receipt;
use crate::signal::Attachment;
use crate::storage::{AliasTarget, Draft, MessageId, Metadata, Storage};
use crate::{
    data::{BodyRange, Channel, ChannelId, GroupData, Message, TypingSet},
    passphrase::Passphrase,
};

use super::encoding::BlobData;
use super::encrypt::{encrypt_db, is_sqlite_encrypted_heuristics};
use super::util::ResultExt as _;

const METADATA_ID: i64 = 0;

pub struct SqliteStorage {
    pool: SqlitePool,
    message_order: crate::config::MessageOrder,
}

impl SqliteStorage {
    /// Rebuild display order from authenticated backup order, preserving unrelated local rows.
    pub async fn restore_received_order(&self, archive: &[MessageId]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        let previous: Vec<(ChannelId, i64)> = sqlx::query_as(
            "SELECT channel_id,arrived_at FROM message_display_order ORDER BY display_order",
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut seen = std::collections::BTreeSet::new();
        let mut ordered = Vec::new();
        for id in archive {
            if seen.insert((id.channel_id, id.arrived_at)) {
                ordered.push((id.channel_id, id.arrived_at));
            }
        }
        for (channel, at) in previous {
            let at = u64::try_from(at)?;
            if seen.insert((channel, at)) {
                ordered.push((channel, at));
            }
        }
        sqlx::query("DELETE FROM message_display_order")
            .execute(&mut *tx)
            .await?;
        for (channel, at) in ordered {
            let at = i64::try_from(at)?;
            sqlx::query("INSERT INTO message_display_order(channel_id,arrived_at) SELECT ?,? WHERE EXISTS (SELECT 1 FROM messages WHERE channel_id=? AND arrived_at=?)")
                .bind(&channel).bind(at).bind(&channel).bind(at).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn maybe_encrypt_and_open(
        url: &Url,
        passphrase: &Passphrase,
        preserve_unencrypted: bool,
    ) -> anyhow::Result<Self> {
        info!(%url, "loading sql app data");

        let db = match is_sqlite_encrypted_heuristics(url) {
            // encrypted or does not exist
            Some(true) | None => Self::open(url, passphrase).await?,
            // not encrypted => encrypt
            Some(false) => {
                encrypt_db(url, passphrase, preserve_unencrypted).await?;
                Self::open(url, passphrase).await?
            }
        };

        Ok(db)
    }

    pub async fn open(url: &Url, passphrase: &Passphrase) -> sqlx::Result<Self> {
        let opts: SqliteConnectOptions = url.as_str().parse()?;
        let opts = opts
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .pragma("key", passphrase.sqlite_pragma_key().to_string());

        let pool = SqlitePool::connect_with(opts.clone()).await?;
        sqlx::migrate!().run(&pool).await?;

        Ok(Self {
            pool,
            message_order: Default::default(),
        })
    }

    #[cfg(any(test, feature = "dev"))]
    pub async fn open_unencrypted(url: &Url) -> sqlx::Result<Self> {
        let opts: SqliteConnectOptions = url.as_str().parse()?;
        let opts = opts
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full);

        let pool = SqlitePool::connect_with(opts.clone()).await?;
        sqlx::migrate!().run(&pool).await?;

        Ok(Self {
            pool,
            message_order: Default::default(),
        })
    }
    /// Anchor values are local row IDs; ordering is by immutable Signal time.
    fn sent_page(
        &self,
        channel: ChannelId,
        anchor: Option<(bool, u64)>,
        limit: usize,
    ) -> Vec<Message> {
        let sql = match anchor {
            None => {
                "SELECT m.arrived_at FROM messages m JOIN message_identity i ON i.channel_id=m.channel_id AND i.local_id=m.arrived_at WHERE m.channel_id=?1 AND m.edit IS NULL ORDER BY i.protocol_time DESC, m.arrived_at DESC LIMIT ?2"
            }
            Some((true, _)) => {
                "SELECT m.arrived_at FROM messages m JOIN message_identity i ON i.channel_id=m.channel_id AND i.local_id=m.arrived_at WHERE m.channel_id=?1 AND m.edit IS NULL AND (i.protocol_time, m.arrived_at) > (?3, ?4) ORDER BY i.protocol_time ASC, m.arrived_at ASC LIMIT ?2"
            }
            Some((false, _)) => {
                "SELECT m.arrived_at FROM messages m JOIN message_identity i ON i.channel_id=m.channel_id AND i.local_id=m.arrived_at WHERE m.channel_id=?1 AND m.edit IS NULL AND (i.protocol_time, m.arrived_at) < (?3, ?4) ORDER BY i.protocol_time DESC, m.arrived_at DESC LIMIT ?2"
            }
        };
        let mut query = sqlx::query_scalar::<_, i64>(sql)
            .bind(&channel)
            .bind(i64::try_from(limit).unwrap_or(i64::MAX));
        if let Some((_, id)) = anchor {
            let time = self.message_sort_time(MessageId::new(channel, id));
            query = query.bind(time as i64).bind(id as i64);
        }
        let mut ids = block_async_in_place(query.fetch_all(&self.pool))
            .ok_logged()
            .unwrap_or_default();
        if !matches!(anchor, Some((true, _))) {
            ids.reverse();
        }
        ids.into_iter()
            .filter_map(|id| self.message(MessageId::new(channel, id as u64)))
            .collect()
    }

    fn message_sort_time(&self, id: MessageId) -> u64 {
        block_async_in_place(
            sqlx::query_scalar::<_, i64>(
                "SELECT protocol_time FROM message_identity WHERE channel_id=? AND local_id=?",
            )
            .bind(&id.channel_id)
            .bind(id.arrived_at as i64)
            .fetch_optional(&self.pool),
        )
        .ok_logged()
        .flatten()
        .map(|n| n as u64)
        .unwrap_or(id.arrived_at)
    }

    fn restore_identity(&self, channel: ChannelId, mut message: Message) -> Message {
        let local_id = message.arrived_at;
        let identity=block_async_in_place(sqlx::query_as::<_,(i64,bool,Option<BlobData<Message>>)>(
            "SELECT protocol_time,is_snapshot,quote_snapshot FROM message_identity WHERE channel_id=? AND local_id=?"
        ).bind(&channel).bind(local_id as i64).fetch_optional(&self.pool)).ok_logged().flatten();
        let snapshot = if let Some((protocol_time, is_snapshot, snapshot)) = identity {
            message.arrived_at = protocol_time as u64;
            message.local_id = (local_id != message.arrived_at).then_some(local_id);
            message.is_edit_snapshot = is_snapshot;
            snapshot
        } else {
            None
        };
        if let Some(snapshot) = snapshot {
            let mut quote = snapshot.into_inner();
            let local = block_async_in_place(sqlx::query_scalar::<_, i64>("SELECT local_id FROM message_identity WHERE channel_id=? AND from_id=? AND protocol_time=? AND is_snapshot=0")
                .bind(&channel).bind(quote.from_id).bind(quote.arrived_at as i64).fetch_optional(&self.pool)).ok_logged().flatten();
            quote.local_id = local
                .map(|id| id as u64)
                .filter(|id| *id != quote.arrived_at);
            message.quote = Some(Box::new(quote));
        } else if let Some(quote) = message.quote.as_mut() {
            let local_id = quote.arrived_at;
            quote.arrived_at = self.message_sort_time(MessageId::new(channel, local_id));
            quote.local_id = (local_id != quote.arrived_at).then_some(local_id);
        }
        message
    }

    fn received_page(
        &self,
        channel_id: ChannelId,
        anchor: Option<(bool, u64)>,
        descending: bool,
        limit: usize,
    ) -> Vec<Message> {
        let sql = match anchor {
            None => {
                "SELECT m.arrived_at FROM messages m JOIN message_display_order o ON o.channel_id = m.channel_id AND o.arrived_at = m.arrived_at WHERE m.channel_id = ?1 AND m.edit IS NULL ORDER BY o.display_order DESC LIMIT ?2"
            }
            Some((true, _)) => {
                "SELECT m.arrived_at FROM messages m JOIN message_display_order o ON o.channel_id = m.channel_id AND o.arrived_at = m.arrived_at WHERE m.channel_id = ?1 AND m.edit IS NULL AND o.display_order > ?3 ORDER BY o.display_order ASC LIMIT ?2"
            }
            Some((false, _)) => {
                "SELECT m.arrived_at FROM messages m JOIN message_display_order o ON o.channel_id = m.channel_id AND o.arrived_at = m.arrived_at WHERE m.channel_id = ?1 AND m.edit IS NULL AND o.display_order < ?3 ORDER BY o.display_order DESC LIMIT ?2"
            }
        };
        let mut query = sqlx::query_scalar::<_, i64>(sql)
            .bind(&channel_id)
            .bind(limit as i64);
        if let Some((_, timestamp)) = anchor {
            query = query.bind(self.message_sort_key(MessageId::new(channel_id, timestamp)) as i64);
        }
        let mut ids = block_async_in_place(query.fetch_all(&self.pool))
            .ok_logged()
            .unwrap_or_default();
        if descending {
            ids.reverse();
        }
        ids.into_iter()
            .filter_map(|at| self.message(MessageId::new(channel_id, at as u64)))
            .collect()
    }
}

struct SqlChannel {
    id: ChannelId,
    name: String,
    group_master_key: Option<Vec<u8>>,
    group_revision: Option<i64>,
    group_members: Option<BlobData<Vec<Uuid>>>,
    muted: bool,
    expire_timer: Option<i64>,
}

impl SqlChannel {
    fn convert(self) -> Result<Channel, ChannelConvertError> {
        let Self {
            id,
            name,
            group_master_key,
            group_revision,
            group_members,
            muted,
            expire_timer,
        } = self;
        use ChannelConvertError::*;
        let group_data = match (group_master_key, group_revision, group_members) {
            (Some(master_key_bytes), Some(revision), Some(members)) => Some(GroupData {
                master_key_bytes: master_key_bytes.try_into().map_err(|_| MasterKeyBytes)?,
                members: members.into_inner(),
                revision: revision.try_into().map_err(|_| Revision)?,
            }),
            _ => None,
        };
        let is_group = group_data.is_some();
        Ok(Channel {
            id,
            name,
            group_data,
            unread_messages: Default::default(),
            muted,
            typing: TypingSet::new(is_group),
            expire_timer: expire_timer.and_then(|t| u32::try_from(t).ok()),
        })
    }
}

struct SqlMessage {
    from_id: Uuid,
    message: Option<String>,
    arrived_at: i64,
    attachments: Option<BlobData<Vec<Attachment>>>,
    reactions: Option<BlobData<Vec<(Uuid, String)>>>,
    receipt: Option<BlobData<Receipt>>,
    body_ranges: Option<BlobData<Vec<BodyRange>>>,
    quote_arrived_at: Option<i64>,
    quote_from_id: Option<Uuid>,
    quote_message: Option<String>,
    quote_attachments: Option<BlobData<Vec<Attachment>>>,
    quote_body_ranges: Option<BlobData<Vec<BodyRange>>>,
    quote_receipt: Option<BlobData<Receipt>>,
    edit: Option<i64>,
    edited: bool,
    deleted: bool,
    expire_timer: Option<i64>,
    expires_at: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
enum MessageConvertError {
    #[error("timestamp out of bounds")]
    InvalidTimestamp,
}

impl SqlMessage {
    fn convert(self) -> Result<Message, MessageConvertError> {
        let SqlMessage {
            from_id,
            message,
            arrived_at,
            attachments,
            reactions,
            receipt,
            body_ranges,
            quote_arrived_at,
            quote_from_id,
            quote_message,
            quote_attachments,
            quote_body_ranges,
            quote_receipt,
            edit,
            edited,
            deleted,
            expire_timer,
            expires_at,
        } = self;

        let quote = quote_arrived_at
            .zip(quote_from_id)
            .and_then(|(arrived_at, from_id)| {
                let mut quote = Message::new(
                    from_id,
                    quote_message,
                    quote_body_ranges
                        .map(BlobData::into_inner)
                        .unwrap_or_default(),
                    arrived_at
                        .try_into()
                        .map_err(|_| MessageConvertError::InvalidTimestamp)
                        .ok_logged()?,
                    quote_attachments
                        .map(BlobData::into_inner)
                        .unwrap_or_default(),
                );
                quote.receipt = quote_receipt.map(BlobData::into_inner).unwrap_or_default();
                Some(quote)
            });

        Ok(Message {
            local_id: None,
            is_edit_snapshot: false,
            from_id,
            message,
            arrived_at: arrived_at
                .try_into()
                .map_err(|_| MessageConvertError::InvalidTimestamp)?,
            quote: quote.map(Box::new),
            attachments: attachments.map(BlobData::into_inner).unwrap_or_default(),
            reactions: reactions.map(BlobData::into_inner).unwrap_or_default(),
            receipt: receipt.map(BlobData::into_inner).unwrap_or_default(),
            body_ranges: body_ranges.map(BlobData::into_inner).unwrap_or_default(),
            send_failed: Default::default(),
            attachment_downloads: Default::default(),
            edit: edit.and_then(|edit| {
                edit.try_into()
                    .map_err(|_| MessageConvertError::InvalidTimestamp)
                    .ok_logged()
            }),
            edited,
            deleted,
            expire_timer: expire_timer.and_then(|t| u32::try_from(t).ok()),
            expires_at: expires_at.and_then(|t| u64::try_from(t).ok()),
        })
    }
}

#[derive(Debug, thiserror::Error)]
enum ChannelConvertError {
    #[error("invalid master key bytes")]
    MasterKeyBytes,
    #[error("invalid revision")]
    Revision,
}

struct SqlName {
    id: Uuid,
    name: String,
}

impl Storage for SqliteStorage {
    fn read_sync_completed(&self, sender: Uuid, timestamp: u64) -> anyhow::Result<bool> {
        let timestamp = i64::try_from(timestamp)?;
        Ok(block_async_in_place(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM private_read_sync_confirmed WHERE sender = ? AND timestamp = ?)"
        ).bind(sender).bind(timestamp).fetch_one(&self.pool))?)
    }

    fn pending_read_sync(&self) -> anyhow::Result<Vec<(Uuid, u64)>> {
        let rows = block_async_in_place(
            sqlx::query_as::<_, (Uuid, i64)>(
                "SELECT sender, timestamp FROM private_read_sync_outbox ORDER BY timestamp, sender",
            )
            .fetch_all(&self.pool),
        )?;
        rows.into_iter()
            .map(|(sender, timestamp)| Ok((sender, u64::try_from(timestamp)?)))
            .collect()
    }

    fn set_read_sync_pending(
        &mut self,
        sender: Uuid,
        timestamp: u64,
        pending: bool,
    ) -> anyhow::Result<()> {
        let timestamp = i64::try_from(timestamp)?;
        block_async_in_place(async {
            if pending {
                sqlx::query("INSERT OR IGNORE INTO private_read_sync_outbox (sender, timestamp) SELECT ?1, ?2 WHERE NOT EXISTS (SELECT 1 FROM private_read_sync_confirmed WHERE sender = ?1 AND timestamp = ?2)")
                    .bind(sender).bind(timestamp).execute(&self.pool).await?;
            } else {
                // An ACK must never remove the retry record without durably recording completion.
                let mut tx = self.pool.begin().await?;
                sqlx::query("INSERT OR IGNORE INTO private_read_sync_confirmed (sender, timestamp) VALUES (?, ?)")
                    .bind(sender).bind(timestamp).execute(&mut *tx).await?;
                sqlx::query(
                    "DELETE FROM private_read_sync_outbox WHERE sender = ? AND timestamp = ?",
                )
                .bind(sender)
                .bind(timestamp)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
            Ok(())
        })
    }

    fn favorite_channels(&self) -> anyhow::Result<std::collections::BTreeSet<ChannelId>> {
        block_async_in_place(async {
            Ok(
                sqlx::query_scalar::<_, ChannelId>("SELECT channel_id FROM favorite_channels")
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .collect(),
            )
        })
    }

    fn set_channel_favorite(&mut self, channel: ChannelId, favorite: bool) -> anyhow::Result<()> {
        let sql = if favorite {
            "INSERT INTO favorite_channels(channel_id) VALUES (?) ON CONFLICT(channel_id) DO NOTHING"
        } else {
            "DELETE FROM favorite_channels WHERE channel_id = ?"
        };
        block_async_in_place(async {
            sqlx::query(sql).bind(&channel).execute(&self.pool).await?;
            Ok(())
        })
    }

    fn hidden_channels(&self) -> anyhow::Result<std::collections::BTreeSet<ChannelId>> {
        block_async_in_place(async {
            Ok(
                sqlx::query_scalar::<_, ChannelId>("SELECT channel_id FROM hidden_channels")
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .collect(),
            )
        })
    }

    fn set_channel_hidden(&mut self, channel: ChannelId, hidden: bool) -> anyhow::Result<()> {
        let sql = if hidden {
            "INSERT INTO hidden_channels(channel_id) VALUES (?) ON CONFLICT(channel_id) DO NOTHING"
        } else {
            "DELETE FROM hidden_channels WHERE channel_id = ?"
        };
        block_async_in_place(async {
            sqlx::query(sql).bind(&channel).execute(&self.pool).await?;
            Ok(())
        })
    }

    fn drafts(&self) -> anyhow::Result<std::collections::BTreeMap<ChannelId, Draft>> {
        block_async_in_place(async {
            let rows = sqlx::query_as::<_, (ChannelId, String, i64, Option<i64>, Vec<u8>, String)>(
                "SELECT channel_id, text, cursor, editing, attachments, mentions FROM composer_drafts",
            )
            .fetch_all(&self.pool)
            .await?;
            let mut drafts = std::collections::BTreeMap::new();
            for (channel_id, text, cursor, editing, attachments, mentions) in rows {
                drafts.insert(
                    channel_id,
                    Draft {
                        text,
                        cursor: usize::try_from(cursor)?,
                        editing: editing.map(u64::try_from).transpose()?,
                        attachments: postcard::from_bytes(&attachments)?,
                        mentions: serde_json::from_str(&mentions)?,
                    },
                );
            }
            Ok(drafts)
        })
    }

    fn store_draft(&mut self, channel_id: ChannelId, draft: &Draft) -> anyhow::Result<()> {
        block_async_in_place(async {
            if draft.text.is_empty() && draft.attachments.is_empty() {
                sqlx::query("DELETE FROM composer_drafts WHERE channel_id = ?")
                    .bind(&channel_id)
                    .execute(&self.pool)
                    .await?;
            } else {
                let cursor = i64::try_from(draft.cursor)?;
                let editing = draft.editing.map(i64::try_from).transpose()?;
                let attachments = postcard::to_allocvec(&draft.attachments)?;
                sqlx::query("INSERT INTO composer_drafts(channel_id, text, cursor, editing, attachments, mentions) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(channel_id) DO UPDATE SET text = excluded.text, cursor = excluded.cursor, editing = excluded.editing, attachments = excluded.attachments, mentions = excluded.mentions")
                    .bind(&channel_id).bind(&draft.text).bind(cursor).bind(editing).bind(attachments).bind(serde_json::to_string(&draft.mentions)?)
                    .execute(&self.pool).await?;
            }
            Ok(())
        })
    }

    fn last_draft_channel(&self) -> anyhow::Result<Option<ChannelId>> {
        block_async_in_place(async {
            Ok(sqlx::query_scalar::<_, ChannelId>(
                "SELECT channel_id FROM composer_context WHERE id = 0",
            )
            .fetch_optional(&self.pool)
            .await?)
        })
    }

    fn store_last_draft_channel(&mut self, channel_id: Option<ChannelId>) -> anyhow::Result<()> {
        block_async_in_place(async {
            if let Some(channel_id) = channel_id {
                sqlx::query("INSERT INTO composer_context(id, channel_id) VALUES (0, ?) ON CONFLICT(id) DO UPDATE SET channel_id = excluded.channel_id")
                    .bind(&channel_id).execute(&self.pool).await?;
            } else {
                sqlx::query("DELETE FROM composer_context WHERE id = 0")
                    .execute(&self.pool)
                    .await?;
            }
            Ok(())
        })
    }

    fn aliases(&self) -> anyhow::Result<std::collections::BTreeMap<AliasTarget, String>> {
        block_async_in_place(async {
            let mut aliases = std::collections::BTreeMap::new();
            for (id, alias) in
                sqlx::query_as::<_, (Uuid, String)>("SELECT id, alias FROM person_aliases")
                    .fetch_all(&self.pool)
                    .await?
            {
                aliases.insert(AliasTarget::Person(id), alias);
            }
            for (id, alias) in
                sqlx::query_as::<_, (ChannelId, String)>("SELECT id, alias FROM channel_aliases")
                    .fetch_all(&self.pool)
                    .await?
            {
                aliases.insert(AliasTarget::Channel(id), alias);
            }
            Ok(aliases)
        })
    }

    fn set_alias(&mut self, target: AliasTarget, alias: Option<&str>) -> anyhow::Result<()> {
        let id = match target {
            AliasTarget::Person(id) => ChannelId::User(id),
            AliasTarget::Channel(id) => id,
        };
        let sql = match (target, alias.is_some()) {
            (AliasTarget::Person(_), true) => {
                "INSERT INTO person_aliases(id, alias) VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET alias = excluded.alias"
            }
            (AliasTarget::Channel(_), true) => {
                "INSERT INTO channel_aliases(id, alias) VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET alias = excluded.alias"
            }
            (AliasTarget::Person(_), false) => "DELETE FROM person_aliases WHERE id = ?",
            (AliasTarget::Channel(_), false) => "DELETE FROM channel_aliases WHERE id = ?",
        };
        block_async_in_place(async {
            let query = sqlx::query(sql).bind(&id);
            if let Some(alias) = alias {
                query.bind(alias).execute(&self.pool).await?;
            } else {
                query.execute(&self.pool).await?;
            }
            Ok(())
        })
    }

    fn channels(&self) -> Vec<Channel> {
        let channels = block_async_in_place(
            query_as!(
                SqlChannel,
                r#"
                    SELECT
                         c.id AS "id: _",
                         c.name,
                         c.group_master_key,
                         c.group_revision,
                         c.group_members AS "group_members: _",
                         c.muted AS "muted: _",
                         c.expire_timer
                    FROM channels c
                    ORDER BY (
                        SELECT MAX(m.arrived_at)
                        FROM messages m
                        WHERE m.channel_id = c.id AND m.edit IS NULL
                    ) DESC
                "#
            )
            .fetch_all(&self.pool),
        );
        let mut channels: Vec<_> = channels
            .ok_logged()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|channel| channel.convert().ok_logged())
            .collect();
        channels.sort_by_key(|c| {
            std::cmp::Reverse(self.sent_page(c.id, None, 1).last().map(|m| m.arrived_at))
        });
        channels
    }

    fn channel(&self, channel_id: ChannelId) -> Option<Channel> {
        let channel_id = &channel_id;
        let channel = block_async_in_place(
            query_as!(
                SqlChannel,
                r#"
                        SELECT
                            id AS "id: _",
                            name,
                            group_master_key,
                            group_revision,
                            group_members AS "group_members: _",
                            muted AS "muted: _",
                            expire_timer
                        FROM channels
                        WHERE id = ?
                    "#,
                channel_id
            )
            .fetch_optional(&self.pool),
        )
        .ok_logged()?;
        channel?.convert().ok_logged()
    }

    fn store_channel(&mut self, channel: &Channel) {
        let id = &channel.id;
        let name = &channel.name;
        let (group_master_key, group_revision, group_members) = channel
            .group_data
            .as_ref()
            .map(|group_data| {
                (
                    Some(&group_data.master_key_bytes[..]),
                    Some(group_data.revision),
                    Some(BlobData(group_data.members.as_slice())),
                )
            })
            .unwrap_or_default();
        let muted = channel.muted;
        let expire_timer: Option<i64> = channel.expire_timer.map(|t| t as i64);
        block_async_in_place(
            query!(
                r#"
                    INSERT INTO channels(
                        id,
                        name,
                        group_master_key,
                        group_revision,
                        group_members,
                        muted,
                        expire_timer
                    )
                    VALUES (?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(id) DO UPDATE SET
                        name = excluded.name,
                        group_master_key = excluded.group_master_key,
                        group_revision = excluded.group_revision,
                        group_members = excluded.group_members,
                        muted = excluded.muted,
                        expire_timer = excluded.expire_timer
                "#,
                id,
                name,
                group_master_key,
                group_revision,
                group_members,
                muted,
                expire_timer
            )
            .execute(&self.pool),
        )
        .ok_logged();
    }

    fn set_message_order(&mut self, order: crate::config::MessageOrder) {
        self.message_order = order;
    }

    fn message_sort_key(&self, id: MessageId) -> u64 {
        if self.message_order == crate::config::MessageOrder::Sent {
            return self.message_sort_time(id);
        }
        block_async_in_place(sqlx::query_scalar::<_, i64>(
            "SELECT display_order FROM message_display_order WHERE channel_id = ? AND arrived_at = ?"
        ).bind(&id.channel_id).bind(id.arrived_at as i64).fetch_optional(&self.pool))
            .ok_logged().flatten().map(|n| n as u64).unwrap_or(0)
    }

    fn search_messages(
        &self,
        channel: ChannelId,
        query: &str,
    ) -> anyhow::Result<Vec<crate::search::SearchHit>> {
        let Some(regex) = crate::search::literal_regex(query)? else {
            return Ok(Vec::new());
        };
        let rows = match self.message_order {
            crate::config::MessageOrder::Sent => block_async_in_place(sqlx::query_as::<_, (i64, String)>(
                "SELECT m.arrived_at,m.message FROM messages m JOIN message_identity i ON i.channel_id=m.channel_id AND i.local_id=m.arrived_at WHERE m.channel_id=? AND m.edit IS NULL AND i.is_snapshot=0 AND m.deleted=0 AND m.message IS NOT NULL ORDER BY i.protocol_time,m.arrived_at"
            ).bind(&channel).fetch_all(&self.pool))?,
            crate::config::MessageOrder::Received => block_async_in_place(sqlx::query_as::<_, (i64, String)>(
                "SELECT m.arrived_at,m.message FROM messages m JOIN message_display_order o ON o.channel_id=m.channel_id AND o.arrived_at=m.arrived_at JOIN message_identity i ON i.channel_id=m.channel_id AND i.local_id=m.arrived_at WHERE m.channel_id=? AND m.edit IS NULL AND i.is_snapshot=0 AND m.deleted=0 AND m.message IS NOT NULL ORDER BY o.display_order,m.arrived_at"
            ).bind(&channel).fetch_all(&self.pool))?,
        };
        let mut hits = Vec::new();
        for (local_id, body) in rows {
            let body = crate::search::normalize_body(&body);
            hits.extend(
                regex
                    .find_iter(&body)
                    .enumerate()
                    .map(|(occurrence, found)| crate::search::SearchHit {
                        message_id: MessageId::new(channel, local_id as u64),
                        range: found.range(),
                        occurrence,
                    }),
            );
        }
        Ok(hits)
    }

    fn oldest_message(&self, channel: ChannelId) -> Option<Message> {
        let id = match self.message_order {
            crate::config::MessageOrder::Sent => block_async_in_place(sqlx::query_scalar::<_, i64>(
                "SELECT m.arrived_at FROM messages m JOIN message_identity i ON i.channel_id=m.channel_id AND i.local_id=m.arrived_at WHERE m.channel_id=? AND m.edit IS NULL ORDER BY i.protocol_time,m.arrived_at LIMIT 1"
            ).bind(&channel).fetch_optional(&self.pool)),
            crate::config::MessageOrder::Received => block_async_in_place(sqlx::query_scalar::<_, i64>(
                "SELECT m.arrived_at FROM messages m JOIN message_display_order o ON o.channel_id=m.channel_id AND o.arrived_at=m.arrived_at WHERE m.channel_id=? AND m.edit IS NULL ORDER BY o.display_order,m.arrived_at LIMIT 1"
            ).bind(&channel).fetch_optional(&self.pool)),
        }.ok_logged()??;
        self.message(MessageId::new(channel, id as u64))
    }

    fn messages_tail(&self, channel_id: ChannelId, limit: usize) -> Vec<Message> {
        if self.message_order == crate::config::MessageOrder::Received {
            return self.received_page(channel_id, None, true, limit);
        }
        self.sent_page(channel_id, None, limit)
    }

    fn messages_before(&self, channel_id: ChannelId, anchor: u64, limit: usize) -> Vec<Message> {
        if self.message_order == crate::config::MessageOrder::Received {
            return self.received_page(channel_id, Some((false, anchor)), true, limit);
        }
        self.sent_page(channel_id, Some((false, anchor)), limit)
    }

    fn messages_after(&self, channel_id: ChannelId, anchor: u64, limit: usize) -> Vec<Message> {
        if self.message_order == crate::config::MessageOrder::Received {
            return self.received_page(channel_id, Some((true, anchor)), false, limit);
        }
        self.sent_page(channel_id, Some((true, anchor)), limit)
    }

    fn messages(&self, channel_id: ChannelId) -> Box<dyn DoubleEndedIterator<Item = Message> + '_> {
        Box::new(self.sent_page(channel_id, None, usize::MAX).into_iter())
    }

    fn message_by_protocol(
        &self,
        channel: ChannelId,
        sender: Uuid,
        timestamp: u64,
    ) -> Option<Message> {
        let timestamp = i64::try_from(timestamp).ok()?;
        let id = block_async_in_place(sqlx::query_scalar::<_, i64>("SELECT local_id FROM message_identity WHERE channel_id=? AND from_id=? AND protocol_time=? AND is_snapshot=0")
            .bind(&channel).bind(sender).bind(timestamp).fetch_optional(&self.pool)).ok_logged()??;
        self.message(MessageId::new(channel, id as u64))
    }

    fn message(&self, message_id: MessageId) -> Option<Message> {
        let channel_id = &message_id.channel_id;
        let arrived_at: i64 = message_id
            .arrived_at
            .try_into()
            .map_err(|_| MessageConvertError::InvalidTimestamp)
            .ok_logged()?;
        let message = block_async_in_place(
            query_as!(
                SqlMessage,
                r#"
                    SELECT
                        m.arrived_at,
                        m.from_id AS "from_id: _",
                        m.message,
                        m.receipt AS "receipt: _",
                        m.body_ranges AS "body_ranges: _",
                        m.attachments AS "attachments: _",
                        m.reactions AS "reactions: _",
                        q.arrived_at AS "quote_arrived_at: _",
                        q.from_id AS "quote_from_id: _",
                        q.message AS quote_message,
                        q.attachments AS "quote_attachments: _",
                        q.body_ranges AS "quote_body_ranges: _",
                        q.receipt AS "quote_receipt: _",
                        m.edit,
                        m.edited as "edited: _",
                        m.deleted as "deleted: _",
                        m.expire_timer as "expire_timer: _",
                        m.expires_at as "expires_at: _"
                    FROM messages AS m
                    LEFT JOIN messages AS q ON q.arrived_at = m.quote AND q.channel_id = ?1
                    WHERE m.channel_id = ?1 AND m.arrived_at = ?2
                    GROUP BY m.arrived_at
                    LIMIT 1
                "#,
                channel_id,
                arrived_at
            )
            .fetch_optional(&self.pool),
        );
        let message = message.ok_logged()??.convert().ok_logged()?;
        Some(self.restore_identity(message_id.channel_id, message))
    }

    fn edits(&self, message_id: MessageId) -> Box<dyn DoubleEndedIterator<Item = Message> + '_> {
        let channel_id = &message_id.channel_id;
        let arrived_at: Option<i64> = message_id
            .arrived_at
            .try_into()
            .map_err(|_| MessageConvertError::InvalidTimestamp)
            .ok_logged();
        let Some(arrived_at) = arrived_at else {
            return Box::new(std::iter::empty());
        };
        let messages = block_async_in_place(
            query_as!(
                SqlMessage,
                r#"
                    SELECT
                        m.arrived_at AS "arrived_at!",
                        m.from_id AS "from_id: _",
                        m.message,
                        m.receipt AS "receipt: _",
                        m.body_ranges AS "body_ranges: _",
                        m.attachments AS "attachments: _",
                        m.reactions AS "reactions: _",
                        q.arrived_at AS "quote_arrived_at: _",
                        q.from_id AS "quote_from_id: _",
                        q.message AS quote_message,
                        q.attachments AS "quote_attachments: _",
                        q.body_ranges AS "quote_body_ranges: _",
                        q.receipt AS "quote_receipt: _",
                        NULL AS "edit: _",
                        m.edited AS "edited: _",
                        m.deleted AS "deleted: _",
                        m.expire_timer AS "expire_timer: _",
                        m.expires_at AS "expires_at: _"
                    FROM messages AS m
                    LEFT JOIN messages AS q ON q.arrived_at = m.quote AND q.channel_id = ?1
                    WHERE m.channel_id = ?1 AND m.edit == ?2
                    ORDER BY m.arrived_at ASC
                "#,
                channel_id,
                arrived_at,
            )
            .fetch_all(&self.pool),
        );
        let channel_id = *channel_id;
        let mut messages: Vec<_> = messages
            .ok_logged()
            .into_iter()
            .flatten()
            .filter_map(|message| {
                message
                    .convert()
                    .ok_logged()
                    .map(|m| self.restore_identity(channel_id, m))
            })
            .collect();
        messages.sort_by_key(|message| (message.arrived_at, message.storage_id()));
        Box::new(messages.into_iter())
    }

    fn messages_count_after(&self, channel_id: ChannelId, arrived_at: u64) -> usize {
        let time = self.message_sort_time(MessageId::new(channel_id, arrived_at));
        block_async_in_place(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages m JOIN message_identity i ON i.channel_id=m.channel_id AND i.local_id=m.arrived_at WHERE m.channel_id=? AND m.edit IS NULL AND (i.protocol_time,m.arrived_at) > (?,?)")
            .bind(&channel_id).bind(time as i64).bind(arrived_at as i64).fetch_one(&self.pool)).ok_logged().and_then(|n| usize::try_from(n).ok()).unwrap_or_default()
    }

    fn remove_expired(&self, now_ms: u64) -> Vec<MessageId> {
        let now_ms = i64::try_from(now_ms).expect("u64 overflow");
        block_async_in_place(
            query_as!(
                MessageId,
                r#"
                    DELETE FROM messages
                    WHERE expires_at IS NOT NULL AND expires_at <= ?1
                    RETURNING
                        channel_id AS "channel_id: _",
                        arrived_at AS "arrived_at: _"
                "#,
                now_ms,
            )
            .fetch_all(&self.pool),
        )
        .ok_logged()
        .unwrap_or_default()
    }

    fn next_expiring_at(&self) -> Option<u64> {
        block_async_in_place(
            query_scalar!(
                r#"
                    SELECT MIN(expires_at) AS "next_expiring_at: u64"
                    FROM messages
                    WHERE expires_at IS NOT NULL
                "#,
            )
            .fetch_optional(&self.pool),
        )
        .ok_logged()??
    }

    fn store_message(&mut self, channel_id: ChannelId, message: &Message) {
        let channel_id = &channel_id;
        let protocol_time: i64 = message
            .arrived_at
            .try_into()
            .map_err(|_| MessageConvertError::InvalidTimestamp)
            .ok_logged()
            .unwrap();
        let from_id = &message.from_id;
        let message_msg = message.message.as_deref();
        let quote: Option<i64> = message.quote.as_ref().and_then(|quote| {
            quote
                .arrived_at
                .try_into()
                .map_err(|_| MessageConvertError::InvalidTimestamp)
                .ok_logged()
        });
        let receipt = BlobData(&message.receipt);
        let body_ranges = BlobData(&message.body_ranges);
        let attachments = BlobData(&message.attachments);
        let reactions = BlobData(&message.reactions);
        let edit: Option<i64> = message.edit.and_then(|edit| {
            edit.try_into()
                .map_err(|_| MessageConvertError::InvalidTimestamp)
                .ok_logged()
        });
        let edited: bool = message.edited;
        let deleted: bool = message.deleted;
        let expire_timer: Option<i64> = message.expire_timer.map(|t| t as i64);
        let expires_at: Option<i64> = message.expires_at.and_then(|t| i64::try_from(t).ok());
        let inserted: anyhow::Result<()> = block_async_in_place(async {
            let mut tx = self.pool.begin().await?;
            let existing: Option<i64> = sqlx::query_scalar("SELECT local_id FROM message_identity WHERE channel_id=? AND from_id=? AND protocol_time=? AND is_snapshot=? ORDER BY local_id LIMIT 1")
                .bind(channel_id).bind(from_id).bind(protocol_time).bind(message.is_edit_snapshot).fetch_optional(&mut *tx).await?;
            if let (Some(existing), Some(explicit)) = (existing, message.local_id) {
                anyhow::ensure!(
                    existing == i64::try_from(explicit)?,
                    "explicit local ID disagrees with existing protocol identity"
                );
            }
            let mut arrived_at = if let Some(id) = existing {
                id
            } else {
                i64::try_from(message.storage_id())?
            };
            let owner: Option<(Uuid, i64, bool)> = sqlx::query_as("SELECT from_id, protocol_time, is_snapshot FROM message_identity WHERE channel_id=? AND local_id=?")
                .bind(channel_id).bind(arrived_at).fetch_optional(&mut *tx).await?;
            if owner
                .is_some_and(|owner| owner != (*from_id, protocol_time, message.is_edit_snapshot))
            {
                anyhow::ensure!(
                    message.local_id.is_none(),
                    "explicit local ID belongs to another message"
                );
                arrived_at = i64::MAX;
                while sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE channel_id=? AND arrived_at=?)",
                )
                .bind(channel_id)
                .bind(arrived_at)
                .fetch_one(&mut *tx)
                .await?
                {
                    arrived_at = arrived_at
                        .checked_sub(1)
                        .ok_or_else(|| anyhow::anyhow!("message IDs exhausted"))?;
                }
            }
            // Quote joins use local IDs; the snapshot also preserves unresolved authors and timestamps.
            let quote = if let Some(q) = message.quote.as_ref() {
                sqlx::query_scalar::<_, i64>("SELECT local_id FROM message_identity WHERE channel_id=? AND from_id=? AND protocol_time=? AND is_snapshot=0")
                    .bind(channel_id).bind(q.from_id).bind(i64::try_from(q.arrived_at)?).fetch_optional(&mut *tx).await?
            } else {
                quote
            };
            query!(
                "
                    INSERT INTO messages(
                        arrived_at,
                        channel_id,
                        from_id,
                        message,
                        quote,
                        receipt,
                        body_ranges,
                        attachments,
                        reactions,
                        edit,
                        edited,
                        deleted,
                        expire_timer,
                        expires_at
                    )
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(channel_id, arrived_at) DO UPDATE SET
                        from_id = excluded.from_id,
                        message = excluded.message,
                        quote = excluded.quote,
                        receipt = excluded.receipt,
                        body_ranges = excluded.body_ranges,
                        attachments = excluded.attachments,
                        reactions = excluded.reactions,
                        edit = excluded.edit,
                        edited = excluded.edited,
                        deleted = excluded.deleted,
                        expire_timer = excluded.expire_timer,
                        expires_at = excluded.expires_at
                ",
                arrived_at,
                channel_id,
                from_id,
                message_msg,
                quote,
                receipt,
                body_ranges,
                attachments,
                reactions,
                edit,
                edited,
                deleted,
                expire_timer,
                expires_at
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO message_identity(channel_id,local_id,from_id,protocol_time,quote_snapshot,is_snapshot) VALUES(?,?,?,?,?,?) ON CONFLICT(channel_id,local_id) DO UPDATE SET quote_snapshot=excluded.quote_snapshot")
                .bind(channel_id).bind(arrived_at).bind(from_id).bind(protocol_time).bind(message.quote.as_deref().map(BlobData)).bind(message.is_edit_snapshot).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(())
        });
        if let Err(error) = inserted {
            error!(%error, ?channel_id, protocol_time, "failed to store message");
        }
    }

    fn remove_message(&mut self, message_id: MessageId) {
        let channel_id = &message_id.channel_id;
        let arrived_at: Option<i64> = message_id
            .arrived_at
            .try_into()
            .map_err(|_| MessageConvertError::InvalidTimestamp)
            .ok_logged();
        let Some(arrived_at) = arrived_at else {
            return;
        };
        let result = block_async_in_place(
            query!(
                "DELETE FROM messages WHERE channel_id = ? AND arrived_at = ?",
                channel_id,
                arrived_at
            )
            .execute(&self.pool),
        );
        result.ok_logged();
    }

    fn names(&self) -> Box<dyn Iterator<Item = (Uuid, String)> + '_> {
        let names = block_async_in_place(
            query_as!(
                SqlName,
                r#"SELECT id AS "id: _", name AS "name: _" FROM names"#
            )
            .fetch_all(&self.pool),
        );
        let names = names
            .ok_logged()
            .into_iter()
            .flatten()
            .map(|SqlName { id, name }| (id, name));
        Box::new(names)
    }

    fn name(&self, id: Uuid) -> Option<String> {
        let name = block_async_in_place(
            query_scalar!(r#"SELECT name AS "name: _" FROM names WHERE id = ?"#, id)
                .fetch_optional(&self.pool),
        );
        name.ok_logged()?
    }

    fn store_name(&mut self, id: Uuid, name: &str) {
        block_async_in_place(
            query!(
                "
                    INSERT INTO names(
                        id, name
                    ) VALUES (?, ?)
                    ON CONFLICT(id) DO UPDATE SET
                        name = excluded.name
                ",
                id,
                name
            )
            .execute(&self.pool),
        )
        .ok_logged();
    }

    fn metadata(&self) -> Metadata {
        let metadata = block_async_in_place(
            query_as!(
                Metadata,
                r#"
                    SELECT
                        contacts_sync_request_at AS "contacts_sync_request_at: _",
                        fully_migrated AS "fully_migrated: _"
                    FROM metadata WHERE id = 0 LIMIT 1
                "#,
            )
            .fetch_optional(&self.pool),
        );
        metadata.ok_logged().flatten().unwrap_or_default()
    }

    fn store_metadata(&mut self, metadata: &Metadata) {
        block_async_in_place(
            query!(
                "REPLACE INTO metadata(id, contacts_sync_request_at, fully_migrated)
                     VALUES (?, ?, ?)",
                METADATA_ID,
                metadata.contacts_sync_request_at,
                metadata.fully_migrated
            )
            .execute(&self.pool),
        )
        .ok_logged();
    }

    fn save(&mut self) {}
}

/// Runs and waits for the given future to complete in a synchronous context.
///
/// Note that even though this function is called in a synchronous context, at some point down the
/// stack it must be called in a multi-threaded asynchronous context. In particular, tests must be
/// asynchronous and of flavor `multi_thread`.
fn block_async_in_place<F>(task: F) -> F::Output
where
    F: Future,
{
    task::block_in_place(|| Handle::current().block_on(task))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;
    use uuid::uuid;

    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn search_full_history_literals_visibility_order_and_collisions() {
        use crate::config::MessageOrder;
        let mut storage = SqliteStorage::open_unencrypted(&"sqlite::memory:".parse().unwrap())
            .await
            .unwrap();
        let channel = ChannelId::User(Uuid::nil());
        let other = ChannelId::User(Uuid::from_u128(9));
        let alice = Uuid::from_u128(1);
        let bob = Uuid::from_u128(2);
        storage.store_channel(&test_channel(channel, "Search"));
        storage.store_channel(&test_channel(other, "Other"));
        storage.store_message(
            channel,
            &Message::text(alice, 100, "ŻÓŁW %_[a].* 👩‍💻 żółw".into()),
        );
        storage.store_message(channel, &Message::text(bob, 100, "żółw".into()));
        let collision = storage
            .message_by_protocol(channel, bob, 100)
            .unwrap()
            .storage_id();
        storage.store_message(channel, &Message::text(alice, 50, "żółw".into()));
        for timestamp in 101..615 {
            storage.store_message(channel, &Message::text(alice, timestamp, "filler".into()));
        }
        storage.store_message(other, &Message::text(alice, 1, "żółw".into()));
        let mut deleted = Message::text(alice, 900, "żółw".into());
        deleted.deleted = true;
        storage.store_message(channel, &deleted);
        let mut edit = Message::text(alice, 901, "żółw".into());
        edit.edit = Some(100);
        storage.store_message(channel, &edit);
        let mut snapshot = Message::text(alice, 902, "żółw".into());
        snapshot.is_edit_snapshot = true;
        snapshot.edit = Some(100);
        storage.store_message(channel, &snapshot);
        let hits = storage.search_messages(channel, "żółw").unwrap();
        assert_eq!(hits.len(), 4);
        assert_eq!(
            hits.iter()
                .map(|hit| hit.message_id.arrived_at)
                .collect::<Vec<_>>(),
            [50, 100, 100, collision]
        );
        assert_eq!(hits[2].occurrence, 1);
        assert_eq!(hits[1].range, 0..7);
        for literal in ["%", "_", "[a].*", "👩‍💻"] {
            assert_eq!(storage.search_messages(channel, literal).unwrap().len(), 1);
        }
        assert!(storage.search_messages(channel, "").unwrap().is_empty());
        assert!(
            storage
                .search_messages(channel, "absent")
                .unwrap()
                .is_empty()
        );
        assert_eq!(storage.oldest_message(channel).unwrap().storage_id(), 50);
        storage.set_message_order(MessageOrder::Received);
        let hits = storage.search_messages(channel, "żółw").unwrap();
        assert_eq!(
            hits.iter()
                .map(|hit| hit.message_id.arrived_at)
                .collect::<Vec<_>>(),
            [100, 100, collision, 50]
        );
        assert_eq!(storage.oldest_message(channel).unwrap().storage_id(), 100);
    }

    // Historical migration fixtures must seed the schema that existed before identity metadata.
    async fn seed_legacy_message(
        pool: &SqlitePool,
        channel: ChannelId,
        timestamp: i64,
        text: &str,
    ) {
        sqlx::query("INSERT INTO messages(channel_id,arrived_at,from_id,message) VALUES(?,?,?,?)")
            .bind(&channel)
            .bind(timestamp)
            .bind(Uuid::nil())
            .bind(text)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn edit_snapshot_does_not_overwrite_adjacent_protocol_message() {
        let mut storage = SqliteStorage::open_unencrypted(&"sqlite::memory:".parse().unwrap())
            .await
            .unwrap();
        let channel = ChannelId::User(Uuid::nil());
        storage.store_channel(&test_channel(channel, "Snapshots"));
        storage.store_message(channel, &Message::text(Uuid::nil(), 100, "original".into()));
        storage.store_message(channel, &Message::text(Uuid::nil(), 101, "adjacent".into()));
        let mut snapshot = storage
            .message_by_protocol(channel, Uuid::nil(), 100)
            .unwrap();
        snapshot.local_id = None;
        snapshot.is_edit_snapshot = true;
        snapshot.edit = Some(100);
        storage.store_message(channel, &snapshot);
        assert_eq!(
            storage
                .message_by_protocol(channel, Uuid::nil(), 100)
                .unwrap()
                .message
                .as_deref(),
            Some("original")
        );
        assert_eq!(
            storage
                .message_by_protocol(channel, Uuid::nil(), 101)
                .unwrap()
                .message
                .as_deref(),
            Some("adjacent")
        );
        let edits: Vec<_> = storage.edits(MessageId::new(channel, 100)).collect();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].arrived_at, 100);
        assert!(edits[0].is_edit_snapshot);
        assert_ne!(edits[0].storage_id(), 100);
        assert_eq!(storage.messages(channel).count(), 2);
        storage.store_message(channel, &snapshot);
        assert_eq!(storage.edits(MessageId::new(channel, 100)).count(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn collisions_preserve_protocol_identity_quotes_pages_and_reopen() {
        let dir = tempdir().unwrap();
        let url: Url = format!(
            "sqlite://{}",
            dir.path().join("collisions.sqlite").display()
        )
        .parse()
        .unwrap();
        let channel = ChannelId::User(Uuid::nil());
        let alice = Uuid::from_u128(1);
        let bob = Uuid::from_u128(2);
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        storage.store_channel(&test_channel(channel, "Collisions"));
        storage.store_message(channel, &Message::text(alice, 100, "alice".into()));
        storage.store_message(channel, &Message::text(bob, 100, "bob".into()));
        let second = storage.message_by_protocol(channel, bob, 100).unwrap();
        let second_id = second.storage_id();
        assert_ne!(second_id, 100);
        assert_eq!(second.arrived_at, 100);
        assert_eq!(
            storage
                .message_by_protocol(channel, alice, 100)
                .unwrap()
                .message
                .as_deref(),
            Some("alice")
        );
        storage.store_message(channel, &Message::text(bob, 100, "bob updated".into()));
        assert_eq!(storage.messages(channel).count(), 2);
        assert_eq!(
            storage
                .message_by_protocol(channel, bob, 100)
                .unwrap()
                .storage_id(),
            second_id
        );
        let mut reply = Message::text(alice, 110, "reply".into());
        reply.quote = Some(Box::new(second));
        storage.store_message(channel, &reply);
        let mut unresolved = Message::text(alice, 120, "unresolved".into());
        unresolved.quote = Some(Box::new(Message::text(bob, 99, "missing".into())));
        storage.store_message(channel, &unresolved);
        assert_eq!(
            storage.messages_after(channel, 100, 1)[0].storage_id(),
            second_id
        );
        assert_eq!(
            storage.messages_before(channel, second_id, 1)[0].storage_id(),
            100
        );
        assert_eq!(
            storage.messages_after(channel, second_id, 1)[0].arrived_at,
            110
        );
        assert_eq!(storage.messages_count_after(channel, second_id), 2);
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel, 4)),
            [100, 100, 110, 120]
        );
        storage.pool.close().await;
        drop(storage);
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        let reply = storage.message(MessageId::new(channel, 110)).unwrap();
        assert_eq!(reply.quote.as_ref().unwrap().arrived_at, 100);
        assert_eq!(reply.quote.as_ref().unwrap().from_id, bob);
        let unresolved = storage.message(MessageId::new(channel, 120)).unwrap();
        assert_eq!(unresolved.quote.as_ref().unwrap().arrived_at, 99);
        assert_eq!(unresolved.quote.as_ref().unwrap().from_id, bob);
        let mut second = storage.message_by_protocol(channel, bob, 100).unwrap();
        second.expires_at = Some(200);
        storage.store_message(channel, &second);
        assert_eq!(
            storage.remove_expired(200),
            [MessageId::new(channel, second_id)]
        );
        assert!(storage.message_by_protocol(channel, alice, 100).is_some());
        assert!(storage.message_by_protocol(channel, bob, 100).is_none());
        let mut explicit = Message::text(bob, 130, "explicit".into());
        explicit.local_id = Some(500);
        storage.store_message(channel, &explicit);
        assert_eq!(
            storage
                .message_by_protocol(channel, bob, 130)
                .unwrap()
                .storage_id(),
            500
        );
        let mut conflicting = Message::text(alice, 140, "must not overwrite".into());
        conflicting.local_id = Some(500);
        storage.store_message(channel, &conflicting);
        assert_eq!(
            storage
                .message(MessageId::new(channel, 500))
                .unwrap()
                .message
                .as_deref(),
            Some("explicit")
        );
        assert!(storage.message_by_protocol(channel, alice, 140).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn editing_original_repeatedly_preserves_snapshot_and_adjacent_real_message() {
        let mut storage = SqliteStorage::open_unencrypted(&"sqlite::memory:".parse().unwrap())
            .await
            .unwrap();
        let channel = ChannelId::User(Uuid::nil());
        let sender = Uuid::from_u128(1);
        storage.store_channel(&test_channel(channel, "edits"));
        storage.store_message(channel, &Message::text(sender, 100, "original".into()));
        storage.store_message(channel, &Message::text(sender, 101, "neighbor".into()));
        storage
            .store_edited_message(channel, 100, Message::text(sender, 200, "edit one".into()))
            .unwrap();
        storage
            .store_edited_message(channel, 100, Message::text(sender, 201, "edit two".into()))
            .unwrap();
        assert_eq!(
            storage
                .message_by_protocol(channel, sender, 101)
                .unwrap()
                .message
                .as_deref(),
            Some("neighbor")
        );
        assert_eq!(
            storage
                .message_by_protocol(channel, sender, 100)
                .unwrap()
                .message
                .as_deref(),
            Some("edit two")
        );
        let history: Vec<_> = storage.edits(MessageId::new(channel, 100)).collect();
        let snapshots: Vec<_> = history.iter().filter(|m| m.is_edit_snapshot).collect();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].message.as_deref(), Some("original"));
        assert_eq!(snapshots[0].arrived_at, 100);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn identity_migration_backfills_existing_rows() {
        let opts: SqliteConnectOptions = "sqlite::memory:".parse().unwrap();
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        // Apply the pre-identity schema with the same production migrations.
        let mut old = sqlx::migrate!();
        old.migrations.to_mut().retain(|m| m.version < 12);
        old.run(&pool).await.unwrap();
        let channel = ChannelId::User(Uuid::nil());
        sqlx::query("INSERT INTO channels(id,name) VALUES(?,?)")
            .bind(&channel)
            .bind("legacy")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages(channel_id,arrived_at,from_id,message) VALUES(?,?,?,?)")
            .bind(&channel)
            .bind(100_i64)
            .bind(Uuid::nil())
            .bind("legacy")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/012_message_identity.up.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let row: (i64, i64, Uuid) =
            sqlx::query_as("SELECT local_id,protocol_time,from_id FROM message_identity")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row, (100, 100, Uuid::nil()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn received_order_survives_skew_updates_edits_and_reopen() {
        use crate::config::MessageOrder;
        let dir = tempdir().unwrap();
        let url: Url = format!("sqlite://{}", dir.path().join("order.sqlite").display())
            .parse()
            .unwrap();
        let channel = ChannelId::User(Uuid::nil());
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        storage.store_channel(&test_channel(channel, "Order"));
        for at in [300, 100, 200] {
            storage.store_message(channel, &Message::text(Uuid::nil(), at, at.to_string()));
        }
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel, 3)),
            [100, 200, 300]
        );
        storage.set_message_order(MessageOrder::Received);
        assert_eq!(arrived_ats(&storage.messages_tail(channel, 2)), [100, 200]);
        assert_eq!(
            arrived_ats(&storage.messages_before(channel, 200, 2)),
            [300, 100]
        );
        assert_eq!(
            arrived_ats(&storage.messages_after(channel, 300, 2)),
            [100, 200]
        );
        assert_eq!(
            arrived_ats(&storage.messages(channel).collect::<Vec<_>>()),
            [100, 200, 300]
        );
        assert_eq!(storage.messages_count_after(channel, 100), 2);
        let key = storage.message_sort_key(MessageId::new(channel, 100));
        storage.store_message(channel, &Message::text(Uuid::nil(), 100, "Updated".into()));
        storage
            .store_edited_message(
                channel,
                100,
                Message::text(Uuid::nil(), 400, "Edited".into()),
            )
            .unwrap();
        assert_eq!(storage.message_sort_key(MessageId::new(channel, 100)), key);
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel, 10)),
            [300, 100, 200]
        );
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        storage.set_message_order(MessageOrder::Received);
        storage.store_message(channel, &Message::text(Uuid::nil(), 50, "Late".into()));
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel, 10)),
            [300, 100, 200, 50]
        );
        storage.set_message_order(MessageOrder::Sent);
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel, 10)),
            [50, 100, 200, 300]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn display_order_migration_backfills_history_chronologically() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let mut old = sqlx::migrate!();
        old.migrations.to_mut().retain(|m| m.version < 10);
        old.run(&pool).await.unwrap();
        let mut storage = SqliteStorage {
            pool,
            message_order: Default::default(),
        };
        let channel = ChannelId::User(Uuid::nil());
        storage.store_channel(&test_channel(channel, "Legacy"));
        for at in [300, 100, 200] {
            seed_legacy_message(&storage.pool, channel, at, "Saved").await;
        }
        sqlx::migrate!().run(&storage.pool).await.unwrap();
        storage.set_message_order(crate::config::MessageOrder::Received);
        storage.store_message(channel, &Message::text(Uuid::nil(), 50, "New".into()));
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel, 10)),
            [100, 200, 300, 50]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn private_read_sync_outbox_is_durable_and_idempotent() {
        let dir = tempdir().unwrap();
        let url: Url = format!("sqlite://{}", dir.path().join("outbox.sqlite").display())
            .parse()
            .unwrap();
        let sender = Uuid::nil();
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert!(!storage.read_sync_completed(sender, 42).unwrap());
        storage.set_read_sync_pending(sender, 42, true).unwrap();
        storage.set_read_sync_pending(sender, 42, true).unwrap();
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.pending_read_sync().unwrap(), [(sender, 42)]);
        storage.set_read_sync_pending(sender, 42, false).unwrap();
        storage.set_read_sync_pending(sender, 42, false).unwrap();
        assert!(storage.pending_read_sync().unwrap().is_empty());
        assert!(storage.read_sync_completed(sender, 42).unwrap());
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert!(storage.read_sync_completed(sender, 42).unwrap());
        storage.set_read_sync_pending(sender, 42, true).unwrap();
        assert!(storage.pending_read_sync().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn private_read_sync_ack_failure_preserves_pending() {
        let mut storage = SqliteStorage::open_unencrypted(&"sqlite::memory:".parse().unwrap())
            .await
            .unwrap();
        let sender = Uuid::nil();
        storage.set_read_sync_pending(sender, 42, true).unwrap();
        sqlx::query("CREATE TRIGGER fail_pending_delete BEFORE DELETE ON private_read_sync_outbox BEGIN SELECT RAISE(ABORT, 'test failure'); END")
            .execute(&storage.pool).await.unwrap();
        assert!(storage.set_read_sync_pending(sender, 42, false).is_err());
        assert_eq!(storage.pending_read_sync().unwrap(), [(sender, 42)]);
        assert!(!storage.read_sync_completed(sender, 42).unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn favorites_migrate_legacy_history_and_survive_reopen() {
        let dir = tempdir().unwrap();
        let url: Url = format!("sqlite://{}", dir.path().join("favorites.sqlite").display())
            .parse()
            .unwrap();
        let pool = SqlitePool::connect_with(
            url.as_str()
                .parse::<SqliteConnectOptions>()
                .unwrap()
                .create_if_missing(true),
        )
        .await
        .unwrap();
        let mut old = sqlx::migrate!();
        old.migrations.to_mut().retain(|m| m.version < 14);
        old.run(&pool).await.unwrap();
        let mut storage = SqliteStorage {
            pool,
            message_order: Default::default(),
        };
        let person = ChannelId::User(Uuid::nil());
        let group = ChannelId::Group([17; 32]);
        storage.store_channel(&test_channel(person, "Legacy"));
        storage
            .store_draft(
                person,
                &Draft {
                    text: "Unsent".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        storage
            .set_alias(AliasTarget::Channel(person), Some("Alias"))
            .unwrap();
        seed_legacy_message(&storage.pool, person, 42, "Saved").await;
        storage.set_channel_hidden(group, true).unwrap();
        sqlx::migrate!().run(&storage.pool).await.unwrap();
        assert!(storage.favorite_channels().unwrap().is_empty());
        for id in [person, group, person] {
            storage.set_channel_favorite(id, true).unwrap();
        }
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.favorite_channels().unwrap(), [person, group].into());
        assert_eq!(storage.channel(person).unwrap().name, "Legacy");
        assert_eq!(storage.drafts().unwrap()[&person].text, "Unsent");
        assert_eq!(
            storage.aliases().unwrap()[&AliasTarget::Channel(person)],
            "Alias"
        );
        assert!(storage.message(MessageId::new(person, 42)).is_some());
        assert!(storage.hidden_channels().unwrap().contains(&group));
        storage.set_channel_favorite(person, false).unwrap();
        storage.set_channel_favorite(person, false).unwrap();
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.favorite_channels().unwrap(), [group].into());
        sqlx::query("DROP TABLE favorite_channels")
            .execute(&storage.pool)
            .await
            .unwrap();
        assert!(storage.favorite_channels().is_err());
        assert!(storage.set_channel_favorite(person, true).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hidden_channels_survive_reopen_and_unhide_idempotently() {
        let dir = tempdir().unwrap();
        let url: Url = format!("sqlite://{}", dir.path().join("hidden.sqlite").display())
            .parse()
            .unwrap();
        let person = ChannelId::User(Uuid::nil());
        let group = ChannelId::Group([9; 32]);
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert!(storage.hidden_channels().unwrap().is_empty());
        for channel in [person, group, person] {
            storage.set_channel_hidden(channel, true).unwrap();
        }
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.hidden_channels().unwrap(), [person, group].into());
        storage.set_channel_hidden(person, false).unwrap();
        storage.set_channel_hidden(person, false).unwrap();
        assert_eq!(storage.hidden_channels().unwrap(), [group].into());
        storage.pool.close().await;
        let storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.hidden_channels().unwrap(), [group].into());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hidden_channels_migration_preserves_existing_data() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let mut old = sqlx::migrate!();
        old.migrations.to_mut().retain(|m| m.version < 9);
        old.run(&pool).await.unwrap();
        let mut storage = SqliteStorage {
            pool,
            message_order: Default::default(),
        };
        let channel = ChannelId::User(Uuid::nil());
        let draft = Draft {
            text: "Unsent".into(),
            ..Draft::default()
        };
        storage.store_channel(&test_channel(channel, "Legacy"));
        seed_legacy_message(&storage.pool, channel, 42, "Saved").await;
        storage
            .set_alias(AliasTarget::Person(Uuid::nil()), Some("Friend"))
            .unwrap();
        // Seed the legacy schema through its original columns.
        sqlx::query("INSERT INTO composer_drafts(channel_id, text, cursor, editing, attachments) VALUES (?, ?, ?, ?, ?)")
            .bind(&channel).bind(&draft.text).bind(0_i64).bind(Option::<i64>::None).bind(postcard::to_allocvec(&draft.attachments).unwrap()).execute(&storage.pool).await.unwrap();
        storage.store_last_draft_channel(Some(channel)).unwrap();
        sqlx::migrate!().run(&storage.pool).await.unwrap();
        assert!(storage.hidden_channels().unwrap().is_empty());
        storage.set_channel_hidden(channel, true).unwrap();
        assert_eq!(storage.channel(channel).unwrap().name, "Legacy");
        assert_eq!(
            storage
                .message(MessageId::new(channel, 42))
                .unwrap()
                .message
                .as_deref(),
            Some("Saved")
        );
        assert_eq!(
            storage
                .aliases()
                .unwrap()
                .get(&AliasTarget::Person(Uuid::nil()))
                .map(String::as_str),
            Some("Friend")
        );
        assert_eq!(storage.drafts().unwrap().get(&channel), Some(&draft));
        assert_eq!(storage.last_draft_channel().unwrap(), Some(channel));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hidden_channel_failures_are_reported() {
        let mut storage = SqliteStorage::open_unencrypted(&"sqlite::memory:".parse().unwrap())
            .await
            .unwrap();
        sqlx::query("INSERT INTO hidden_channels(channel_id) VALUES (X'FF')")
            .execute(&storage.pool)
            .await
            .unwrap();
        assert!(storage.hidden_channels().is_err());
        storage.pool.close().await;
        assert!(storage.hidden_channels().is_err());
        let channel = ChannelId::User(Uuid::nil());
        assert!(storage.set_channel_hidden(channel, true).is_err());
        assert!(storage.set_channel_hidden(channel, false).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn composer_drafts_survive_reopen_and_remove_independently() {
        use crate::storage::Draft;
        let dir = tempdir().unwrap();
        let url: Url = format!("sqlite://{}", dir.path().join("drafts.sqlite").display())
            .parse()
            .unwrap();
        let user = ChannelId::User(Uuid::nil());
        let group = ChannelId::Group([7; 32]);
        let draft = Draft {
            text: "Zażółć 🐢\nnext line".into(),
            cursor: 8,
            editing: Some(42),
            attachments: vec!["/tmp/a file 🐢.png".into()],
            mentions: Vec::new(),
        };
        let attachment_only = Draft {
            attachments: vec!["/tmp/another file".into()],
            ..Draft::default()
        };
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert!(storage.drafts().unwrap().is_empty());
        assert_eq!(storage.last_draft_channel().unwrap(), None);
        storage.store_draft(user, &draft).unwrap();
        storage.store_draft(group, &attachment_only).unwrap();
        storage.store_last_draft_channel(Some(group)).unwrap();
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.drafts().unwrap().get(&user), Some(&draft));
        assert_eq!(
            storage.drafts().unwrap().get(&group),
            Some(&attachment_only)
        );
        assert_eq!(storage.last_draft_channel().unwrap(), Some(group));
        storage.store_last_draft_channel(Some(user)).unwrap();
        storage.store_draft(user, &Draft::default()).unwrap();
        assert_eq!(storage.drafts().unwrap().len(), 1);
        assert_eq!(storage.last_draft_channel().unwrap(), Some(user));
        storage.store_last_draft_channel(None).unwrap();
        storage.pool.close().await;
        let storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.drafts().unwrap().len(), 1);
        assert_eq!(storage.last_draft_channel().unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn composer_draft_migration_preserves_legacy_data() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let mut old = sqlx::migrate!();
        old.migrations.to_mut().retain(|m| m.version < 8);
        old.run(&pool).await.unwrap();
        let mut storage = SqliteStorage {
            pool,
            message_order: Default::default(),
        };
        let channel = ChannelId::User(Uuid::nil());
        storage.store_channel(&test_channel(channel, "Legacy"));
        seed_legacy_message(&storage.pool, channel, 42, "Saved").await;
        storage
            .set_alias(AliasTarget::Person(Uuid::nil()), Some("Friend"))
            .unwrap();
        sqlx::migrate!().run(&storage.pool).await.unwrap();
        assert_eq!(storage.channel(channel).unwrap().name, "Legacy");
        assert_eq!(
            storage
                .message(MessageId::new(channel, 42))
                .unwrap()
                .message
                .as_deref(),
            Some("Saved")
        );
        assert_eq!(
            storage
                .aliases()
                .unwrap()
                .get(&AliasTarget::Person(Uuid::nil()))
                .map(String::as_str),
            Some("Friend")
        );
        assert!(storage.drafts().unwrap().is_empty());
        assert_eq!(storage.last_draft_channel().unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn draft_mentions_migrate_legacy_and_survive_reopen() {
        let dir = tempdir().unwrap();
        let url: Url = format!("sqlite://{}", dir.path().join("mentions.sqlite").display())
            .parse()
            .unwrap();
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        let channel = ChannelId::User(Uuid::nil());
        let mut draft = crate::storage::Draft {
            text: "@Bartosz".into(),
            cursor: 8,
            ..Default::default()
        };
        storage.store_draft(channel, &draft).unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/013_draft_mentions.down.sql"
        ))
        .execute(&storage.pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 13")
            .execute(&storage.pool)
            .await
            .unwrap();
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.drafts().unwrap()[&channel], draft);
        draft.mentions.push(crate::mentions::DraftMention {
            start: 0,
            end: 8,
            text: draft.text.clone(),
            aci: Uuid::new_v4(),
        });
        storage.store_draft(channel, &draft).unwrap();
        storage.pool.close().await;
        let storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        assert_eq!(storage.drafts().unwrap()[&channel], draft);
        sqlx::query("UPDATE composer_drafts SET mentions = 'invalid'")
            .execute(&storage.pool)
            .await
            .unwrap();
        assert!(storage.drafts().is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn composer_draft_failures_are_reported() {
        use crate::storage::Draft;
        let mut storage = SqliteStorage::open_unencrypted(&"sqlite::memory:".parse().unwrap())
            .await
            .unwrap();
        let channel = ChannelId::User(Uuid::nil());
        storage
            .store_draft(
                channel,
                &Draft {
                    text: "hello".into(),
                    ..Draft::default()
                },
            )
            .unwrap();
        sqlx::query("UPDATE composer_drafts SET cursor = -1")
            .execute(&storage.pool)
            .await
            .unwrap();
        assert!(storage.drafts().is_err());
        sqlx::query("UPDATE composer_drafts SET cursor = 0, attachments = X'FF'")
            .execute(&storage.pool)
            .await
            .unwrap();
        assert!(storage.drafts().is_err());
        storage.pool.close().await;
        assert!(storage.drafts().is_err());
        assert!(storage.last_draft_channel().is_err());
        assert!(storage.store_draft(channel, &Draft::default()).is_err());
        assert!(storage.store_last_draft_channel(Some(channel)).is_err());
        assert!(storage.store_last_draft_channel(None).is_err());
        assert!(
            storage
                .store_draft(
                    channel,
                    &Draft {
                        text: "unsaved".into(),
                        ..Draft::default()
                    }
                )
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_aliases_survive_reopen_without_overwriting_names() {
        use crate::storage::AliasTarget;
        let dir = tempdir().unwrap();
        let url: Url = format!("sqlite://{}", dir.path().join("aliases.sqlite").display())
            .parse()
            .unwrap();
        let person = uuid!("966960e0-a8cd-43f1-ac7a-2c986dd470cd");
        let channel_id = ChannelId::User(person);
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        storage.store_channel(&test_channel(channel_id, "Original conversation"));
        storage.store_name(person, "Original person");
        storage
            .set_alias(AliasTarget::Person(person), Some("Żółw 🐢"))
            .unwrap();
        storage
            .set_alias(AliasTarget::Channel(channel_id), Some("Praca"))
            .unwrap();
        storage.store_name(person, "Updated person");
        storage.store_channel(&test_channel(channel_id, "Updated conversation"));
        storage.pool.close().await;
        let mut storage = SqliteStorage::open_unencrypted(&url).await.unwrap();
        let aliases = storage.aliases().unwrap();
        assert_eq!(
            aliases
                .get(&AliasTarget::Person(person))
                .map(String::as_str),
            Some("Żółw 🐢")
        );
        assert_eq!(
            aliases
                .get(&AliasTarget::Channel(channel_id))
                .map(String::as_str),
            Some("Praca")
        );
        assert_eq!(storage.name(person).as_deref(), Some("Updated person"));
        assert_eq!(
            storage.channel(channel_id).unwrap().name,
            "Updated conversation"
        );
        storage
            .set_alias(AliasTarget::Person(person), None)
            .unwrap();
        assert!(
            !storage
                .aliases()
                .unwrap()
                .contains_key(&AliasTarget::Person(person))
        );
        assert_eq!(storage.aliases().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_alias_migration_preserves_old_data() {
        use crate::storage::AliasTarget;
        let opts: SqliteConnectOptions = "sqlite::memory:".parse().unwrap();
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        let mut old = sqlx::migrate!();
        old.migrations.to_mut().retain(|m| m.version < 7);
        old.run(&pool).await.unwrap();
        sqlx::query("INSERT INTO names(id, name) VALUES (?, 'Legacy name')")
            .bind(Uuid::nil())
            .execute(&pool)
            .await
            .unwrap();
        let mut storage = SqliteStorage {
            pool,
            message_order: Default::default(),
        };
        let channel_id = ChannelId::User(Uuid::nil());
        storage.store_channel(&test_channel(channel_id, "Legacy conversation"));
        seed_legacy_message(&storage.pool, channel_id, 42, "Legacy message").await;
        sqlx::migrate!().run(&storage.pool).await.unwrap();
        assert_eq!(
            storage.channel(channel_id).unwrap().name,
            "Legacy conversation"
        );
        assert_eq!(
            storage
                .message(MessageId::new(channel_id, 42))
                .unwrap()
                .message
                .as_deref(),
            Some("Legacy message")
        );
        assert!(storage.aliases().unwrap().is_empty());
        assert_eq!(storage.name(Uuid::nil()).as_deref(), Some("Legacy name"));
        storage
            .set_alias(AliasTarget::Person(Uuid::nil()), Some("Alias"))
            .unwrap();
        assert_eq!(storage.name(Uuid::nil()).as_deref(), Some("Legacy name"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_alias_write_failure_is_reported() {
        use crate::storage::AliasTarget;
        let mut storage = fixtures().await;
        storage.pool.close().await;
        assert!(
            storage
                .set_alias(AliasTarget::Person(Uuid::nil()), Some("Alias"))
                .is_err()
        );
    }

    async fn fixtures() -> SqliteStorage {
        let url: Url = "sqlite::memory:".parse().unwrap();
        let mut storage = SqliteStorage::open(&url, &Passphrase::new("secret").unwrap())
            .await
            .unwrap();

        let user_channel = ChannelId::User(uuid!("966960e0-a8cd-43f1-ac7a-2c986dd470cd"));
        storage.store_channel(&Channel {
            id: user_channel,
            name: "direct-channel".to_owned(),
            group_data: None,
            unread_messages: 1,
            muted: false,
            typing: TypingSet::new(false),
            expire_timer: None,
        });
        storage.store_message(
            user_channel,
            &Message {
                local_id: None,
                is_edit_snapshot: false,
                from_id: uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a"),
                message: Some("hello".to_owned()),
                arrived_at: 1664832050000,
                quote: None,
                attachments: Default::default(),
                reactions: Default::default(),
                receipt: Receipt::Nothing,
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

        let group_channel = ChannelId::Group([
            52, 49, 52, 57, 98, 57, 54, 56, 54, 56, 48, 55, 102, 100, 98, 52, 97, 56, 99, 57, 53,
            100, 57, 98, 53, 52, 49, 51, 98, 98, 99, 100,
        ]);
        storage.store_channel(&Channel {
            id: group_channel,
            name: "group-channel".to_owned(),
            group_data: None,
            unread_messages: 2,
            muted: false,
            typing: TypingSet::new(true),
            expire_timer: None,
        });
        storage.store_message(
            group_channel,
            &Message {
                local_id: None,
                is_edit_snapshot: false,
                from_id: uuid!("ac9b8aa1-691a-47e1-a566-d3e942945d07"),
                message: Some("world".to_owned()),
                arrived_at: 1664832050001,
                quote: None,
                attachments: Default::default(),
                reactions: Default::default(),
                receipt: Receipt::Nothing,
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

        storage.store_name(uuid!("966960e0-a8cd-43f1-ac7a-2c986dd470cd"), "ellie");
        storage.store_name(uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a"), "joel");

        storage
    }

    /// Storage with a single channel containing messages at arrived_at 10, 20, ..., 90
    async fn windowed_fixtures() -> (SqliteStorage, ChannelId) {
        let url: Url = "sqlite::memory:".parse().unwrap();
        let mut storage = SqliteStorage::open(&url, &Passphrase::new("secret").unwrap())
            .await
            .unwrap();

        let channel_id = ChannelId::User(uuid!("966960e0-a8cd-43f1-ac7a-2c986dd470cd"));
        storage.store_channel(&Channel {
            id: channel_id,
            name: "direct-channel".to_owned(),
            group_data: None,
            unread_messages: 0,
            muted: false,
            typing: TypingSet::new(false),
            expire_timer: None,
        });

        let from_id = uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a");
        for arrived_at in (10..=90).step_by(10) {
            storage.store_message(
                channel_id,
                &Message::text(from_id, arrived_at, format!("message {arrived_at}")),
            );
        }

        (storage, channel_id)
    }

    fn arrived_ats(messages: &[Message]) -> Vec<u64> {
        messages.iter().map(|message| message.arrived_at).collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_messages_tail() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let (storage, channel_id) = windowed_fixtures().await;

        // newest `limit` messages, ascending
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel_id, 3)),
            [70, 80, 90]
        );

        // short result when limit exceeds the number of messages
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel_id, 100)),
            [10, 20, 30, 40, 50, 60, 70, 80, 90]
        );

        assert!(storage.messages_tail(channel_id, 0).is_empty());

        let unknown_channel = ChannelId::User(uuid!("00000000-0000-0000-0000-000000000000"));
        assert!(storage.messages_tail(unknown_channel, 3).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_messages_before() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let (storage, channel_id) = windowed_fixtures().await;

        // messages closest to the anchor, ascending; the anchor itself is excluded
        assert_eq!(
            arrived_ats(&storage.messages_before(channel_id, 50, 2)),
            [30, 40]
        );

        // anchor between two messages
        assert_eq!(
            arrived_ats(&storage.messages_before(channel_id, 55, 2)),
            [40, 50]
        );

        // short result at the oldest message
        assert_eq!(
            arrived_ats(&storage.messages_before(channel_id, 50, 100)),
            [10, 20, 30, 40]
        );

        // nothing before the oldest message
        assert!(storage.messages_before(channel_id, 10, 3).is_empty());

        assert!(storage.messages_before(channel_id, 50, 0).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_messages_after() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let (storage, channel_id) = windowed_fixtures().await;

        // messages closest to the anchor, ascending; the anchor itself is excluded
        assert_eq!(
            arrived_ats(&storage.messages_after(channel_id, 50, 2)),
            [60, 70]
        );

        // anchor between two messages
        assert_eq!(
            arrived_ats(&storage.messages_after(channel_id, 45, 2)),
            [50, 60]
        );

        // short result at the newest message
        assert_eq!(
            arrived_ats(&storage.messages_after(channel_id, 50, 100)),
            [60, 70, 80, 90]
        );

        // nothing after the newest message
        assert!(storage.messages_after(channel_id, 90, 3).is_empty());

        assert!(storage.messages_after(channel_id, 50, 0).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_windowed_messages_skip_edits() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let (mut storage, channel_id) = windowed_fixtures().await;
        let from_id = uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a");

        // editing message 50 preserves the original body in an edit row at 51 and stores the edit
        // itself at 95; both must be skipped
        storage.store_edited_message(
            channel_id,
            50,
            Message::text(from_id, 95, "message 50 edited".to_owned()),
        );

        let messages = storage.messages_tail(channel_id, 100);
        assert_eq!(arrived_ats(&messages), [10, 20, 30, 40, 50, 60, 70, 80, 90]);
        let edited = &messages[4];
        assert_eq!(edited.arrived_at, 50);
        assert_eq!(edited.message.as_deref(), Some("message 50 edited"));
        assert!(edited.edited);

        // the edit row at 51 is skipped
        assert_eq!(
            arrived_ats(&storage.messages_before(channel_id, 52, 1)),
            [50]
        );
        // the edit row at 95 is skipped
        assert!(storage.messages_after(channel_id, 90, 10).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_windowed_messages_resolve_quotes() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let (mut storage, channel_id) = windowed_fixtures().await;
        let from_id = uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a");

        let quoted = storage.message(MessageId::new(channel_id, 20)).unwrap();
        let mut reply = Message::text(from_id, 100, "reply".to_owned());
        reply.quote = Some(Box::new(quoted));
        storage.store_message(channel_id, &reply);

        let messages = storage.messages_tail(channel_id, 1);
        assert_eq!(arrived_ats(&messages), [100]);
        let quote = messages[0].quote.as_ref().unwrap();
        assert_eq!(quote.arrived_at, 20);
        assert_eq!(quote.message.as_deref(), Some("message 20"));
    }

    fn test_channel(id: ChannelId, name: &str) -> Channel {
        Channel {
            id,
            name: name.to_owned(),
            group_data: None,
            unread_messages: 0,
            muted: false,
            typing: TypingSet::new(false),
            expire_timer: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_channels_sorted_by_recency() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let url: Url = "sqlite::memory:".parse().unwrap();
        let mut storage = SqliteStorage::open(&url, &Passphrase::new("secret").unwrap())
            .await
            .unwrap();
        let from_id = uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a");

        let recent = ChannelId::User(uuid!("11111111-1111-1111-1111-111111111111"));
        let middle = ChannelId::User(uuid!("22222222-2222-2222-2222-222222222222"));
        let old = ChannelId::User(uuid!("33333333-3333-3333-3333-333333333333"));
        let empty = ChannelId::User(uuid!("44444444-4444-4444-4444-444444444444"));

        // store channels in a deliberately non-recency order
        for (id, name) in [
            (old, "old"),
            (recent, "recent"),
            (empty, "empty"),
            (middle, "middle"),
        ] {
            storage.store_channel(&test_channel(id, name));
        }
        storage.store_message(old, &Message::text(from_id, 100, "old".to_owned()));
        storage.store_message(middle, &Message::text(from_id, 200, "middle".to_owned()));
        storage.store_message(recent, &Message::text(from_id, 300, "recent".to_owned()));

        // most recent first; the channel without messages sorts last
        let order: Vec<ChannelId> = storage.channels().into_iter().map(|c| c.id).collect();
        assert_eq!(order, [recent, middle, old, empty]);

        // an edit (newer arrived_at, edit IS NOT NULL) must not change recency
        storage.store_edited_message(
            old,
            100,
            Message::text(from_id, 999, "old edited".to_owned()),
        );
        let order: Vec<ChannelId> = storage.channels().into_iter().map(|c| c.id).collect();
        assert_eq!(order, [recent, middle, old, empty]);
    }

    /// Storing a channel must not delete its messages.
    ///
    /// To guard against using `REPLACE INTO` and deleting messages on cascade.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_store_channel_keeps_messages() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let url: Url = "sqlite::memory:".parse().unwrap();
        let mut storage = SqliteStorage::open(&url, &Passphrase::new("secret").unwrap())
            .await
            .unwrap();
        let from_id = uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a");
        let channel_id = ChannelId::User(uuid!("11111111-1111-1111-1111-111111111111"));

        storage.store_channel(&test_channel(channel_id, "channel"));
        for arrived_at in [100, 200, 300] {
            storage.store_message(
                channel_id,
                &Message::text(from_id, arrived_at, format!("message {arrived_at}")),
            );
        }
        assert_eq!(storage.messages_tail(channel_id, 10).len(), 3);

        // Re-store the same channel
        let mut channel = test_channel(channel_id, "channel");
        channel.muted = true;
        storage.store_channel(&channel);

        let messages = storage.messages_tail(channel_id, 10);
        let arrived_ats: Vec<u64> = messages.iter().map(|m| m.arrived_at).collect();
        assert_eq!(
            arrived_ats,
            [100, 200, 300],
            "messages were lost when re-storing the channel"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_messages_count_after() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let (mut storage, channel_id) = windowed_fixtures().await;
        let from_id = uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a");

        assert_eq!(storage.messages_count_after(channel_id, 50), 4);
        assert_eq!(storage.messages_count_after(channel_id, 90), 0);
        assert_eq!(storage.messages_count_after(channel_id, 0), 9);

        // edit rows (preserved original at 51, edit at 95) must not be counted
        storage.store_edited_message(
            channel_id,
            50,
            Message::text(from_id, 95, "message 50 edited".to_owned()),
        );
        assert_eq!(storage.messages_count_after(channel_id, 50), 4);
        assert_eq!(storage.messages_count_after(channel_id, 90), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_last_message() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let (storage, channel_id) = windowed_fixtures().await;

        let last = storage.last_message(channel_id).unwrap();
        assert_eq!(last.arrived_at, 90);
        assert_eq!(last.message.as_deref(), Some("message 90"));

        let unknown_channel = ChannelId::User(uuid!("00000000-0000-0000-0000-000000000000"));
        assert_eq!(storage.last_message(unknown_channel), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_remove_expired() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let (mut storage, channel_id) = windowed_fixtures().await;
        let from_id = uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a");

        for (arrived_at, expires_at) in [(101, 100), (102, 200), (103, 300)] {
            let mut message = Message::text(from_id, arrived_at, format!("expiring {arrived_at}"));
            message.expires_at = Some(expires_at);
            storage.store_message(channel_id, &message);
        }

        assert_eq!(storage.next_expiring_at(), Some(100));

        // nothing expired yet
        assert!(storage.remove_expired(99).is_empty());
        assert_eq!(storage.next_expiring_at(), Some(100));

        // expiry at the boundary is inclusive
        let mut removed = storage.remove_expired(200);
        removed.sort_unstable();
        assert_eq!(
            removed,
            [
                MessageId::new(channel_id, 101),
                MessageId::new(channel_id, 102)
            ]
        );
        assert_eq!(storage.message(MessageId::new(channel_id, 101)), None);
        assert_eq!(storage.message(MessageId::new(channel_id, 102)), None);
        assert!(storage.message(MessageId::new(channel_id, 103)).is_some());
        assert_eq!(storage.next_expiring_at(), Some(300));

        let removed = storage.remove_expired(10_000);
        assert_eq!(removed, [MessageId::new(channel_id, 103)]);
        assert_eq!(storage.next_expiring_at(), None);

        // messages without expires_at are untouched
        assert_eq!(
            arrived_ats(&storage.messages_tail(channel_id, 100)),
            [10, 20, 30, 40, 50, 60, 70, 80, 90]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_channels() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let storage = fixtures().await;
        let channels = storage.channels();
        assert_eq!(channels.len(), 2);
        assert_eq!(storage.channel(channels[0].id).unwrap().id, channels[0].id);
        assert_eq!(storage.channel(channels[1].id).unwrap().id, channels[1].id);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_messages() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let storage = fixtures().await;
        let id: Uuid = "966960e0-a8cd-43f1-ac7a-2c986dd470cd".parse().unwrap();

        let messages: Vec<_> = storage.messages(id.into()).collect();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message.as_deref(), Some("hello"));

        let arrived_at = messages[0].arrived_at;
        let message = storage
            .message(MessageId::new(id.into(), arrived_at))
            .unwrap();
        assert_eq!(message.arrived_at, arrived_at);
        assert_eq!(message.message.as_deref(), Some("hello"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_store_existing_message() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let mut storage = fixtures().await;
        let id: Uuid = "966960e0-a8cd-43f1-ac7a-2c986dd470cd".parse().unwrap();
        let arrived_at = 1664832050000;
        let mut message = storage
            .message(MessageId::new(id.into(), arrived_at))
            .unwrap();
        message.message = Some("changed".to_string());

        let arrived_at = message.arrived_at;
        storage.store_message(id.into(), &message);

        let messages: Vec<_> = storage.messages(id.into()).collect();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].arrived_at, arrived_at);
        assert_eq!(messages[0].message.as_deref(), Some("changed"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_store_new_message() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let mut storage = fixtures().await;

        let id: Uuid = uuid!("966960e0-a8cd-43f1-ac7a-2c986dd470cd");

        // store quote
        let quote_arrived_at = 1664832050000;
        let quote = storage
            .message(MessageId::new(id.into(), quote_arrived_at))
            .unwrap();

        // store message
        let arrived_at = 1664832050001;
        assert_eq!(storage.message(MessageId::new(id.into(), arrived_at)), None);
        let attachments = vec![Attachment {
            id: "some_attachment".to_owned(),
            content_type: "image/png".to_owned(),
            filename: "example.png".into(),
            size: 42,
        }];
        let reactions = vec![(id, "+1".to_owned())];
        let receipt = Receipt::Read;
        let body_ranges = vec![BodyRange {
            start: 0,
            end: 1,
            value: crate::data::AssociatedValue::MentionUuid(id),
        }];
        storage.store_message(
            id.into(),
            &Message {
                local_id: None,
                is_edit_snapshot: false,
                from_id: id,
                message: Some("new msg".to_string()),
                arrived_at,
                quote: Some(Box::new(quote.clone())),
                attachments: attachments.clone(),
                reactions: reactions.clone(),
                receipt,
                body_ranges: body_ranges.clone(),
                send_failed: Default::default(),
                attachment_downloads: Default::default(),
                edit: Default::default(),
                edited: Default::default(),
                deleted: Default::default(),
                expire_timer: None,
                expires_at: None,
            },
        );

        let messages: Vec<_> = storage.messages(id.into()).collect();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].arrived_at, arrived_at);
        assert_eq!(messages[1].message.as_deref(), Some("new msg"));
        assert_eq!(messages[1].quote.as_deref(), Some(&quote));
        assert_eq!(messages[1].attachments, attachments);
        assert_eq!(messages[1].reactions, reactions);
        assert_eq!(messages[1].receipt, receipt);
        assert_eq!(messages[1].body_ranges, body_ranges);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_same_arrived_at_in_different_channels() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let mut storage = fixtures().await;

        // channel_a already has "hello" at this timestamp
        let channel_a = ChannelId::User(uuid!("966960e0-a8cd-43f1-ac7a-2c986dd470cd"));
        let arrived_at = 1664832050000;

        let channel_b = ChannelId::User(uuid!("91a6315b-027c-44ce-bacb-4d5cf012ba8c"));
        storage.store_channel(&Channel {
            id: channel_b,
            name: "channel-b".to_owned(),
            group_data: None,
            unread_messages: 0,
            muted: false,
            typing: TypingSet::new(false),
            expire_timer: None,
        });
        storage.store_message(
            channel_b,
            &Message::text(
                uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a"),
                arrived_at,
                "world".to_owned(),
            ),
        );

        // both rows coexist under the composite primary key
        assert_eq!(
            storage
                .message(MessageId::new(channel_a, arrived_at))
                .unwrap()
                .message
                .as_deref(),
            Some("hello")
        );
        assert_eq!(
            storage
                .message(MessageId::new(channel_b, arrived_at))
                .unwrap()
                .message
                .as_deref(),
            Some("world")
        );
        assert_eq!(storage.messages(channel_a).count(), 1);
        assert_eq!(storage.messages(channel_b).count(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_delete_channel_cascades_messages() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let storage = fixtures().await;
        let channel_id = ChannelId::User(uuid!("966960e0-a8cd-43f1-ac7a-2c986dd470cd"));
        assert_eq!(storage.messages(channel_id).count(), 1);

        sqlx::query("DELETE FROM channels WHERE id = ?")
            .bind(&channel_id)
            .execute(&storage.pool)
            .await
            .unwrap();

        assert_eq!(storage.messages(channel_id).count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_store_message_for_missing_channel_is_dropped() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let mut storage = fixtures().await;
        let missing = ChannelId::User(uuid!("00000000-0000-0000-0000-000000000000"));

        // the foreign key rejects the insert; the message is dropped, not stored
        storage.store_message(
            missing,
            &Message::text(
                uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a"),
                123,
                "lost".to_owned(),
            ),
        );

        assert_eq!(storage.message(MessageId::new(missing, 123)), None);
        assert_eq!(storage.messages(missing).count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_names() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let mut storage = fixtures().await;
        let id1 = uuid!("966960e0-a8cd-43f1-ac7a-2c986dd470cd");
        let id2 = uuid!("a955d20f-6b83-4e69-846e-a99b1779ff7a");
        let id3 = uuid!("91a6315b-027c-44ce-bacb-4d5cf012ba8c");

        assert_eq!(storage.names().count(), 2);
        assert_eq!(storage.name(id1).unwrap(), "ellie");
        assert_eq!(storage.name(id2).unwrap(), "joel");

        storage.store_name(id3, "abby");
        assert_eq!(storage.names().count(), 3);
        assert_eq!(storage.name(id1).unwrap(), "ellie");
        assert_eq!(storage.name(id2).unwrap(), "joel");
        assert_eq!(storage.name(id3).unwrap(), "abby");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_storage_metadata() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let mut storage = fixtures().await;
        assert_eq!(storage.metadata().contacts_sync_request_at, None);

        let dt = Utc::now();
        storage.store_metadata(&Metadata {
            contacts_sync_request_at: Some(dt),
            fully_migrated: Some(true),
        });

        let Metadata {
            contacts_sync_request_at,
            fully_migrated,
        } = storage.metadata();
        assert_eq!(contacts_sync_request_at, Some(dt));
        assert_eq!(fully_migrated, Some(true));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlite_created_encrypted_with_passphrase() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();

        let tempdir = tempdir().unwrap();
        let path = tempdir.path().join("data.db");
        let url: Url = format!("sqlite://{}", path.display()).parse().unwrap();

        assert_eq!(is_sqlite_encrypted_heuristics(&url), None);

        let secret = "secret".to_owned();
        SqliteStorage::maybe_encrypt_and_open(&url, &Passphrase::new(secret).unwrap(), false)
            .await
            .unwrap();

        assert_eq!(is_sqlite_encrypted_heuristics(&url), Some(true));
    }
}
