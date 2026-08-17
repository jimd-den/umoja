//! Adapters: the only crate that touches the filesystem, the network,
//! interpreters or other processes.
//!
//! Everything here implements a trait from [`pa_domain::ports`]. Nothing here
//! makes a decision that a use case could have made instead.

#![forbid(unsafe_code)]

pub mod files;
pub mod gates;
pub mod hash;
pub mod kernel;
pub mod lock;
pub mod paths;
pub mod runners;
pub mod skills_fs;
pub mod stores;
pub mod summariser;
pub mod sys;
pub mod table;
