//! Per-conversation FIFO gates reserved synchronously before send tasks are spawned.
//!
//! A permit releases its successor when its scope ends, including error returns.
//! This assumes queued send futures live until completion or application shutdown:
//! canceling a turn before it acquires its permit also drops its successor gate,
//! and does not preserve ordering around that canceled future.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use tokio::sync::oneshot;

use crate::data::ChannelId;

#[derive(Clone, Default)]
pub(crate) struct SendOrder {
    inner: Arc<Mutex<BTreeMap<ChannelId, oneshot::Receiver<()>>>>,
}

impl SendOrder {
    /// Reserve ordering on the caller thread, before asynchronous work is scheduled.
    pub(crate) fn reserve(&self, channel: ChannelId) -> SendTurn {
        let (completion, next) = oneshot::channel();
        let previous = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(channel, next);
        SendTurn {
            previous,
            completion,
        }
    }
}

pub(crate) struct SendTurn {
    previous: Option<oneshot::Receiver<()>>,
    completion: oneshot::Sender<()>,
}

impl SendTurn {
    pub(crate) async fn wait(self) -> SendPermit {
        let Self {
            previous,
            completion,
        } = self;
        if let Some(previous) = previous {
            // Closing the predecessor's sender is the completion signal.
            let _ = previous.await;
        }
        SendPermit {
            _completion: completion,
        }
    }
}

/// Hold this guard across the complete upload/send operation.
pub(crate) struct SendPermit {
    _completion: oneshot::Sender<()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, pin::pin, task::Poll};
    use uuid::Uuid;

    fn channel(value: u128) -> ChannelId {
        ChannelId::User(Uuid::from_u128(value))
    }

    async fn is_pending<F: Future>(mut future: std::pin::Pin<&mut F>) -> bool {
        std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn same_channel_acquires_in_reservation_order() {
        let order = SendOrder::default();
        let first = order.reserve(channel(1)).wait().await;
        let mut second = pin!(order.reserve(channel(1)).wait());
        let mut third = pin!(order.reserve(channel(1)).wait());
        assert!(is_pending(second.as_mut()).await);
        assert!(is_pending(third.as_mut()).await);
        drop(first);
        let second = second.await;
        assert!(is_pending(third.as_mut()).await);
        drop(second);
        let _third = third.await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn other_conversation_is_independent() {
        let order = SendOrder::default();
        let _first = order.reserve(channel(1)).wait().await;
        let mut same = pin!(order.reserve(channel(1)).wait());
        assert!(is_pending(same.as_mut()).await);
        let mut other = pin!(order.reserve(channel(2)).wait());
        assert!(!is_pending(other.as_mut()).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn error_scope_releases_next_turn_reserved_through_clone() {
        let order = SendOrder::default();
        let cloned = order.clone();
        let first = order.reserve(channel(1)).wait().await;
        let mut next = pin!(cloned.reserve(channel(1)).wait());
        assert!(is_pending(next.as_mut()).await);
        fn fail(_permit: SendPermit) -> Result<(), ()> {
            Err(())
        }
        assert_eq!(fail(first), Err(()));
        let _next = next.await;
    }
}
