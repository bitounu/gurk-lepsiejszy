use std::sync::LazyLock;

use chrono::{DateTime, Local};
use phonenumber::PhoneNumber;
use ratatui::widgets::ListState;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{
    data::{Channel, ChannelId},
    storage::Storage,
};

const MESSAGE_SCROLL_BACK: bool = false;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatefulList<T> {
    #[serde(skip)]
    pub state: ListState,
    pub items: Vec<T>,
    #[serde(skip)]
    pub rendered: Rendered,
}

impl<T: PartialEq> PartialEq for StatefulList<T> {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
    }
}

impl<T: Eq> Eq for StatefulList<T> {}

#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub offset: usize,
}

impl<T> Default for StatefulList<T> {
    fn default() -> Self {
        Self {
            state: Default::default(),
            items: Vec::new(),
            rendered: Default::default(),
        }
    }
}

impl<T> StatefulList<T> {
    pub fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i + 1 >= self.items.len() {
                    if MESSAGE_SCROLL_BACK { 0 } else { i }
                } else {
                    i + 1
                }
            }
            None => {
                if !self.items.is_empty() {
                    0
                } else {
                    return; // nothing to select
                }
            }
        };
        self.state.select(Some(i));
    }

    pub fn previous(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    if MESSAGE_SCROLL_BACK {
                        self.items.len() - 1
                    } else {
                        0
                    }
                } else {
                    i - 1
                }
            }
            None => {
                if !self.items.is_empty() {
                    0
                } else {
                    return; // nothing to select
                }
            }
        };
        self.state.select(Some(i));
    }

    pub(crate) fn selected_item(&self) -> Option<&T> {
        let idx = self.state.selected()?;
        Some(&self.items[idx])
    }
}

impl StatefulList<Channel> {
    /// Modifies the channel at the given index and writes it to the storage.
    ///
    /// If the channel is not found, does nothing. Returns `Some(true)` if the channel was modified.
    /// Returns `None` if the channel does not exist.
    pub(crate) fn modify_channel(
        &mut self,
        storage: &mut dyn Storage,
        idx: usize,
        f: impl FnOnce(&mut Channel) -> bool,
    ) -> Option<bool> {
        let channel = self.items.get_mut(idx)?;
        if f(channel) {
            storage.store_channel(channel);
            Some(true)
        } else {
            Some(false)
        }
    }

    /// Modifies the selected channel and writes it to the storage.
    ///
    /// If the channel is not found, does nothing. Returns `Some(true)` if the channel was modified.
    /// Returns `None` if no channel is selected or the channel does not exist.
    pub(crate) fn modify_selected_channel(
        &mut self,
        storage: &mut dyn Storage,
        f: impl FnOnce(&mut Channel) -> bool,
    ) -> Option<bool> {
        let idx = self.state.selected()?;
        self.modify_channel(storage, idx, f)
    }

    /// Modifies the channel with the given id and writes it to the storage.
    ///
    /// If the channel is not found, does nothing. Returns `Some(true)` if the channel was modified.
    /// Returns `None` if the channel does not exist.
    pub(crate) fn modify_channel_by_id(
        &mut self,
        storage: &mut dyn Storage,
        channel_id: ChannelId,
        f: impl FnOnce(&mut Channel) -> bool,
    ) -> Option<bool> {
        let idx = self
            .items
            .iter()
            .position(|channel| channel.id == channel_id)?;
        self.modify_channel(storage, idx, f)
    }
}

pub fn utc_timestamp_msec_to_local(timestamp: u64) -> DateTime<Local> {
    DateTime::from_timestamp((timestamp / 1000) as i64, (timestamp % 1000) as u32)
        .expect("invalid datetime")
        .with_timezone(&Local)
}

pub fn utc_now_timestamp_msec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

pub fn is_phone_number(s: impl AsRef<str>) -> bool {
    // Note: previously we formatted phone numbers sometimes incorrectly (not always as E164). So,
    // some users might still have them stored with spaces and dashes. So, we strip them here, even
    // the formatting now is correct.
    let stripped = s.as_ref().replace(&[' ', '-'][..], "");
    stripped.parse::<PhoneNumber>().is_ok()
}

// Based on Alacritty, APACHE-2.0 License
pub(crate) static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "(ipfs:|ipns:|magnet:|mailto:|gemini:|gopher:|https:|http:|news:|file:|git:|ssh:|ftp:)\
     [^\u{0000}-\u{001F}\u{007F}-\u{009F}<>\"\\s{-}\\^⟨⟩`]+",
    )
    .unwrap()
});

// Based on Alacritty, APACHE-2.0 License
pub(crate) static ATTACHMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("file:[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>\"\\s{-}\\^⟨⟩`]+").unwrap()
});

/// Reserve a process-wide timestamp for a new outgoing message. Split attachment
/// batches must retain distinct identities even when sent within one millisecond.
pub(crate) fn next_send_timestamp_msec() -> u64 {
    static LAST_SEND: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    reserve_send_timestamp(&LAST_SEND, utc_now_timestamp_msec())
}

fn reserve_send_timestamp(counter: &std::sync::atomic::AtomicU64, now: u64) -> u64 {
    use std::sync::atomic::Ordering;
    let mut previous = counter.load(Ordering::Relaxed);
    loop {
        let next = now.max(
            previous
                .checked_add(1)
                .expect("Outgoing timestamp exhausted"),
        );
        match counter.compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(current) => previous = current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_phone_number() {
        assert!(is_phone_number("+1 800-000-0000"));
    }
}

#[cfg(test)]
mod send_timestamp_tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn reservation_handles_same_clock_rollback_and_forward_gap() {
        let counter = AtomicU64::new(0);
        assert_eq!(reserve_send_timestamp(&counter, 100), 100);
        assert_eq!(reserve_send_timestamp(&counter, 100), 101);
        assert_eq!(reserve_send_timestamp(&counter, 90), 102);
        assert_eq!(reserve_send_timestamp(&counter, 500), 500);
        assert_eq!(reserve_send_timestamp(&counter, 500), 501);
    }

    #[test]
    fn concurrent_reservations_are_unique() {
        let counter = AtomicU64::new(0);
        let mut values = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| scope.spawn(|| reserve_send_timestamp(&counter, 100)))
                .collect();
            handles
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        values.sort_unstable();
        assert_eq!(values, (100..116).collect::<Vec<_>>());
    }
}
