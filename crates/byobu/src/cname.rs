//! CNAME uncloaking: a tracker served from a site's own subdomain (`metrics.news.test`, a
//! CNAME for `news.eulerian.net`) is matched under the name it really points to.
//!
//! Lookups never hold a request. The first request to a new host is matched by its own name
//! while a resolver thread looks it up; later ones use the cached canonical name. Only
//! first-party subdomains are looked up: a third-party host is already matched by its name.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a lookup found: the name at the end of the CNAME chain (`None` when there is none),
/// and how long the answer may be kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub canonical: Option<String>,
    pub ttl: Duration,
}

/// Looks a host up (blocking). `None` when the lookup failed.
pub type Resolve = dyn Fn(&str) -> Option<Resolution> + Send + Sync;

/// Answers are kept at least this long, so a short DNS TTL doesn't mean a lookup per page.
const MIN_TTL: Duration = Duration::from_secs(5 * 60);
/// And at most this long.
const MAX_TTL: Duration = Duration::from_secs(60 * 60);
/// A failed lookup is tried again after this.
const RETRY: Duration = Duration::from_secs(60);
/// A host whose lookup hasn't come back is queued again after this.
const PENDING: Duration = Duration::from_secs(15);
/// How many hosts are remembered.
const HOSTS: usize = 4096;
/// Lookups run on this many threads, so one slow name doesn't hold up the rest.
const THREADS: usize = 2;

enum Entry {
    Pending(Instant),
    Known {
        canonical: Option<String>,
        until: Instant,
    },
}

#[derive(Default)]
struct Cache {
    hosts: HashMap<String, Entry>,
    order: VecDeque<String>,
}

impl Cache {
    fn insert(&mut self, host: String, entry: Entry) {
        if !self.hosts.contains_key(&host) {
            if self.hosts.len() >= HOSTS {
                if let Some(old) = self.order.pop_front() {
                    self.hosts.remove(&old);
                }
            }
            self.order.push_back(host.clone());
        }
        self.hosts.insert(host, entry);
    }
}

/// The canonical-name cache and its resolver threads. Cheap to clone and share.
#[derive(Clone)]
pub struct Uncloaker {
    cache: Arc<Mutex<Cache>>,
    queue: Sender<String>,
    /// Bumped when a host turns out to be cloaked (or stops being), so decisions remembered
    /// for it are dropped
    epoch: Arc<AtomicU64>,
}

impl Uncloaker {
    /// Start the resolver threads.
    pub fn spawn(resolve: Box<Resolve>) -> Self {
        let (queue, rx) = mpsc::channel::<String>();
        let rx = Arc::new(Mutex::new(rx));
        let resolve: Arc<Resolve> = Arc::from(resolve);
        let me = Self {
            cache: Arc::default(),
            queue,
            epoch: Arc::default(),
        };
        for i in 0..THREADS {
            let (me, rx, resolve) = (me.clone(), rx.clone(), resolve.clone());
            let _ = std::thread::Builder::new()
                .name(format!("byobu-cname-{i}"))
                .spawn(move || me.run(&rx, &*resolve));
        }
        me
    }

    fn run(&self, rx: &Mutex<Receiver<String>>, resolve: &Resolve) {
        loop {
            let host = match rx.lock() {
                Ok(rx) => match rx.recv() {
                    Ok(h) => h,
                    Err(_) => return,
                },
                Err(_) => return,
            };
            let found = resolve(&host);
            self.learn(&host, found);
        }
    }

