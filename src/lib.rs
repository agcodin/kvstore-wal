//! A small durable key-value store: an in-memory ordered index over an
//! append-only, checksummed write-ahead log.

pub mod record;
pub mod server;
pub mod sim;
pub mod soak;
pub mod storage;
pub mod store;

pub use storage::{FileStorage, Storage};
pub use store::{Error, Options, Recovery, Result, Store, SyncPolicy};
