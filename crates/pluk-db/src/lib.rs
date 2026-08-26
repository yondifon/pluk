pub mod capping;
pub mod config;
pub mod driver;
pub mod error;
pub mod factory;
pub mod fake;
pub mod mysql;
pub mod postgres;
pub mod sql_log;
pub mod ssl;
pub mod types;

#[cfg(test)]
mod tests;

pub use capping::{cap_and_mask, cap_rows, mask_columns};
pub use config::{SqlConfig, SshTunnelProvider, TunnelEndpoint};
pub use driver::{Driver, with_opts};
pub use error::DriverError;
pub use factory::{CreateDriverOpts, DriverWithTunnel, create_driver};
pub use ssl::{SslConfig, SslMode, build_ssl_config};
pub use types::*;
