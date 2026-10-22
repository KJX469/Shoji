//! A thread that does nothing but match requests, so the engine's data stays warm in its CPU
//! caches instead of competing with everything else the UI thread touches.
//!
//! Two ways in: [`Matcher::check`] waits for the answer (a short spin, then a park), and
//! [`Matcher::submit`] returns at once and the answer is collected later with
//! [`Matcher::take_answers`] after the `ready` callback says some are waiting. Nothing is let
//! through unmatched: a caller waits as long as matching takes, and a matcher thread that died
//! is started again.

use crate::shield::{Decision, Mode, RequestKind, Shield};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// One request to decide on.
#[derive(Debug, Clone)]
pub struct Query {
    pub url: String,
    /// The top-level page
    pub source: String,
    pub kind: RequestKind,
    pub mode: Mode,
    pub method: String,
    /// Who asked (a tab id), passed through to the recorder
    pub tab: u64,
}

/// The decision, and the filter line behind it when it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub decision: Decision,
    pub rule: Option<String>,
}

/// Called on the matcher thread for every decision that isn't `Allow` (counters, the log).
pub type Recorder = Box<dyn Fn(&Query, &Answer) + Send + Sync>;

/// The reply slot of the thread that calls [`Matcher::check`], reused for every call. Each
/// call has its own number, so an answer to an earlier call (a query sent again while the
/// matcher was slow) is never taken for a later one.
struct Reply {
    /// The number of the latest call answered
    done: AtomicU64,
    answer: Mutex<Option<(u64, Answer)>>,
    waiter: Thread,
}

enum Job {
    Wait(Query, u64, Arc<Reply>),
    Later(Query, u64),
    /// A decision made elsewhere (from a cache) that still counts
    Record(Query, Answer),
}

struct Shared {
    shield: Shield,
    recorder: Option<Recorder>,
    /// Answers to `submit`ted queries, by ticket
    answers: Mutex<Vec<(u64, Answer)>>,
    /// Called when `answers` goes from empty to not empty
    ready: Option<Box<dyn Fn() + Send + Sync>>,
    /// Until when rule text is wanted (unix ms; it is cloned out of the engine only then)
    rules_until: AtomicU64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct Matcher {
    shared: Arc<Shared>,
    tx: Mutex<Sender<Job>>,
    /// The reply slot of the (single) thread that calls `check`, reused for every call
    reply: Mutex<Option<Arc<Reply>>>,
    /// Numbers `check` calls
    calls: AtomicU64,
}

/// How long the matcher keeps polling for the next request before it sleeps. Requests come in
/// bursts while a page loads; a sleeping thread takes tens of microseconds to wake.
const LINGER: Duration = Duration::from_micros(300);
/// How long a waiting caller spins before it parks.
const SPIN: Duration = Duration::from_micros(40);
/// How long a waiting caller waits before it asks again (the matcher may have stopped).
const RESEND: Duration = Duration::from_secs(5);

impl Matcher {
    /// Start the matcher for `shield`. `recorder` sees every non-allow decision; `ready` is
    /// called (on the matcher thread) when answers to submitted queries are waiting.
    pub fn spawn(
        shield: Shield,
        recorder: Option<Recorder>,
        ready: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let shared = Arc::new(Shared {
            shield,
            recorder,
            answers: Mutex::new(Vec::new()),
            ready,
            rules_until: AtomicU64::new(0),
        });
        let tx = start(shared.clone());
        Self {
            shared,
            tx: Mutex::new(tx),
            reply: Mutex::new(None),
            calls: AtomicU64::new(0),
        }
    }

    /// Include rule text with decisions for the next `for_ms` (while the logger is open).
    pub fn want_rules(&self, for_ms: u64) {
        self.shared
            .rules_until
            .store(now_ms() + for_ms, Ordering::Relaxed);
    }

    /// Pass a decision made from a cache to the recorder, off the caller's thread.
    pub fn record(&self, q: Query, a: Answer) {
        self.send(Job::Record(q, a));
    }

