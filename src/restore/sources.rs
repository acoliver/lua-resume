//! The trusted source a restore generates code from.

use std::collections::HashMap;

use crate::syntax::ast::Block;

/// Normalized chunks by chunk name, as `debug.getinfo` reports it in `source`
/// (`=name` for a chunk loaded with that name).
#[derive(Clone, Debug, Default)]
pub struct Sources {
    chunks: HashMap<String, Block>,
}

impl Sources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the normalized tree that was loaded under the name `chunk`.
    pub fn insert(&mut self, chunk: &str, normalized: Block) {
        self.chunks.insert(chunk.to_owned(), normalized);
    }

    pub fn get(&self, chunk: &str) -> Option<&Block> {
        self.chunks.get(chunk)
    }
}
