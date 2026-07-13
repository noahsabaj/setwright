//! Setwright's local-first domain core.
//!
//! The modules in this tree deliberately have no dependency on Tauri.  The
//! desktop command layer is an adapter: source bytes, revisions, persistence,
//! review state, preflight policy, and compile specifications live here.

pub mod compile;
pub mod contracts;
pub mod error;
pub mod latex;
pub mod persistence;
pub mod preflight;
pub mod project;
pub mod review;
pub mod snapshot;
pub mod source;

pub use compile::*;
pub use contracts::*;
pub use error::{AppError, AppResult};
pub use latex::*;
pub use persistence::*;
pub use preflight::*;
pub use project::*;
pub use review::*;
pub use snapshot::*;
pub use source::*;
