//! Serializable staging records for an authenticated backup import.
use crate::data::{Channel, ChannelId, Message};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub media_root: [u8; 32],
    pub version: u64,
    pub backup_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaReference {
    /// Serialized signal.backup.FilePointer, retaining all locator information.
    pub pointer_bytes: Vec<u8>,
    pub content_type: String,
    pub file_name: Option<String>,
    pub is_long_text: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedMessage {
    pub channel_id: ChannelId,
    pub message: Message,
    pub render_index: u64,
    pub media: Vec<MediaReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsupportedRecord {
    pub reason: String,
    pub frame_index: u64,
    pub frame_bytes: Vec<u8>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ArchiveSummary {
    pub frames: u64,
    pub messages: u64,
    pub channels: u64,
    pub unsupported: BTreeMap<String, u64>,
}

/// Implementations must stage records: callbacks precede final HMAC and completion validation.
pub trait ArchiveSink {
    fn metadata(&mut self, metadata: BackupMetadata) -> Result<()>;
    fn channel(&mut self, channel: Channel) -> Result<()>;
    fn message(&mut self, message: ImportedMessage) -> Result<()>;
    /// Persist unrepresentable records rather than silently losing their contents.
    fn unsupported(&mut self, record: UnsupportedRecord) -> Result<()>;
}
