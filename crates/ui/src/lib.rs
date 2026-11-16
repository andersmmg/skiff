//! GPUI views for Skiff, built on gpui-kit's component library.

mod format;
pub mod tray;
mod views;
pub mod window;

pub use views::Root;
pub(crate) use views::Page;
pub use window::open_main_window;
