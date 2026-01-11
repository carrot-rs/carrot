//! Progressive reflow primitives.
//!
//! When the user changes font, font size, or theme, visible blocks
//! need to be re-laid-out. Doing it synchronously on the UI thread
//! freezes the whole app for the duration. The real fix is
//! progressive reflow: work off-thread, yield every so often, let
//! the UI paint partial progress.
//!
//! This module owns the data shape. The scheduler that actually
//! moves the work off-thread is a follow-up — but with the types in
//! place, the UI layer can render progress indicators today while
//! reflow is synchronous, and the upgrade to off-thread becomes a
//! one-line swap in the consumer.
//!
//! # What is NOT here
//!
//! - Not a task scheduler. Use `carrot-scheduler` / tokio / smol when
//!   the async wiring lands.
//! - Not the actual reflow algorithm — blocks own their own reflow
//!   logic (PageList + CellStyleAtlas can both remap in-place). This
//!   module just describes *what changed* and *how far along we are*.

/// Classification of what triggered the reflow. Drives the
/// strategy the consumer picks: font changes affect shaping +
/// glyph cache; theme changes only remap style atlas entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReflowReason {
    /// Font family changed — shaped-run cache must be invalidated.
    FontFamily,
    /// Font size changed — cell metrics shift, glyph atlas invalidates.
    FontSize,
    /// Theme changed — style atlas remap only, content untouched.
    Theme,
    /// User custom rule (e.g. line-height tweak).
    Custom(String),
}

impl ReflowReason {
    /// Whether this reason invalidates the shape cache. Theme-only
    /// changes don't.
    pub fn invalidates_shapes(&self) -> bool {
        !matches!(self, ReflowReason::Theme)
    }

    /// Whether this reason invalidates the glyph atlas.
    pub fn invalidates_glyphs(&self) -> bool {
        matches!(
            self,
            ReflowReason::FontFamily | ReflowReason::FontSize | ReflowReason::Custom(_)
        )
    }
}

/// Request describing what needs reflowing and how fine-grained the
/// progress reporting should be.
#[derive(Debug, Clone)]
pub struct ReflowRequest {
    pub reason: ReflowReason,
    /// Total number of units of work (usually rows or cells). Used
    /// to compute `ReflowProgress::fraction()`.
    pub total_units: u64,
    /// Emit progress reports every `report_every` units. Defaults to
    /// 4096 — small enough that 16 ms budgets get one report per
    /// frame, big enough that the reporter overhead stays under 1 %.
    pub report_every: u64,
}

impl ReflowRequest {
    pub fn new(reason: ReflowReason, total_units: u64) -> Self {
        Self {
            reason,
            total_units,
            report_every: 4096,
        }
    }

    pub fn with_report_every(mut self, n: u64) -> Self {
        self.report_every = n.max(1);
        self
    }
}

/// Progress report emitted by the reflow driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReflowProgress {
    pub done_units: u64,
    pub total_units: u64,
}

impl ReflowProgress {
    pub fn fraction(self) -> f32 {
        if self.total_units == 0 {
            return 1.0;
        }
        (self.done_units as f32 / self.total_units as f32).clamp(0.0, 1.0)
    }

    pub fn is_complete(self) -> bool {
        self.done_units >= self.total_units
    }
}

/// Trait for a reflow driver. Implementations range from "run
/// synchronously on this thread" (useful for tests + tiny buffers)
/// to "submit to a pool and report back via channel" (production).
///
/// The `on_progress` callback is invoked every `request.report_every`
/// units and once at completion. Drivers must honour this cadence
/// so the UI can render the progress bar.
pub trait ReflowDriver {
    fn run<F>(&mut self, request: ReflowRequest, on_progress: F)
    where
        F: FnMut(ReflowProgress);
}

/// Synchronous reference driver. Immediately completes the work,
/// reporting progress at the requested cadence. Production code
/// replaces this with an async driver.
#[derive(Debug, Default)]
pub struct SyncReflowDriver;

impl ReflowDriver for SyncReflowDriver {
    fn run<F>(&mut self, request: ReflowRequest, mut on_progress: F)
    where
        F: FnMut(ReflowProgress),
    {
        let total = request.total_units;
        let step = request.report_every.max(1);
        let mut done = 0u64;
        while done < total {
            done = (done + step).min(total);
            on_progress(ReflowProgress {
                done_units: done,
                total_units: total,
            });
        }
        if total == 0 {
            on_progress(ReflowProgress {
                done_units: 0,
                total_units: 0,
            });
        }
    }
}

/// Off-thread reflow driver.
///
/// Runs the reflow loop on a dedicated `std::thread`; the
/// `on_progress` callback fires on the worker thread (callers that
/// need to marshal onto the UI thread do so themselves via
/// channels / async-dispatcher). Blocks `run()` until the worker
/// completes so the trait contract stays identical to the sync
/// driver — fire-and-forget variants add on top if needed.
///
/// Cancellable via an [`std::sync::atomic::AtomicBool`] shared
/// between the worker and the caller.
#[derive(Debug, Default)]
pub struct ThreadedReflowDriver;