    /// Record what a lookup of `host` found.
    fn learn(&self, host: &str, found: Option<Resolution>) {
        let now = Instant::now();
        let (canonical, until) = match found {
            Some(r) => {
                let canonical = r
                    .canonical
                    .map(|c| c.trim_end_matches('.').to_ascii_lowercase())
                    .filter(|c| !c.is_empty() && c != host);
                (canonical, now + r.ttl.clamp(MIN_TTL, MAX_TTL))
            }
            None => (None, now + RETRY),
        };
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        let before = match cache.hosts.get(host) {
            Some(Entry::Known { canonical, .. }) => canonical.clone(),
            _ => None,
        };
        if before != canonical {
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
        cache.insert(host.to_string(), Entry::Known { canonical, until });
    }

    /// The canonical name of `host` if it is known to have one. An unknown or expired host is
    /// queued for a lookup, and `None` is returned until it is back.
    pub fn canonical(&self, host: &str) -> Option<String> {
        let now = Instant::now();
        let Ok(mut cache) = self.cache.lock() else {
            return None;
        };
        let (answer, stale) = match cache.hosts.get(host) {
            Some(Entry::Known { canonical, until }) => (canonical.clone(), *until <= now),
            Some(Entry::Pending(since)) => (None, now.duration_since(*since) > PENDING),
            None => (None, true),
        };
        if stale {
            match cache.hosts.get_mut(host) {
                // An expired answer is still used while it is looked up again.
                Some(Entry::Known { until, .. }) => *until = now + PENDING,
                _ => cache.insert(host.to_string(), Entry::Pending(now)),
            }
            drop(cache);
            let _ = self.queue.send(host.to_string());
        }
        answer
    }

    /// Changes whenever a host's canonical name changes.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
}

/// Whether `host` is worth looking up: a dotted name, not an address or a local name.
pub fn worth_resolving(host: &str) -> bool {
    !host.is_empty()
        && host.contains('.')
        && !host.starts_with('[')
        && host.parse::<std::net::Ipv4Addr>().is_err()
        && !host.ends_with(".local")
        && !host.ends_with(".localhost")
        && !host.ends_with(".internal")
        && host != "localhost"
}

/// `url` with its host replaced by `canonical`, cut to the origin: only host rules count, so
/// a path rule written for one site never matches another site's CDN.
pub fn origin_with_host(url: &str, canonical: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(format!("{}://{}/", u.scheme(), canonical))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(u: &Uncloaker, host: &str) -> Option<String> {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(c) = u.canonical(host) {
                return Some(c);
            }
            if Instant::now() > end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn names_are_looked_up_once_and_cached() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let c2 = calls.clone();
        let u = Uncloaker::spawn(Box::new(move |h: &str| {
            c2.lock().unwrap().push(h.to_string());
            Some(Resolution {
                canonical: (h == "metrics.news.test").then(|| "News.Tracker.Example.".into()),
                ttl: Duration::from_secs(30),
            })
        }));
        // Before the first ask: the lookup it queues may finish at once.
        let e0 = u.epoch();
        assert_eq!(
            u.canonical("metrics.news.test"),
            None,
            "first ask is queued"
        );
        assert_eq!(
            wait_for(&u, "metrics.news.test").as_deref(),
            Some("news.tracker.example")
        );
        assert!(u.epoch() > e0, "a cloaked host bumps the epoch");
        let e1 = u.epoch();
        assert_eq!(u.canonical("cdn.news.test"), None);
        let end = Instant::now() + Duration::from_secs(5);
        while !calls.lock().unwrap().iter().any(|h| h == "cdn.news.test") && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(u.canonical("cdn.news.test"), None);
        assert_eq!(u.epoch(), e1, "a plain host doesn't");
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.iter().filter(|h| *h == "metrics.news.test").count(),
            1
        );
        assert_eq!(calls.iter().filter(|h| *h == "cdn.news.test").count(), 1);
    }

    #[test]
    fn local_names_and_addresses_are_skipped() {
        assert!(worth_resolving("metrics.news.test"));
        for h in [
            "localhost",
            "printer.local",
            "10.0.0.1",
            "[::1]",
            "intranet",
            "",
        ] {
            assert!(!worth_resolving(h), "{h}");
        }
    }

    #[test]
    fn replay_keeps_only_the_origin() {
        assert_eq!(
            origin_with_host("https://m.news.test/ads/pixel.gif?x=1", "t.example").as_deref(),
            Some("https://t.example/")
        );
    }
}
