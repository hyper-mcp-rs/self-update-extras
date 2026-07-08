//! Self-updating support for CLI binaries.
//!
//! This crate provides two composable wrappers around any type implementing
//! [`self_update::update::ReleaseUpdate`], each exposed through a builder in
//! the style of `self_update`'s own backends:
//!
//! - [`throttle::Update`] limits how often update checks run by recording the
//!   time of the last check in a throttle file in the system temp directory.
//! - [`restart::Update`] re-executes the process with the freshly installed
//!   binary after a successful update, using a guard environment variable to
//!   prevent restart loops.
//!
//! Both wrappers implement `ReleaseUpdate` themselves and their builders
//! produce a `Box<dyn ReleaseUpdate>`, so they can be layered over a backend
//! (or over each other) and used anywhere a `ReleaseUpdate` is expected.
//!
//! # Example
//!
//! ```ignore
//! use auto_update::{restart, throttle};
//! use self_update::backends::github;
//! use self_update::update::ReleaseUpdate;
//! use std::time::Duration;
//!
//! // Any `ReleaseUpdate` implementation, e.g. a self_update GitHub backend.
//! let backend = github::Update::configure().build()?;
//!
//! let throttled = throttle::Update::configure()
//!     .release_update(backend)
//!     .throttle_window(Duration::from_secs(15 * 60))
//!     .build()?;
//!
//! let updater = restart::Update::configure()
//!     .release_update(throttled)
//!     .guard_env("MY_APP_AUTO_UPDATED")
//!     .build()?;
//!
//! let status = updater.update()?;
//! ```

pub mod restart;
pub mod throttle;

#[cfg(test)]
mod test_support;
