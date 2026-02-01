//! GitHub CLI integration for the vertical tabs panel.
//!
//! The PR-badge feature needs `gh` on the user's PATH and an
//! authenticated account. This module groups everything that deals
//! with detecting those two conditions and nudging the user when
//! they aren't met:
//!
//! - [`state`] — async detection state machine + PR-fetch cache +
//!   modal deployment logic, all as `impl VerticalTabsPanel` methods.
//! - [`install_modal`] — modal that proposes installing `gh` via the
//!   user's package manager (writes the install command to their
//!   active terminal so prompts stay interactive).
//! - [`auth_modal`] — modal that proposes running `gh auth login` in
//!   the active terminal once `gh` is present but unauthenticated.

pub mod auth_modal;
pub mod install_modal;
pub mod state;
