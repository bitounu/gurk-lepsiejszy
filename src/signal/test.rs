use std::pin::Pin;
use std::{cell::RefCell, rc::Rc};

use async_trait::async_trait;
use presage::libsignal_service::content::Content;
use presage::libsignal_service::prelude::AttachmentIdentifier;
use presage::libsignal_service::sender::AttachmentSpec;
use presage::model::contacts::Contact;
use presage::model::groups::Group;
use presage::proto::AttachmentPointer;
use presage::proto::data_message::Quote;
use tokio::sync::oneshot;
use tokio_stream::Stream;
use uuid::Uuid;

use crate::data::{Channel, GroupData, Message};
use crate::receipt::Receipt;

use super::{Attachment, GroupMasterKeyBytes, ProfileKeyBytes, ResolvedGroup, SignalManager};

type ReadSyncLog = Rc<RefCell<Vec<Vec<(Uuid, u64)>>>>;
type SentFilesLog = Rc<RefCell<Vec<Vec<(String, Option<String>, Vec<u8>)>>>>;

/// Signal manager mock which does not send any messages.
pub struct SignalManagerMock {
    user_id: Uuid,
    pub read_syncs: ReadSyncLog,
    pub sent_destinations: Rc<RefCell<Vec<crate::data::ChannelId>>>,
    pub sent_files: SentFilesLog,
    pub sent_messages: Rc<RefCell<Vec<Message>>>,
}

impl SignalManagerMock {
    pub fn new() -> Self {
        Self {
            user_id: Uuid::nil(),
            sent_destinations: Default::default(),
            sent_files: Default::default(),
            sent_messages: Default::default(),
            read_syncs: Default::default(),
        }
    }
}

impl Default for SignalManagerMock {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait(?Send)]
impl SignalManager for SignalManagerMock {
    fn user_id(&self) -> Uuid {
        self.user_id
    }

    async fn resolve_group(
        &mut self,
        master_key_bytes: super::GroupMasterKeyBytes,
    ) -> anyhow::Result<ResolvedGroup> {
        Ok(ResolvedGroup {
            name: "some_group".to_string(),
            group_data: GroupData {
                master_key_bytes,
                members: Default::default(),
                revision: 0,
            },
            profile_keys: Default::default(),
        })
    }

    fn save_attachment(
        &self,
        attachment_pointer: AttachmentPointer,
    ) -> oneshot::Receiver<anyhow::Result<Attachment>> {
        let id = match attachment_pointer.attachment_identifier.unwrap() {
            AttachmentIdentifier::CdnId(id) => id.to_string(),
            AttachmentIdentifier::CdnKey(id) => id,
        };
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Ok(Attachment {
            id,
            content_type: attachment_pointer.content_type.unwrap(),
            filename: "somefile".to_string().into(),
            size: attachment_pointer.size.unwrap(),
        }));
        rx
    }

    fn send_receipt(&self, _: Uuid, _: Vec<u64>, _: Receipt) {}

    fn send_read_sync(&self, read: Vec<(Uuid, u64)>) -> oneshot::Receiver<anyhow::Result<()>> {
        self.read_syncs.borrow_mut().push(read);
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Ok(()));
        rx
    }

    fn send_text(
        &self,
        channel: &Channel,
        text: crate::mentions::MessageText,
        quote_message: Option<&Message>,
        _edit_message_timestamp: Option<u64>,
        attachments: Vec<(AttachmentSpec, Vec<u8>)>,
    ) -> (Message, oneshot::Receiver<anyhow::Result<()>>) {
        self.sent_destinations.borrow_mut().push(channel.id);
        self.sent_files.borrow_mut().push(
            attachments
                .into_iter()
                .map(|(spec, bytes)| (spec.content_type, spec.file_name, bytes))
                .collect(),
        );
        let (message, body_ranges) = text.encode();
        let timestamp = crate::util::next_send_timestamp_msec();
        let quote = quote_message.map(|message| Quote {
            id: Some(message.arrived_at),
            author_aci: Some(message.from_id.to_string()),
            text: message.message.clone(),
            ..Default::default()
        });
        let quote_message = quote.and_then(Message::from_quote).map(Box::new);
        let message = Message {
            from_id: self.user_id(),
            message: Some(message),
            arrived_at: timestamp,
            local_id: None,
            is_edit_snapshot: false,
            quote: quote_message,
            attachments: Default::default(),
            reactions: Default::default(),
            receipt: Receipt::Sent,
            body_ranges,
            send_failed: Default::default(),
            attachment_downloads: Default::default(),
            edit: Default::default(),
            edited: Default::default(),
            deleted: Default::default(),
            expire_timer: None,
            expires_at: None,
        };
        self.sent_messages.borrow_mut().push(message.clone());
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Ok(()));
        (message, rx)
    }

    fn send_reaction(&self, _channel: &Channel, _message: &Message, _emoji: String, _remove: bool) {
    }

    fn send_delete(&self, _channel: &Channel, _target_sent_timestamp: u64) {}

    fn send_delete_for_me(&self, _channel: &Channel, _message: &Message) {}

    fn resolve_profile_name(
        &self,
        _id: Uuid,
        _profile_key: ProfileKeyBytes,
    ) -> oneshot::Receiver<Option<String>> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(None);
        rx
    }

    async fn profile_name(&self, _id: Uuid) -> Option<String> {
        None
    }

    async fn contact(&self, _id: Uuid) -> Option<Contact> {
        None
    }

    async fn receive_messages(
        &mut self,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = Box<Content>>>>> {
        Ok(Box::pin(tokio_stream::empty()))
    }

    fn clone_boxed(&self) -> Box<dyn SignalManager> {
        Box::new(Self {
            user_id: self.user_id,
            sent_destinations: self.sent_destinations.clone(),
            sent_files: self.sent_files.clone(),
            sent_messages: self.sent_messages.clone(),
            read_syncs: self.read_syncs.clone(),
        })
    }

    async fn contacts(&self) -> Box<dyn Iterator<Item = Contact>> {
        Box::new(std::iter::empty())
    }

    async fn groups(&self) -> Box<dyn Iterator<Item = (GroupMasterKeyBytes, Group)>> {
        Box::new(std::iter::empty())
    }
}
