//! Display UI as a function of the app's state
//!
//! Also contains helpers for computing coordinates (for clicking)

mod coords;
mod draw;
pub(crate) mod file_opener;
pub(crate) mod file_picker;
pub(crate) mod image_preview;
pub(crate) mod image_viewer;
mod name_resolver;
pub(crate) mod shortcut_bar;
pub(crate) mod text_selection;

pub use coords::coords_within_channels_view;
pub use draw::draw;

pub const CHANNEL_VIEW_RATIO: u32 = 4;

pub(crate) mod mentions;
