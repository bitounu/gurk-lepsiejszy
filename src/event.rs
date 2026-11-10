use uuid::Uuid;

use crate::signal::Attachment;
use crate::storage::MessageId;

#[derive(Debug)]
pub enum Event {
    ForwardPrepared {
        generation: u64,
        target: crate::data::ChannelId,
        result: anyhow::Result<Vec<(presage::libsignal_service::sender::AttachmentSpec, Vec<u8>)>>,
    },
    ImageViewerReady,
    FilePickerUpdated,
    FileOpened {
        generation: u64,
        result: anyhow::Result<()>,
    },
    ScreenshotCaptured {
        channel_id: crate::data::ChannelId,
        result: anyhow::Result<Option<std::path::PathBuf>>,
    },
    AttachmentsPrepared {
        job_id: u64,
        result: anyhow::Result<Vec<(presage::libsignal_service::sender::AttachmentSpec, Vec<u8>)>>,
    },
    SentTextResult {
        message_id: MessageId,
        result: anyhow::Result<()>,
    },
    AttachmentSaved {
        message_id: MessageId,
        attachment: anyhow::Result<Attachment>,
    },
    ProfileNameResolved {
        user_id: Uuid,
        name: Option<String>,
    },
}
