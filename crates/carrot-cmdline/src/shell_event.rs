//! Composed shell-integration event.
//!
//! Carrot's shell hooks emit a pair of sequences at each block
//! lifecycle marker: the OSC 133 lifecycle code *plus* an optional
//! OSC 7777 sidecar with typed metadata. Consumers want to see
//! both together — [`ShellBlockEvent`] is that composed event.
//!
//! # Typical stream
//!
//! ```text
//! (PromptStart, Some(BlockMetadata { cwd, user, git, … }))
//! (InputStart,  None)                         ; no metadata change
//! (CommandStart, None)
//! … command runs …
//! (CommandEnd, Some(BlockMetadata { exit: 0, runtime_ms: 247 }))
//! ```
//!
//! The [`ShellStream`] helper buffers partial events (OSC 133
//! arrived but OSC 7777 hasn't yet) and yields a composed event
//! once both are available *or* once the next lifecycle arrives
//! (whichever comes first).

use crate::osc133::ShellEvent;
use crate::osc7777::BlockMetadata;

/// Lifecycle event + optional metadata sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellBlockEvent {
    pub lifecycle: ShellEvent,
    pub metadata: Option<BlockMetadata>,
}

impl ShellBlockEvent {
    pub fn new(lifecycle: ShellEvent) -> Self {
        Self {
            lifecycle,
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: BlockMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    pub fn exit_code(&self) -> Option<i32> {
        crate::osc133::exit_code(&self.lifecycle)
            .or_else(|| self.metadata.as_ref().and_then(|m| m.exit))
    }

    pub fn runtime_ms(&self) -> Option<u64> {
        self.metadata.as_ref().and_then(|m| m.runtime_ms)
    }

    pub fn cwd(&self) -> Option<&str> {
        self.metadata.as_ref().and_then(|m| m.cwd.as_deref())
    }

    pub fn git_branch(&self) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(|m| m.git.as_ref())
            .and_then(|g| g.branch.as_deref())
    }
}

/// Buffer that composes OSC 133 + OSC 7777 into [`ShellBlockEvent`]s.
///
/// Shell hooks may emit the pair in either order: usually `OSC 133;
/// A` first then `OSC 7777 ; …`, but sometimes the reverse. The
/// stream accepts them individually and yields a composed event
/// when:
///
/// 1. A lifecycle + its metadata have both arrived, or
/// 2. A new lifecycle arrives and the previous one is flushed
///    alone (no metadata for that marker).
///
/// Call `flush()` at end-of-stream to drain any buffered event.
#[derive(Debug, Default, Clone)]
pub struct ShellStream {
    pending_lifecycle: Option<ShellEvent>,
    pending_metadata: Option<BlockMetadata>,
}

impl ShellStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest a lifecycle marker. Returns `Some(event)` when the
    /// previous pair is now complete (or flushed alone because of
    /// timeout / new marker).
    pub fn push_lifecycle(&mut self, ev: ShellEvent) -> Option<ShellBlockEvent> {
        let flushed = self.take_pair();
        self.pending_lifecycle = Some(ev);
        flushed
    }

    /// Ingest metadata. If we already have a buffered lifecycle,
    /// the pair is emitted. Otherwise we buffer the metadata to
    /// attach to the next lifecycle.
    pub fn push_metadata(&mut self, md: BlockMetadata) -> Option<ShellBlockEvent> {
        if let Some(lifecycle) = self.pending_lifecycle.take() {
            return Some(
                ShellBlockEvent::new(lifecycle)
                    .with_metadata(self.pending_metadata.take().unwrap_or_default())
                    .with_metadata(md),
            );
        }
        // Buffer for next lifecycle.
        self.pending_metadata = Some(md);
        None
    }

    /// Drain any buffered lifecycle (with or without metadata).
    pub fn flush(&mut self) -> Option<ShellBlockEvent> {
        self.take_pair()
    }

    fn take_pair(&mut self) -> Option<ShellBlockEvent> {
        let lifecycle = self.pending_lifecycle.take()?;
        let metadata = self.pending_metadata.take();
        Some(match metadata {
            Some(md) => ShellBlockEvent::new(lifecycle).with_metadata(md),
            None => ShellBlockEvent::new(lifecycle),
        })
    }