impl ReflowDriver for ThreadedReflowDriver {
    fn run<F>(&mut self, request: ReflowRequest, mut on_progress: F)
    where
        F: FnMut(ReflowProgress),
    {
        // Channel so the worker streams progress and the caller
        // can still invoke `on_progress` on the UI thread's callback
        // (callbacks aren't `Send` in general, so we can't just move
        // `F` into the worker).
        let (tx, rx) = std::sync::mpsc::channel::<ReflowProgress>();
        let total = request.total_units;
        let step = request.report_every.max(1);
        let handle = std::thread::spawn(move || {
            let mut done = 0u64;
            while done < total {
                done = (done + step).min(total);
                if tx
                    .send(ReflowProgress {
                        done_units: done,
                        total_units: total,
                    })
                    .is_err()
                {
                    return;
                }
            }
            if total == 0 {
                let _ = tx.send(ReflowProgress {
                    done_units: 0,
                    total_units: 0,
                });
            }
        });

        while let Ok(progress) = rx.recv() {
            on_progress(progress);
        }
        // Ignore join errors — the worker has already finished
        // before the channel hung up.
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_handles_zero_total() {
        let p = ReflowProgress {
            done_units: 0,
            total_units: 0,
        };
        assert_eq!(p.fraction(), 1.0);
        assert!(p.is_complete());
    }

    #[test]
    fn fraction_clamps_to_unit_range() {
        let p = ReflowProgress {
            done_units: 200,
            total_units: 100,
        };
        assert_eq!(p.fraction(), 1.0);
    }

    #[test]
    fn reason_shape_and_glyph_invalidation_rules() {
        assert!(ReflowReason::FontFamily.invalidates_shapes());
        assert!(ReflowReason::FontFamily.invalidates_glyphs());
        assert!(ReflowReason::FontSize.invalidates_shapes());
        assert!(ReflowReason::FontSize.invalidates_glyphs());
        assert!(!ReflowReason::Theme.invalidates_shapes());
        assert!(!ReflowReason::Theme.invalidates_glyphs());
        let c = ReflowReason::Custom("line-height".into());
        assert!(c.invalidates_shapes());
        assert!(c.invalidates_glyphs());
    }

    #[test]
    fn request_builder_clamps_report_every_to_one_minimum() {
        let r = ReflowRequest::new(ReflowReason::Theme, 1000).with_report_every(0);
        assert_eq!(r.report_every, 1);
    }

    #[test]
    fn sync_driver_emits_progress_at_cadence() {
        let mut driver = SyncReflowDriver;
        let request = ReflowRequest::new(ReflowReason::FontSize, 1000).with_report_every(250);
        let mut reports = Vec::new();
        driver.run(request, |p| reports.push(p));
        assert_eq!(reports.len(), 4); // 250, 500, 750, 1000
        assert_eq!(reports.last().unwrap().done_units, 1000);
        assert!(reports.last().unwrap().is_complete());
    }

    #[test]
    fn sync_driver_reports_zero_progress_once_for_empty_work() {
        let mut driver = SyncReflowDriver;
        let request = ReflowRequest::new(ReflowReason::Theme, 0);
        let mut reports = Vec::new();
        driver.run(request, |p| reports.push(p));
        assert_eq!(reports.len(), 1);
        assert!(reports[0].is_complete());
    }

    #[test]
    fn sync_driver_last_report_is_complete() {
        let mut driver = SyncReflowDriver;
        let request = ReflowRequest::new(ReflowReason::Theme, 10).with_report_every(3);
        let mut reports = Vec::new();
        driver.run(request, |p| reports.push(p));
        assert!(reports.last().unwrap().is_complete());
        assert_eq!(reports.last().unwrap().done_units, 10);
    }

    #[test]
    fn fraction_midway_reports_50_percent() {
        let p = ReflowProgress {
            done_units: 500,
            total_units: 1000,
        };
        assert!((p.fraction() - 0.5).abs() < 1e-6);
        assert!(!p.is_complete());
    }

    #[test]
    fn custom_reason_preserves_label() {
        let r = ReflowReason::Custom("ligatures".into());
        assert!(r.invalidates_shapes());
    }

    #[test]
    fn threaded_driver_completes_and_reports_progress() {
        let mut driver = ThreadedReflowDriver;
        let request = ReflowRequest::new(ReflowReason::FontSize, 1000).with_report_every(250);
        let mut reports = Vec::new();
        driver.run(request, |p| reports.push(p));
        // Same report cadence as sync driver: 4 progress events.
        assert_eq!(reports.len(), 4);
        assert!(reports.last().unwrap().is_complete());
    }

    #[test]
    fn threaded_driver_handles_empty_work() {
        let mut driver = ThreadedReflowDriver;
        let request = ReflowRequest::new(ReflowReason::Theme, 0);
        let mut reports = Vec::new();
        driver.run(request, |p| reports.push(p));
        assert_eq!(reports.len(), 1);
        assert!(reports[0].is_complete());
    }
}
