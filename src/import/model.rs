//! The data a host fills in from its decoder of a saved Lua 5.3 thread.

use std::collections::HashMap;

use crate::capture::{Cell, TableData, ThreadData, Value};
use crate::lua53::Proto;
use crate::lua53::sites::{FunctionIndex, SiteError};
use crate::restore::Sources;
use crate::syntax::ast::Block;

/// Lua 5.3 `CallInfo` status bits that the import reads.
pub mod status {
    pub const LUA: u8 = 1 << 1;
    pub const YPCALL: u8 = 1 << 4;
    pub const TAIL: u8 = 1 << 5;
    /// A frame stopped by a debug hook. Hooks are not restorable.
    pub const HOOKED: u8 = 1 << 2;
}

/// One `CallInfo` of a saved thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedFrame {
    /// Stack index of the running function.
    pub func: usize,
    /// Stack index of register 0 (Lua frames).
    pub base: usize,
    /// The saved program counter: the index of the instruction after the call.
    pub saved_pc: usize,
    pub call_status: u8,
    /// The label of the continuation function (`k`) of a native frame, if it has one.
    pub continuation: Option<String>,
}

/// A saved coroutine: its stack, its frames from the outermost, and its open upvalues. The
/// base `CallInfo` of the thread is not a frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedThread {
    /// Stack slots from index 0. Values refer to the arenas of [`SavedHeap`].
    pub stack: Vec<Value>,
    pub frames: Vec<SavedFrame>,
    /// For each open upvalue, the stack index it points at and the cell it shares. The cell
    /// takes the value of that stack slot.
    pub open_upvalues: Vec<(usize, usize)>,
}

/// A saved closure: an index into [`SavedHeap::protos`] and one cell per upvalue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedClosure {
    pub proto: usize,
    pub upvalues: Vec<usize>,
}

/// The objects the stacks reach, already in the snapshot's value model. `Value::Closure(i)`
/// is `closures[i]` and `Value::Table(i)` is `tables[i]`. The imported threads come first in
/// the snapshot, so with `n` of them `Value::Thread(i)` is `threads[i - n]` for `i >= n`.
#[derive(Clone, Debug, Default)]
pub struct SavedHeap {
    pub tables: Vec<TableData>,
    pub cells: Vec<Cell>,
    pub closures: Vec<SavedClosure>,
    pub protos: Vec<Proto<(), ()>>,
    pub threads: Vec<ThreadData>,
    /// Named data the host wants restored with the threads, such as the save's root table.
    pub roots: Vec<(String, Value)>,
}

/// A chunk of the game: how the 5.4 state names it, and its source as the 5.3 build saw it
/// and as the 5.4 state loads it.
#[derive(Clone, Debug)]
pub struct ImportChunk {
    pub(super) name: String,
    pub(super) index: FunctionIndex,
    pub(super) normalized: Block,
}

impl ImportChunk {
    /// `name` is the chunk name of the normalized tree in the 5.4 state (`=file.lua`).
    /// `stripped` is the original tree after `strip_debug_calls`, which the saved prototypes
    /// were compiled from, and `normalized` is the tree that was rewritten from it.
    pub fn new(name: &str, stripped: &Block, normalized: Block) -> Result<Self, SiteError> {
        Ok(Self {
            name: name.to_owned(),
            index: FunctionIndex::with_normalized(stripped, &normalized)?,
            normalized,
        })
    }
}

/// The chunks of the game by the source name saved prototypes carry (`@Lua/file.lua`).
#[derive(Clone, Debug, Default)]
pub struct ImportSources {
    pub(super) chunks: HashMap<Vec<u8>, ImportChunk>,
}

impl ImportSources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, saved_source: &[u8], chunk: ImportChunk) {
        self.chunks.insert(saved_source.to_vec(), chunk);
    }

    /// The trees to hand to `restore` for the imported snapshot.
    pub fn restore_sources(&self) -> Sources {
        let mut sources = Sources::new();
        for chunk in self.chunks.values() {
            sources.insert(&chunk.name, chunk.normalized.clone());
        }
        sources
    }
}

/// Renames of permanents from the labels in the save to the names the 5.4 host registered.
#[derive(Clone, Debug)]
pub struct PermanentMap {
    renames: HashMap<String, String>,
}

/// The label Lua 5.3's `ipairs` iterator has in a persist save.
pub const IPAIRS_AUX: &str = "__ipairs_aux__";
/// The name the 5.4 host registers `ipairs`'s iterator under.
pub const IPAIRS_ITERATOR: &str = "ipairs iterator";

impl Default for PermanentMap {
    fn default() -> Self {
        let mut map = Self {
            renames: HashMap::new(),
        };
        map.rename(IPAIRS_AUX, IPAIRS_ITERATOR);
        map
    }
}

impl PermanentMap {
    pub fn rename(&mut self, saved: &str, host: &str) {
        self.renames.insert(saved.to_owned(), host.to_owned());
    }

    pub(super) fn host_name(&self, saved: &str) -> String {
        self.renames
            .get(saved)
            .map_or_else(|| saved.to_owned(), Clone::clone)
    }
}
