//! The MCP protocol layer: surface building and owner pools.

pub(crate) mod build;
pub mod owner;
mod surface;

pub use build::{
    Owner, apply_overrides, build_group_surface, build_integration_surface, build_owner_surface,
    resolve_owner,
};
pub use owner::OwnerPool;
pub use surface::Surface;
