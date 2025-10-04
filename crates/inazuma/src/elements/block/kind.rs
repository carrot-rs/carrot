/// Kind of block — what sort of activity produced it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// A shell command + its output.
    #[default]
    Shell,
    /// A TUI application owning the viewport (vim, htop, claude, …).
    Tui,
}
