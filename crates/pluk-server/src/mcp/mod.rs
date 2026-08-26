//! The MCP protocol layer: surface building, owner pools, namespacing.

pub(crate) mod build;
pub mod namespace;
pub mod owner;
mod surface;

pub use build::{
    apply_overrides, build_group_surface, build_integration_surface, build_owner_surface,
    resolve_owner, Owner,
};
pub use owner::OwnerPool;
pub use surface::Surface;
