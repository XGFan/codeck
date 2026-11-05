//! Background worker: consumes jobs (full scan, FS batches, text backlog, rebuild, root swap)
//! over a channel and emits events.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use codeck_core::model::{AppInfo, IndexStatus, SessionsChanged};
use codeck_core::{ChangeSet, CoreError, CoreResult, Engine, paths};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::events::{IndexStatusEvent, LiveChangedEvent, PricesChangedEvent, SessionsChangedEvent};
use crate::prices;
use crate::settings::{self, Settings};
use crate::state::{AppState, open_engine};
use crate::watcher::{self, Watcher};

const LIVE_POLL: Duration = Duration::from_secs(3);
const RESCAN_EVERY_POLLS: u32 = 200; // 10 minutes
const STATUS_INTERVAL: Duration = Duration::from_millis(200); // <= 5 events/s

#[derive(Debug)]
pub enum Job {
    /// Stat walk + metadata parse; followed by `TextBacklog`.
    FullScan,
    /// Debounced filesystem batch (paths under `<root>/projects` or `<root>/sessions`).
    Fs(Vec<PathBuf>),
    /// Requests the full-text backlog. Requests coalesce; the backlog runs whenever no other job
    /// is queued and yields to any other job between transactions (A14b).
    TextBacklog,
    Rebuild,
    /// Restart the watcher and rescan (at startup).
    SetRoot,
    /// Saves the settings, opens an Engine on their data root and swaps it in, then restarts the
    /// watcher and rescans. Done here, between jobs, so the old Engine is not writing to the index
    /// the new one opens (both use the same file).
    SwapRoot {
        settings: Settings,
        reply: Sender<CoreResult<AppInfo>>,
    },
    PollLive,
    /// Periodic stat rescan; also refreshes the watcher (new symlinked projects).
    Rescan,
    /// Fetches the price list in the background when the cached one is a day old.
    RefreshPrices,
    /// A fetched price list (LiteLLM JSON) to adopt.
    ApplyPrices(String),
}

/// Queues jobs; any job except `TextBacklog` also asks a running backlog to yield (after its
/// current transaction commits).
#[derive(Clone)]
pub struct JobSender {
    tx: Sender<Job>,
    backlog_cancel: Arc<AtomicBool>,
}

impl JobSender {
    /// Returns false once the worker is gone.
    pub fn send(&self, job: Job) -> bool {
        if !matches!(job, Job::TextBacklog) {
            self.backlog_cancel.store(true, Ordering::Relaxed);
        }
        self.tx.send(job).is_ok()
    }
}

pub fn channel() -> (JobSender, Receiver<Job>, Arc<AtomicBool>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    (
        JobSender {
            tx,
            backlog_cancel: cancel.clone(),
        },
        rx,
        cancel,
    )
}

/// Spawns the worker and the timer feeding it live polls and periodic rescans.
pub fn spawn(app: AppHandle, rx: Receiver<Job>, jobs: JobSender, backlog_cancel: Arc<AtomicBool>) {
    let timer_jobs = jobs.clone();
    std::thread::Builder::new()
        .name("cv-timer".into())
        .spawn(move || {
            let mut polls = 0u32;
            loop {
                std::thread::sleep(LIVE_POLL);
                polls += 1;
                let job = if polls.is_multiple_of(RESCAN_EVERY_POLLS) {
                    Job::Rescan
                } else {
                    Job::PollLive
                };
                let refresh = polls.is_multiple_of(RESCAN_EVERY_POLLS);
                if !timer_jobs.send(job) || (refresh && !timer_jobs.send(Job::RefreshPrices)) {
                    break;
                }
            }
        })
        .expect("spawn timer thread");
    std::thread::Builder::new()
        .name("cv-worker".into())
        .spawn(move || {
            Worker {
                app,
                queue: Queue::new(rx, backlog_cancel.clone()),
                jobs,
                backlog_cancel,
                watcher: None,
                last_status: Arc::new(Mutex::new(None)),
                fetching_prices: Arc::new(AtomicBool::new(false)),
            }
            .run()
        })
        .expect("spawn worker thread");
}

/// What the worker does next.
#[derive(Debug)]
enum Step {
    Job(Job),
    Backlog,
}

/// The job queue plus the coalesced text-backlog request. Queued jobs always go first; the
/// backlog runs only when the queue is empty and stays requested until it completes.
struct Queue {
    rx: Receiver<Job>,
    backlog_cancel: Arc<AtomicBool>,
    backlog: bool,
}

impl Queue {
    fn new(rx: Receiver<Job>, backlog_cancel: Arc<AtomicBool>) -> Self {
        Queue {
            rx,
            backlog_cancel,
            backlog: false,
        }
    }

