//! Retriable private linked-device read synchronization. Persistence is managed
//! by App: enqueue only after committing the encrypted outbox, and delete outbox
//! rows only for the successful acknowledgements returned by `step`.
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use uuid::Uuid;

use crate::signal::SignalManager;

type ReadKey = (Uuid, u64);

#[derive(Default)]
pub(crate) struct ReadSyncQueue {
    pending: BTreeSet<ReadKey>,
    completed: BTreeSet<ReadKey>,
    inflight: Option<(Vec<ReadKey>, oneshot::Receiver<anyhow::Result<()>>)>,
    retry_at: Option<Instant>,
}

impl ReadSyncQueue {
    pub(crate) fn remember_completed(&mut self, sender: Uuid, timestamp: u64) {
        self.completed.insert((sender, timestamp));
    }
    pub(crate) fn is_completed(&self, sender: Uuid, timestamp: u64) -> bool {
        self.completed.contains(&(sender, timestamp))
    }

    pub(crate) fn contains(&self, sender: Uuid, timestamp: u64) -> bool {
        self.pending.contains(&(sender, timestamp))
    }

    pub(crate) fn queue(&mut self, sender: Uuid, timestamp: u64) {
        self.pending.insert((sender, timestamp));
    }

    /// Returns only server-acknowledged entries. Failure, cancellation, and
    /// offline state retain all queued entries for a later retry.
    pub(crate) fn step(&mut self, manager: &dyn SignalManager) -> Vec<ReadKey> {
        let mut acknowledged = Vec::new();
        if let Some((batch, receiver)) = self.inflight.as_mut() {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    acknowledged = batch.clone();
                    for entry in batch.iter() {
                        self.pending.remove(entry);
                    }
                    self.inflight = None;
                    self.retry_at = None;
                }
                Ok(Err(_)) | Err(oneshot::error::TryRecvError::Closed) => {
                    self.inflight = None;
                    self.retry_at = Some(Instant::now() + Duration::from_secs(5));
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if self.inflight.is_none()
            && !self.pending.is_empty()
            && self
                .retry_at
                .is_none_or(|deadline| Instant::now() >= deadline)
        {
            // Bound each sync frame; contacts are only data inside this message.
            let batch: Vec<_> = self.pending.iter().take(100).copied().collect();
            let receiver = manager.send_read_sync(batch.clone());
            self.inflight = Some((batch, receiver));
        }
        acknowledged
    }
}

impl super::App {
    pub(crate) fn mark_visible_reads(&mut self, ids: Vec<crate::storage::MessageId>) {
        for id in ids {
            let Some(mut message) = self.storage.message(id) else {
                continue;
            };
            if message.from_id == self.user_id || message.deleted {
                continue;
            }
            if message.receipt == crate::receipt::Receipt::Read
                && (self.read_sync.contains(message.from_id, message.arrived_at)
                    || self
                        .read_sync
                        .is_completed(message.from_id, message.arrived_at))
            {
                continue;
            }
            match self
                .storage
                .read_sync_completed(message.from_id, message.arrived_at)
            {
                Ok(true) => {
                    self.read_sync
                        .remember_completed(message.from_id, message.arrived_at);
                    if message.receipt != crate::receipt::Receipt::Read {
                        message.receipt = crate::receipt::Receipt::Read;
                        self.store_message(id.channel_id, message);
                    }
                    continue;
                }
                Ok(false) => {}
                Err(error) => {
                    self.composer_error =
                        Some(format!("Cannot inspect private read state: {error:#}"));
                    continue;
                }
            }
            if let Err(error) =
                self.storage
                    .set_read_sync_pending(message.from_id, message.arrived_at, true)
            {
                self.composer_error =
                    Some(format!("Cannot preserve private read state: {error:#}"));
                continue;
            }
            self.read_sync.queue(message.from_id, message.arrived_at);
            message.receipt = crate::receipt::Receipt::Read;
            self.store_message(id.channel_id, message);
        }
    }

