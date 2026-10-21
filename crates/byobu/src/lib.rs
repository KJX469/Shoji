//! Byōbu — Shoji's ad and tracker shield.
//!
//! The matcher is Brave's `adblock` engine. Filter lists and scriptlet resources are never
//! bundled: [`updater`] fetches them from the uBlock Origin and Brave projects and keeps them
//! fresh with conditional requests, so upstream fixes reach Shoji within hours.

pub mod links;
pub mod shield;
pub mod sources;
pub mod updater;

pub use shield::{CosmeticPayload, Decision, Mode, RequestKind, Shield};
pub use updater::Updater;