    fn send(&self, mut job: Job) {
        let Ok(tx) = self.tx.lock() else { return };
        let mut tx = tx;
        for _ in 0..3 {
            match tx.send(job) {
                Ok(()) => return,
                Err(mpsc::SendError(back)) => {
                    // The thread is gone (it panicked): start a new one and try again.
                    eprintln!("byobu: the matcher thread stopped; starting it again");
                    *tx = start(self.shared.clone());
                    job = back;
                }
            }
        }
    }

    /// Decide on `q`, waiting for the answer. Nothing is let through unanswered: a matcher
    /// that stopped is started again and asked again, for as long as it takes.
    pub fn check(&self, q: Query) -> Answer {
        let reply = {
            let mut slot = self.reply.lock().unwrap_or_else(|e| e.into_inner());
            let current = std::thread::current();
            match slot.as_ref() {
                Some(r) if r.waiter.id() == current.id() => r.clone(),
                _ => {
                    let r = Arc::new(Reply {
                        done: AtomicU64::new(0),
                        answer: Mutex::new(None),
                        waiter: current,
                    });
                    *slot = Some(r.clone());
                    r
                }
            }
        };
        let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        self.send(Job::Wait(q.clone(), call, reply.clone()));
        let start = Instant::now();
        let mut asked = start;
        loop {
            if reply.done.load(Ordering::Acquire) == call {
                let mut slot = reply.answer.lock().unwrap_or_else(|e| e.into_inner());
                if let Some((n, answer)) = slot.take() {
                    if n == call {
                        return answer;
                    }
                }
            }
            if start.elapsed() < SPIN {
                std::hint::spin_loop();
                continue;
            }
            std::thread::park_timeout(Duration::from_millis(50));
            // A matcher that died with this query is replaced by `send`; ask again.
            if asked.elapsed() > RESEND {
                eprintln!(
                    "byobu: no answer from the matcher after {:?}; asking again",
                    start.elapsed()
                );
                asked = Instant::now();
                self.send(Job::Wait(q.clone(), call, reply.clone()));
            }
        }
    }

    /// Queue `q` under `ticket`; its answer arrives through [`Matcher::take_answers`].
    pub fn submit(&self, q: Query, ticket: u64) {
        self.send(Job::Later(q, ticket));
    }

    /// Answers to submitted queries that have arrived since the last call.
    pub fn take_answers(&self) -> Vec<(u64, Answer)> {
        self.shared
            .answers
            .lock()
            .map(|mut a| std::mem::take(&mut *a))
            .unwrap_or_default()
    }
}

fn start(shared: Arc<Shared>) -> Sender<Job> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("byobu-matcher".into())
        .spawn(move || run(&shared, rx));
    tx
}

/// How long recent matches took on the matcher thread, in microseconds (most recent last).
static DECIDE_TIMES: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// (median, 99th percentile, count) of matches so far on the matcher thread, in microseconds.
pub fn decide_times() -> Option<(u32, u32, usize)> {
    let mut t = DECIDE_TIMES.lock().ok()?.clone();
    if t.is_empty() {
        return None;
    }
    t.sort_unstable();
    let at = |q: f64| t[((t.len() - 1) as f64 * q).round() as usize];
    Some((at(0.5), at(0.99), t.len()))
}

fn decide(shared: &Shared, q: &Query) -> Answer {
    let started = Instant::now();
    let answer = decide_inner(shared, q);
    if let Ok(mut t) = DECIDE_TIMES.lock() {
        if t.len() >= 100_000 {
            t.clear();
        }
        t.push(started.elapsed().as_micros() as u32);
    }
    answer
}

