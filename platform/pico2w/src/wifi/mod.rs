//! WiFi captive-portal configuration support.
//!
//! Gated behind the `wifi` cargo feature; the rest of the codebase compiles
//! without this module when the feature is absent.
//!
//! # Module layout
//!
//! - [`config`]  — WiFi credential flash storage (read/write/erase).
//! - [`driver`]  — CYW43439 hardware initialisation + scan + AP helpers.
//! - [`portal`]  — Embassy tasks for DNS catch-all + HTTP config portal.

pub mod config;
pub mod driver;
pub mod portal;

pub use config::WifiConfig;
pub use portal::{PortalCredentials, PORTAL_RESULT};
