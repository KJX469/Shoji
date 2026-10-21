//! Byōbu — Shoji's ad and tracker shield.
//!
//! The matcher is Brave's `adblock` engine. Filter lists and scriptlet resources are never
//! bundled: [`updater`] fetches them from the uBlock Origin and Brave projects and keeps them
//! fresh with conditional requests, so upstream fixes reach Shoji within hours.

pub mod headers;
pub mod links;
pub mod matcher;
pub mod popups;
pub mod shield;
pub mod sources;
pub mod updater;

pub use matcher::{Answer, Matcher, Query};
pub use shield::{CosmeticPayload, Decision, Mode, RequestKind, Shield};
pub use updater::Updater;
