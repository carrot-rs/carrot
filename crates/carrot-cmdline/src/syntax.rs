//! Shell-specific grammar bindings.
//!
//! Each shell has its own parser that produces a typed
//! [`crate::ast::CommandAst`]:
//!
//! - [`bash`]: tree-sitter-bash grammar.
//! - [`zsh`]:  tree-sitter-zsh grammar.
//! - [`fish`]: tree-sitter-fish grammar.
//! - [`nu`]:   tree-sitter-nu grammar.
//!
//! The fallback whitespace parser lives in [`crate::parse`] and is
//! used as a backstop when a grammar reports parse errors on
//! mid-typing partial input.

pub mod bash;
pub mod fish;
pub mod nu;
pub mod zsh;