    /// Blocks for the next step; `None` once every sender is gone.
    fn next(&mut self) -> Option<Step> {
        loop {
            if self.backlog {
                // Cleared before looking at the queue: any job sent from here on raises it again,
                // so a running backlog yields to it.
                self.backlog_cancel.store(false, Ordering::Relaxed);
                match self.rx.try_recv() {
                    Ok(Job::TextBacklog) => continue,
                    Ok(job) => return Some(Step::Job(job)),
                    Err(TryRecvError::Empty) => return Some(Step::Backlog),
                    Err(TryRecvError::Disconnected) => return None,
                }
            }
            match self.rx.recv() {
                Ok(Job::TextBacklog) => self.backlog = true,
                Ok(job) => return Some(Step::Job(job)),
                Err(_) => return None,
            }
        }
    }
}

struct Worker {
    app: AppHandle,
    queue: Queue,
    jobs: JobSender,
    backlog_cancel: Arc<AtomicBool>,
    watcher: Option<Watcher>,
    /// Time of the last emitted progress status (throttle).
    last_status: Arc<Mutex<Option<Instant>>>,
    /// A price fetch is in flight.
    fetching_prices: Arc<AtomicBool>,
}

fn log_err(op: &str, e: &CoreError) {
    match e {
        CoreError::Cancelled => tracing::debug!("{op}: {e}"),
        _ => tracing::warn!("{op} failed: {e}"),
    }
}

impl Worker {
    fn run(mut self) {
        while let Some(step) = self.queue.next() {
            let engine = self.app.state::<AppState>().engine();
            let job = match step {
                Step::Backlog => {
                    self.queue.backlog = self.text_backlog(&engine);
                    continue;
                }
                Step::Job(job) => job,
            };
            match job {
                Job::SetRoot | Job::Rescan => {
                    self.restart_watcher(&engine);
                    self.full_scan(&engine);
                }
                Job::SwapRoot { settings, reply } => self.swap_root(engine, &settings, &reply),
                Job::FullScan => self.full_scan(&engine),
                Job::Fs(paths) => self.fs_batch(&engine, &paths),
                // `Queue::next` absorbs backlog requests; kept for exhaustiveness.
                Job::TextBacklog => self.queue.backlog = true,
                Job::Rebuild => {
                    let result = engine.rebuild(&self.progress());
                    self.scan_done(&engine, "rebuild", result);
                }
                Job::PollLive => self.poll_live(&engine),
                Job::RefreshPrices => self.refresh_prices(&engine),
                Job::ApplyPrices(json) => match engine.update_prices(&json) {
                    Ok(true) => {
                        let _ = PricesChangedEvent.emit(&self.app);
                    }
                    Ok(false) => {}
                    Err(e) => log_err("update prices", &e),
                },
            }
        }
    }

