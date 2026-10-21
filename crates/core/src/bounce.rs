//! Bounce-tracking cleanup, as in Safari and Firefox: a site that only ever shows up as a hop in
//! a redirect chain (a tracker bouncing you through itself to set or read its cookie), and that
//! you never visit yourself, has its cookies and storage deleted a week after it was first seen.
//! No list is needed: the browser sees every navigation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const DAY: u64 = 24 * 3600;
/// A hop-only site is cleaned this long after it was first seen.
pub const GRACE: u64 = 7 * DAY;
/// A visit counts against cleaning for this long.
const VISIT_KEEP: u64 = 45 * DAY;
/// At most this many sites are tracked as hops (the oldest go first).
const MAX_HOPS: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Hop {
    /// When the site was first seen as a hop (unix seconds)
    pub first: u64,
    /// The hosts it was seen on, whose data is cleared
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BounceLog {
    /// Site (registrable domain) -> hop record
    pub hops: BTreeMap<String, Hop>,
    /// Site -> last time you visited it yourself
    pub visited: BTreeMap<String, u64>,
}

impl BounceLog {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    /// `host` (on `site`) was a hop in a redirect chain.
    pub fn hop(&mut self, site: &str, host: &str, now: u64) {
        if self.visited.contains_key(site) {
            return;
        }
        let entry = self.hops.entry(site.to_string()).or_insert_with(|| Hop {
            first: now,
            hosts: Vec::new(),
        });
        if !entry.hosts.iter().any(|h| h == host) && entry.hosts.len() < 16 {
            entry.hosts.push(host.to_string());
        }
        if self.hops.len() > MAX_HOPS {
            if let Some(oldest) = self
                .hops
                .iter()
                .min_by_key(|(_, h)| h.first)
                .map(|(s, _)| s.clone())
            {
                self.hops.remove(&oldest);
            }
        }
    }

    /// You visited `site` yourself: it is never cleaned while the visit is recent.
    pub fn visit(&mut self, site: &str, now: u64) {
        self.visited.insert(site.to_string(), now);
        self.hops.remove(site);
    }

    /// Hop-only sites due for cleaning, with their hosts. Old visits are forgotten here too.
    pub fn due(&mut self, now: u64) -> Vec<(String, Vec<String>)> {
        self.visited
            .retain(|_, at| now.saturating_sub(*at) < VISIT_KEEP);
        self.hops
            .iter()
            .filter(|(site, h)| {
                now.saturating_sub(h.first) >= GRACE && !self.visited.contains_key(*site)
            })
            .map(|(site, h)| (site.clone(), h.hosts.clone()))
            .collect()
    }

    /// The site's data was cleaned; it starts over.
    pub fn forget(&mut self, site: &str) {
        self.hops.remove(site);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_only_sites_are_cleaned_after_a_week_unless_visited() {
        let mut log = BounceLog::default();
        log.hop("tracker.example", "t.tracker.example", 0);
        log.hop("tracker.example", "x.tracker.example", 10);
        log.hop("shop.example", "go.shop.example", 0);
        assert!(log.due(GRACE - 1).is_empty(), "not before a week");
        log.visit("shop.example", 100);
        let due = log.due(GRACE);
        assert_eq!(
            due,
            vec![(
                "tracker.example".to_string(),
                vec![
                    "t.tracker.example".to_string(),
                    "x.tracker.example".to_string()
                ]
            )]
        );
        log.forget("tracker.example");
        assert!(log.due(GRACE * 2).is_empty());
        // A site you visit isn't recorded as a hop again while the visit is recent...
        log.hop("shop.example", "go.shop.example", GRACE);
        assert!(log.due(GRACE * 3).is_empty());
        // ...but is once the visit is long past.
        log.due(200 * DAY);
        log.hop("shop.example", "go.shop.example", 200 * DAY);
        assert_eq!(log.due(210 * DAY).len(), 1);
    }

    #[test]
    fn saves_and_loads() {
        let path = std::env::temp_dir().join(format!("shoji-bounce-{}.json", std::process::id()));
        let mut log = BounceLog::default();
        log.hop("a.example", "a.example", 5);
        log.visit("b.example", 6);
        log.save(&path);
        assert_eq!(BounceLog::load(&path), log);
        let _ = std::fs::remove_file(&path);
        assert_eq!(BounceLog::load(&path), BounceLog::default());
    }
}