    pub fn step_private_reads(&mut self) {
        for (sender, timestamp) in self.read_sync.step(self.signal_manager.as_ref()) {
            if let Err(error) = self.storage.set_read_sync_pending(sender, timestamp, false) {
                self.read_sync.queue(sender, timestamp);
                self.composer_error = Some(format!("Cannot confirm private read state: {error:#}"));
            } else {
                self.read_sync.remember_completed(sender, timestamp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::test::SignalManagerMock;

    #[test]
    fn deduplicates_and_acknowledges_only_after_success() {
        let manager = SignalManagerMock::new();
        let mut queue = ReadSyncQueue::default();
        let sender = Uuid::new_v4();
        queue.queue(sender, 42);
        queue.queue(sender, 42);
        assert!(queue.step(&manager).is_empty());
        assert_eq!(
            manager.read_syncs.borrow().as_slice(),
            &[vec![(sender, 42)]]
        );
        assert_eq!(queue.step(&manager), vec![(sender, 42)]);
        assert!(manager.sent_messages.borrow().is_empty());
        assert!(queue.pending.is_empty());
    }

    #[test]
    fn canceled_transport_keeps_pending_and_backs_off() {
        let manager = SignalManagerMock::new();
        let mut queue = ReadSyncQueue::default();
        let entry = (Uuid::new_v4(), 42);
        queue.queue(entry.0, entry.1);
        let (tx, rx) = oneshot::channel();
        drop(tx);
        queue.inflight = Some((vec![entry], rx));
        assert!(queue.step(&manager).is_empty());
        assert!(queue.pending.contains(&entry));
        assert!(manager.read_syncs.borrow().is_empty());
        queue.retry_at = None;
        assert!(queue.step(&manager).is_empty());
        assert_eq!(queue.step(&manager), vec![entry]);
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn private_reads_cover_only_visible_focused_messages_and_survive_restart() {
        use crate::{app::App, data::Message, receipt::Receipt, storage::MessageId};
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let mock = crate::signal::test::SignalManagerMock::new();
        let syncs = mock.read_syncs.clone();
        let sent = mock.sent_messages.clone();
        app.signal_manager = Box::new(mock);
        let channel = app.selected_channel_id().unwrap();
        let sender = channel.user().unwrap();
        for timestamp in 1..=50 {
            let mut message =
                Message::text(sender, timestamp, format!("incoming message {timestamp}"));
            message.receipt = Receipt::Delivered;
            app.store_message(channel, message);
        }
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 14)).unwrap();
        let latest_id = MessageId::new(channel, 50);
        let mut legacy_read = app.storage.message(latest_id).unwrap();
        legacy_read.receipt = Receipt::Read;
        app.store_message(channel, legacy_read);
        app.terminal_focused = false;
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(app.storage.pending_read_sync().unwrap().is_empty());
        app.terminal_focused = true;
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let pending = app.storage.pending_read_sync().unwrap();
        assert!(!pending.is_empty());
        assert!(pending.contains(&(sender, 50)));
        assert!(pending.len() < 50);
        assert!(
            pending
                .iter()
                .all(|(author, timestamp)| *author == sender && *timestamp > 1)
        );
        assert_eq!(
            app.storage
                .message(MessageId::new(channel, 1))
                .unwrap()
                .receipt,
            Receipt::Delivered
        );
        let (mut restored, _) = App::try_new(app.config, app.signal_manager, app.storage).unwrap();
        restored.step_private_reads();
        assert_eq!(syncs.borrow().len(), 1);
        assert!(!restored.storage.pending_read_sync().unwrap().is_empty());
        restored.step_private_reads();
        assert!(restored.storage.pending_read_sync().unwrap().is_empty());
        assert!(restored.storage.read_sync_completed(sender, 50).unwrap());
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut restored))
            .unwrap();
        restored.step_private_reads();
        assert_eq!(syncs.borrow().len(), 1);
        assert!(sent.borrow().is_empty());
    }
}
