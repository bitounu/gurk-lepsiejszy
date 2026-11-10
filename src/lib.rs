//! Signal Messenger client for terminal

pub mod backup;

pub mod app;
pub mod backoff;
mod channels;
pub mod command;
pub mod config;
pub mod cursor;
pub mod data;
#[cfg(feature = "dev")]
pub mod dev;
pub(crate) mod emoji;
pub mod event;
pub mod input;
pub mod onboarding;
pub mod passphrase;
pub mod receipt;
pub mod screenshot;
pub mod search;
pub mod shortcuts;
pub mod signal;
pub mod storage;
pub mod ui;
pub mod util;

pub mod image_metadata;
pub mod spellcheck;

pub mod editor_preview;

pub mod mentions;
