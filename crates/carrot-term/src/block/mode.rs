//! VT-mode bitflags mirror for block.
//!
//! Re-exports [`crate::term::mode::TermMode`] into the block namespace
//! so consumers can address modes without importing the legacy module
//! tree. Once the legacy `term/` is deleted, the underlying definition
//! moves here one-to-one; the re-export stays so call sites don't churn.

pub use crate::term::mode::TermMode;
