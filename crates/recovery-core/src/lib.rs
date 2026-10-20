//! JFileRescue core library: read-only file system parsing, deleted-entry
//! discovery, signature carving, session persistence and file write-out.
//!
//! This crate has no GUI dependency and is shared by the CLI and the Tauri app.

pub mod carve;
pub mod device;
pub mod error;
pub mod fs;
pub mod image;
pub mod model;
pub mod partition;
pub mod progress;
pub mod recover;
pub mod scan;
pub mod session;
pub mod util;

pub use error::{Error, Result};