    /// Fetches off the worker thread (a slow network must not hold up indexing) and hands the
    /// result back as `ApplyPrices`. Failures keep the current table (cache or embedded snapshot).
    fn refresh_prices(&self, engine: &Engine) {
        if prices::is_fresh(engine.prices_fetched_at())
            || self.fetching_prices.swap(true, Ordering::SeqCst)
        {
            return;
        }
        let jobs = self.jobs.clone();
        let fetching = self.fetching_prices.clone();
        let spawned = std::thread::Builder::new()
            .name("cv-prices".into())
            .spawn(move || {
                match prices::fetch() {
                    Ok(json) => {
                        jobs.send(Job::ApplyPrices(json));
                    }
                    Err(e) => tracing::warn!("fetch prices: {e}"),
                }
                fetching.store(false, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.fetching_prices.store(false, Ordering::SeqCst);
        }
    }

    fn swap_root(&mut self, old: Arc<Engine>, s: &Settings, reply: &Sender<CoreResult<AppInfo>>) {
        // Release this thread's handle first: if it was the last one, the old index connections
        // close before the new Engine may replace the index file.
        drop(old);
        let opened = settings::save(s)
            .map_err(CoreError::from)
            .and_then(|()| open_engine(s));
        match opened {
            Ok(engine) => {
                let engine = Arc::new(engine);
                *self.app.state::<AppState>().engine.write() = engine.clone();
                let _ = reply.send(Ok(engine.app_info()));
                self.restart_watcher(&engine);
                self.full_scan(&engine);
            }
            Err(e) => {
                let _ = reply.send(Err(e));
            }
        }
    }

    fn restart_watcher(&mut self, engine: &Engine) {
        self.watcher = None;
        let root = PathBuf::from(engine.app_info().data_root);
        self.watcher = watcher::start(&root, self.jobs.clone());
        if let Some(w) = &self.watcher {
            tracing::info!("watching {} dirs under {}", w.watched, root.display());
        }
    }

    fn emit_status(&self, status: IndexStatus) {
        let _ = IndexStatusEvent(status).emit(&self.app);
    }

    fn emit_error(&self, engine: &Engine, e: &CoreError) {
        let mut status = engine.index_status();
        status.error = Some(e.to_string());
        self.emit_status(status);
    }

    fn emit_changes(&self, cs: ChangeSet) {
        if cs.changed.is_empty() && cs.removed.is_empty() && !cs.projects_changed {
            return;
        }
        let _ = SessionsChangedEvent(SessionsChanged {
            changed: cs.changed,
            removed: cs.removed,
            projects_changed: cs.projects_changed,
        })
        .emit(&self.app);
    }

    /// Throttled progress callback for the engine.
    fn progress(&self) -> impl Fn(&IndexStatus) + 'static {
        let app = self.app.clone();
        let last = self.last_status.clone();
        move |status: &IndexStatus| {
            let mut last = last.lock();
            let now = Instant::now();
            if last.is_some_and(|t| now.duration_since(t) < STATUS_INTERVAL) {
                return;
            }
            *last = Some(now);
            let _ = IndexStatusEvent(status.clone()).emit(&app);
        }
    }

    fn full_scan(&self, engine: &Engine) {
        let result = engine.scan_all(&self.progress());
        self.scan_done(engine, "scan_all", result);
    }

    /// Emits a full scan's (or rebuild's) changes and queues the text backlog.
    fn scan_done(&self, engine: &Engine, op: &str, result: Result<ChangeSet, CoreError>) {
        match result {
            Ok(cs) => {
                self.emit_changes(cs);
                self.emit_status(engine.index_status());
                self.jobs.send(Job::TextBacklog);
            }
            Err(e) => {
                log_err(op, &e);
                self.emit_error(engine, &e);
            }
        }
    }

    fn fs_batch(&self, engine: &Engine, paths: &[PathBuf]) {
        let sessions_dir = paths::sessions_dir(&PathBuf::from(engine.app_info().data_root));
        let (live, project): (Vec<&PathBuf>, Vec<&PathBuf>) =
            paths.iter().partition(|p| p.starts_with(&sessions_dir));
        if !project.is_empty() {
            let project: Vec<PathBuf> = project.into_iter().cloned().collect();
            match engine.apply_changes(&project) {
                Ok(cs) => {
                    let any = !cs.changed.is_empty() || !cs.removed.is_empty();
                    self.emit_changes(cs);
                    self.emit_status(engine.index_status());
                    if any {
                        self.jobs.send(Job::TextBacklog);
                    }
                }
                Err(e) => log_err("apply_changes", &e),
            }
        }
        if !live.is_empty() {
            self.poll_live(engine);
        }
    }

    fn poll_live(&self, engine: &Engine) {
        match engine.poll_live() {
            Ok(Some(live)) => {
                let _ = LiveChangedEvent(live).emit(&self.app);
            }
            Ok(None) => {}
            Err(e) => log_err("poll_live", &e),
        }
    }

    /// Runs the backlog until done or until another job is queued (it yields after the current
    /// transaction). Returns whether the backlog is still wanted.
    fn text_backlog(&self, engine: &Engine) -> bool {
        let result = engine.index_text_backlog(&self.progress(), &self.backlog_cancel);
        self.emit_status(engine.index_status());
        match result {
            Ok(()) => false,
            Err(CoreError::Cancelled) => true,
            Err(e) => {
                log_err("index_text_backlog", &e);
                self.emit_error(engine, &e);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> (JobSender, Queue) {
        let (jobs, rx, cancel) = channel();
        (jobs, Queue::new(rx, cancel))
    }

    #[test]
    fn backlog_requests_coalesce_and_run_after_queued_jobs() {
        let (jobs, mut q) = queue();
        // Two backlog requests used to requeue each other forever (worker livelock).
        jobs.send(Job::TextBacklog);
        jobs.send(Job::TextBacklog);
        jobs.send(Job::PollLive);
        jobs.send(Job::TextBacklog);
        assert!(matches!(q.next(), Some(Step::Job(Job::PollLive))));
        assert!(matches!(q.next(), Some(Step::Backlog)));
        assert!(q.rx.is_empty());
    }

    #[test]
    fn backlog_yields_to_new_jobs_and_resumes() {
        let (jobs, mut q) = queue();
        jobs.send(Job::TextBacklog);
        assert!(matches!(q.next(), Some(Step::Backlog)));
        assert!(!q.backlog_cancel.load(Ordering::Relaxed));
        // A poll arriving while the backlog runs asks it to yield.
        jobs.send(Job::PollLive);
        assert!(q.backlog_cancel.load(Ordering::Relaxed));
        // The backlog yielded (still wanted): the poll runs first, then the backlog resumes.
        assert!(matches!(q.next(), Some(Step::Job(Job::PollLive))));
        assert!(matches!(q.next(), Some(Step::Backlog)));
        assert!(!q.backlog_cancel.load(Ordering::Relaxed));
        // Completed: the queue blocks again until the next job.
        q.backlog = false;
        jobs.send(Job::Rescan);
        assert!(matches!(q.next(), Some(Step::Job(Job::Rescan))));
        drop(jobs);
        assert!(q.next().is_none());
    }
}
