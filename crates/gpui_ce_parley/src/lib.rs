//! Shared text support built on the Parley text stack.

mod catalog;
mod store;
mod text_system;

pub use catalog::SystemFonts;
pub use text_system::ParleyTextSystem;

pub(crate) use catalog::{CatalogState, FaceFamily, FaceRequest, FontCatalog};
pub(crate) use store::{FontRasterizer, FontStore};
