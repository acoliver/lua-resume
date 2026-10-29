//! The trusted source a restore generates code from.

use std::collections::HashMap;

use crate::syntax::OperatorHooks;
use crate::syntax::ast::Block;

/// Normalized chunks by chunk name, as `debug.getinfo` reports it in `source`
/// (`=name` for a chunk loaded with that name).
#[derive(Clone, Debug, Default)]
pub struct Sources {
    chunks: HashMap<String, Block>,
    hooks: OperatorHooks,
}

impl Sources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the normalized tree that was loaded under the name `chunk`.
    pub fn insert(&mut self, chunk: &str, normalized: Block) {
        self.chunks.insert(chunk.to_owned(), normalized);
    }

    /// Sets the operator hooks the chunks were loaded with, so the functions a restore
    /// generates call the same host functions in place of the same operators.
    #[must_use]
    pub fn with_hooks(mut self, hooks: OperatorHooks) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn hooks(&self) -> &OperatorHooks {
        &self.hooks
    }

    pub fn get(&self, chunk: &str) -> Option<&Block> {
        self.chunks.get(chunk)
    }
}
