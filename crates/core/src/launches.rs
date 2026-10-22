//! Which version ran last and whether it got through its first minute, kept in
//! `<data>/launches.json` (version numbers only, so it is kept in Private mode too). A version
//! that closes unexpectedly twice that early is offered a way back to the one before it.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// A run that ends before this many seconds without quitting normally counts as an early close.
pub const SETTLE_SECS: u64 = 60;
/// Early closes in a row before going back is offered.
const OFFER_AFTER: u32 = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Launches {
    /// The version of the latest run
    pub version: String,
    /// That run hasn't yet lasted [`SETTLE_SECS`] or quit normally
    pub open: bool,
    /// Runs of `version` in a row that closed unexpectedly within [`SETTLE_SECS`]
    pub early_closes: u32,
    /// The version that ran before `version`
    pub previous: String,
    /// The user chose to keep `version` anyway
    pub kept: bool,
}

impl Launches {
    pub fn load(path: &Path) -> Self {
        crate::userdata::read_text(path)
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string(self) {
            let _ = crate::userdata::write_atomic(path, &json);
        }
    }

    /// Record the start of a run of `version`: the last run counts as an early close if it was
    /// still open, and a new version starts its count again.
    pub fn start(path: &Path, version: &str) -> Self {
        let mut l = Self::load(path);
        if l.version == version {
            if l.open {
                l.early_closes = l.early_closes.saturating_add(1);
            }
        } else {
            if !l.version.is_empty() {
                l.previous = std::mem::take(&mut l.version);
            }
            l.version = version.to_string();
            l.early_closes = 0;
            l.kept = false;
        }
        l.open = true;
        l.save(path);
        l
    }

    /// The run got through its first minute, or quit normally before it.
    pub fn settle(path: &Path) {
        let mut l = Self::load(path);
        if l.open || l.early_closes > 0 {
            l.open = false;
            l.early_closes = 0;
            l.save(path);
        }
    }

    /// The user wants to keep this version: don't offer to go back again for it.
    pub fn keep(path: &Path) {
        let mut l = Self::load(path);
        l.kept = true;
        l.save(path);
    }

    /// The older version to offer going back to, once this one keeps closing early.
    pub fn rollback_to(&self) -> Option<&str> {
        (self.early_closes >= OFFER_AFTER
            && !self.kept
            && crate::env::is_newer_release(&self.version, &self.previous))
        .then_some(self.previous.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_early_closes_of_a_new_version_offer_the_previous_one() {
        let dir = std::env::temp_dir().join(format!("shoji-launches-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launches.json");

        // 0.1.6 ran fine, then 0.1.7 is installed.
        Launches::start(&path, "0.1.6");
        Launches::settle(&path);
        let l = Launches::start(&path, "0.1.7");
        assert_eq!((l.previous.as_str(), l.early_closes), ("0.1.6", 0));
        // It closes early twice: the third start offers 0.1.6.
        assert_eq!(Launches::start(&path, "0.1.7").rollback_to(), None);
        let l = Launches::start(&path, "0.1.7");
        assert_eq!(l.early_closes, 2);
        assert_eq!(l.rollback_to(), Some("0.1.6"));
        // Keeping it stops the offer; a run that settles resets the count.
        Launches::keep(&path);
        assert_eq!(Launches::start(&path, "0.1.7").rollback_to(), None);
        Launches::settle(&path);
        let l = Launches::start(&path, "0.1.7");
        assert_eq!((l.early_closes, l.open), (0, true));

        // Going back to 0.1.6 never offers the newer 0.1.7 as a way "back".
        Launches::start(&path, "0.1.6");
        Launches::start(&path, "0.1.6");
        let l = Launches::start(&path, "0.1.6");
        assert_eq!(l.previous, "0.1.7");
        assert_eq!(l.rollback_to(), None);

        // A first install has nothing to go back to.
        let _ = std::fs::remove_file(&path);
        for _ in 0..3 {
            Launches::start(&path, "0.1.7");
        }
        assert_eq!(Launches::load(&path).rollback_to(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
