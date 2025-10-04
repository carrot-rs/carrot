/// Abstract lifecycle event for a block.
///
/// The primitive is wire-format-agnostic. A consumer (e.g. `carrot-term`)
/// translates OSC 133 or other shell-integration signals into these events
/// and dispatches them to `BlockState::on_lifecycle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockLifecycle {
    /// The prompt has started rendering.
    PromptStart,
    /// The user's input region has started (prompt is done).
    InputStart,
    /// A command is now executing and its output stream begins.
    CommandStart,
    /// The command finished. Carries the exit code.
    CommandEnd {
        /// Process exit code.
        exit_code: i32,
    },
}