    /// Quick inspector: is there any pending data buffered?
    pub fn is_pending(&self) -> bool {
        self.pending_lifecycle.is_some() || self.pending_metadata.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc7777::GitInfo;

    fn meta_with_cwd(cwd: &str) -> BlockMetadata {
        BlockMetadata {
            cwd: Some(cwd.into()),
            ..Default::default()
        }
    }

    #[test]
    fn new_event_starts_without_metadata() {
        let e = ShellBlockEvent::new(ShellEvent::PromptStart);
        assert!(e.metadata.is_none());
        assert_eq!(e.cwd(), None);
    }

    #[test]
    fn with_metadata_attaches_cwd() {
        let e = ShellBlockEvent::new(ShellEvent::PromptStart).with_metadata(meta_with_cwd("/tmp"));
        assert_eq!(e.cwd(), Some("/tmp"));
    }

    #[test]
    fn exit_code_prefers_lifecycle_then_metadata() {
        let lifecycle_only = ShellBlockEvent::new(ShellEvent::CommandEnd { exit_code: 1 });
        assert_eq!(lifecycle_only.exit_code(), Some(1));

        // Lifecycle exit=0 → metadata overrides with its own value.
        let metadata_only =
            ShellBlockEvent::new(ShellEvent::PromptStart).with_metadata(BlockMetadata {
                exit: Some(7),
                ..Default::default()
            });
        assert_eq!(metadata_only.exit_code(), Some(7));
    }

    #[test]
    fn git_branch_helper_reads_through_metadata() {
        let md = BlockMetadata {
            git: Some(GitInfo {
                branch: Some("main".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let e = ShellBlockEvent::new(ShellEvent::PromptStart).with_metadata(md);
        assert_eq!(e.git_branch(), Some("main"));
    }

    #[test]
    fn stream_composes_lifecycle_then_metadata() {
        let mut s = ShellStream::new();
        assert!(s.push_lifecycle(ShellEvent::PromptStart).is_none());
        let ev = s.push_metadata(meta_with_cwd("/home")).unwrap();
        assert_eq!(ev.lifecycle, ShellEvent::PromptStart);
        assert_eq!(ev.cwd(), Some("/home"));
    }

    #[test]
    fn stream_composes_metadata_then_lifecycle() {
        let mut s = ShellStream::new();
        assert!(s.push_metadata(meta_with_cwd("/tmp")).is_none());
        // A new lifecycle flushes the previous (none) and buffers
        // the new one; we need one more event to force a yield.
        assert!(s.push_lifecycle(ShellEvent::PromptStart).is_none());
        let ev = s.flush().unwrap();
        assert_eq!(ev.lifecycle, ShellEvent::PromptStart);
        assert_eq!(ev.cwd(), Some("/tmp"));
    }

    #[test]
    fn stream_flushes_lifecycle_without_metadata_on_new_event() {
        let mut s = ShellStream::new();
        assert!(s.push_lifecycle(ShellEvent::PromptStart).is_none());
        // InputStart arrives before any metadata — PromptStart must
        // be flushed alone.
        let flushed = s.push_lifecycle(ShellEvent::InputStart).unwrap();
        assert_eq!(flushed.lifecycle, ShellEvent::PromptStart);
        assert!(flushed.metadata.is_none());
    }

    #[test]
    fn stream_flush_drains_pending_lifecycle() {
        let mut s = ShellStream::new();
        s.push_lifecycle(ShellEvent::CommandEnd { exit_code: 0 });
        let ev = s.flush().unwrap();
        assert!(matches!(ev.lifecycle, ShellEvent::CommandEnd { .. }));
    }

    #[test]
    fn stream_flush_returns_none_when_empty() {
        let mut s = ShellStream::new();
        assert!(s.flush().is_none());
    }

    #[test]
    fn stream_is_pending_reports_buffer_state() {
        let mut s = ShellStream::new();
        assert!(!s.is_pending());
        s.push_lifecycle(ShellEvent::PromptStart);
        assert!(s.is_pending());
        s.push_metadata(meta_with_cwd("/x"));
        assert!(!s.is_pending());
    }

    #[test]
    fn runtime_ms_only_populated_from_metadata() {
        let ev = ShellBlockEvent::new(ShellEvent::CommandEnd { exit_code: 0 }).with_metadata(
            BlockMetadata {
                runtime_ms: Some(500),
                ..Default::default()
            },
        );
        assert_eq!(ev.runtime_ms(), Some(500));
        let bare = ShellBlockEvent::new(ShellEvent::CommandEnd { exit_code: 0 });
        assert_eq!(bare.runtime_ms(), None);
    }
}