fn decide_inner(shared: &Shared, q: &Query) -> Answer {
    let (decision, rule) = shared
        .shield
        .check_request(&q.url, &q.source, q.kind, q.mode, &q.method);
    // Rule text comes from an engine kept only while the logger is open.
    let wanted = now_ms() < shared.rules_until.load(Ordering::Relaxed);
    if !wanted {
        shared.shield.forget_rule_names();
    }
    let rule = match (wanted, &decision) {
        (false, _) | (_, Decision::Allow) => None,
        _ => rule.or_else(|| shared.shield.rule_for(&q.url, &q.source, q.kind, &q.method)),
    };
    let answer = Answer { decision, rule };
    if answer.decision != Decision::Allow {
        if let Some(rec) = &shared.recorder {
            rec(q, &answer);
        }
    }
    answer
}

fn run(shared: &Shared, rx: Receiver<Job>) {
    let mut idle_since = Instant::now();
    loop {
        let job = match rx.try_recv() {
            Ok(job) => job,
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {
                if idle_since.elapsed() < LINGER {
                    std::hint::spin_loop();
                    continue;
                }
                match rx.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                }
            }
        };
        match job {
            Job::Record(q, a) => {
                if let Some(rec) = &shared.recorder {
                    rec(&q, &a);
                }
            }
            Job::Wait(q, call, reply) => {
                // A call already answered (it was sent again while slow) needs nothing more.
                if reply.done.load(Ordering::Acquire) >= call {
                    continue;
                }
                let answer = decide(shared, &q);
                if let Ok(mut a) = reply.answer.lock() {
                    *a = Some((call, answer));
                }
                reply.done.store(call, Ordering::Release);
                reply.waiter.unpark();
            }
            Job::Later(q, ticket) => {
                let answer = decide(shared, &q);
                let first = shared
                    .answers
                    .lock()
                    .map(|mut a| {
                        a.push((ticket, answer));
                        a.len() == 1
                    })
                    .unwrap_or(false);
                if first {
                    if let Some(ready) = &shared.ready {
                        ready();
                    }
                }
            }
        }
        idle_since = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shield() -> Shield {
        let s = Shield::new();
        assert!(s.load(vec![("||ads.example^\n".to_string(), true)], &[]));
        s
    }

    fn q(url: &str) -> Query {
        Query {
            url: url.into(),
            source: "https://news.test/".into(),
            kind: RequestKind::Script,
            mode: Mode::Standard,
            method: "GET".into(),
            tab: 7,
        }
    }

    #[test]
    fn waiting_and_queued_checks_agree_with_the_engine() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let m = Matcher::spawn(
            shield(),
            Some(Box::new(move |q, a| {
                s2.lock().unwrap().push((q.tab, a.decision.clone()))
            })),
            None,
        );
        assert_eq!(
            m.check(q("https://ads.example/x.js")).decision,
            Decision::Block
        );
        assert_eq!(
            m.check(q("https://news.test/app.js")).decision,
            Decision::Allow
        );
        for i in 0..100 {
            let url = if i % 2 == 0 {
                "https://ads.example/a.js"
            } else {
                "https://news.test/b.js"
            };
            m.submit(q(url), i);
        }
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_secs(5);
        while got.len() < 100 && Instant::now() < end {
            got.extend(m.take_answers());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got.len(), 100);
        for (t, a) in got {
            let want = if t % 2 == 0 {
                Decision::Block
            } else {
                Decision::Allow
            };
            assert_eq!(a.decision, want, "ticket {t}");
        }
        // Only blocks were recorded: one from `check`, fifty queued.
        assert_eq!(seen.lock().unwrap().len(), 51);
    }

    #[test]
    fn rule_text_only_when_asked() {
        let m = Matcher::spawn(shield(), None, None);
        assert_eq!(m.check(q("https://ads.example/x.js")).rule, None);
        m.want_rules(60_000);
        // The engine with rule text is built on the first ask.
        let end = Instant::now() + Duration::from_secs(10);
        let mut rule = None;
        while rule.is_none() && Instant::now() < end {
            rule = m.check(q("https://ads.example/x.js")).rule;
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(rule.as_deref(), Some("||ads.example^"));
    }
}
