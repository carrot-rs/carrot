//! Motion-reduction settings.
//!
//! Cursor blink, AI ghost-text fade-in, completion-dropdown
//! animations must respect the platform `prefers-reduced-motion`
//! setting.
//!
//! This module owns the **settings** + **query** side of that
//! contract. Platform detection (reading the OS preference) lives
//! in `inazuma::platform` when the concrete adapters land; this
//! crate just consumes an enum.
//!
//! # Usage
//!
//! - Render code checks [`MotionPreference::allow_animation`]
//!   before playing the fade / blink / slide.
//! - The default is [`MotionPreference::Full`] until the platform
//!   adapter reports otherwise.
//! - The user can override via settings (`config.motion = "reduced"`).
//!
//! # What is NOT here
//!
//! - Not a platform detector. Reading NSWorkspace.accessibility-
//!   DisplayShouldReduceMotion / Windows SPI_GETCLIENTAREAANIMATION
//!   / GNOME gtk-enable-animations belongs behind inazuma.
//! - Not a renderer — consumers check the flag and branch.

/// User / platform preference for animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MotionPreference {
    /// All animations enabled (the default).
    #[default]
    Full,
    /// Animations disabled — cursor is solid, fades become cuts,
    /// dropdowns appear without slide-in.
    Reduced,
}

impl MotionPreference {
    /// Whether a given [`AnimationKind`] should play under this
    /// preference. Reduced motion blocks every animated kind; Full
    /// allows all.
    pub fn allow_animation(self, kind: AnimationKind) -> bool {
        match self {
            MotionPreference::Full => true,
            MotionPreference::Reduced => !kind.is_animation(),
        }
    }

    /// True when the platform asked for reduced motion.
    pub fn is_reduced(self) -> bool {
        matches!(self, MotionPreference::Reduced)
    }
}

/// Category of animation the render layer wants to play. The
/// categorisation exists so that future overrides (e.g. "I want the
/// cursor to blink but not the fade-ins") can toggle per-kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimationKind {
    /// Cursor blink (on/off cycle).
    CursorBlink,
    /// AI ghost-text fade-in.
    GhostTextFadeIn,
    /// Completion dropdown slide-in.
    CompletionDropdownSlide,
    /// Hover highlight crossfade (button / tab / list item).
    HoverCrossfade,
    /// Selection marquee march.
    SelectionMarquee,
    /// Scrolling inertia / momentum.
    ScrollInertia,
    /// Non-animation paint (regular draw). `Full` and `Reduced`
    /// always allow this — used by callers that want a single
    /// `allow_animation` check without branching on the preference
    /// directly.
    StaticPaint,
}

impl AnimationKind {
    pub fn is_animation(self) -> bool {
        !matches!(self, AnimationKind::StaticPaint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_allows_everything() {
        for kind in [
            AnimationKind::CursorBlink,
            AnimationKind::GhostTextFadeIn,
            AnimationKind::CompletionDropdownSlide,
            AnimationKind::HoverCrossfade,
            AnimationKind::SelectionMarquee,
            AnimationKind::ScrollInertia,
            AnimationKind::StaticPaint,
        ] {
            assert!(MotionPreference::Full.allow_animation(kind));
        }
    }

    #[test]
    fn reduced_blocks_animations_but_not_static() {
        assert!(!MotionPreference::Reduced.allow_animation(AnimationKind::CursorBlink));
        assert!(!MotionPreference::Reduced.allow_animation(AnimationKind::GhostTextFadeIn));
        assert!(!MotionPreference::Reduced.allow_animation(AnimationKind::CompletionDropdownSlide));
        assert!(!MotionPreference::Reduced.allow_animation(AnimationKind::HoverCrossfade));
        assert!(!MotionPreference::Reduced.allow_animation(AnimationKind::SelectionMarquee));
        assert!(!MotionPreference::Reduced.allow_animation(AnimationKind::ScrollInertia));
        // Static paint is always allowed.
        assert!(MotionPreference::Reduced.allow_animation(AnimationKind::StaticPaint));
    }

    #[test]
    fn default_is_full() {
        assert_eq!(MotionPreference::default(), MotionPreference::Full);
    }

    #[test]
    fn is_reduced_flag() {
        assert!(MotionPreference::Reduced.is_reduced());
        assert!(!MotionPreference::Full.is_reduced());
    }

    #[test]
    fn animation_kind_partitions_correctly() {
        for kind in [
            AnimationKind::CursorBlink,
            AnimationKind::GhostTextFadeIn,
            AnimationKind::CompletionDropdownSlide,
            AnimationKind::HoverCrossfade,
            AnimationKind::SelectionMarquee,
            AnimationKind::ScrollInertia,
        ] {
            assert!(kind.is_animation(), "{kind:?} should be animation");
        }
        assert!(!AnimationKind::StaticPaint.is_animation());
    }
}
