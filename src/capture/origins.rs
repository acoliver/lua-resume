//! Continuation functions that a restore built, mapped back to the frames they stand for.
//!
//! A restored coroutine runs generated continuation functions, not the game's own functions.
//! The restore records, in the Lua state, which saved frame each continuation continues, so a
//! later capture reports the original function, closure, line and tail-call flag.

use std::collections::HashMap;

use mlua::{Function, Lua};

/// The saved frame a continuation function continues.
#[derive(Clone)]
pub(crate) struct Origin {
    /// The restored closure of the frame; its upvalue cells are the frame's.
    pub(crate) closure: Function,
    pub(crate) line: i64,
    pub(crate) tail_call: bool,
    /// Holds the continuation so its address is not reused while the origin is registered.
    continuation: Function,
}

#[derive(Default)]
struct Registry(HashMap<usize, Origin>);

impl Origin {
    pub(crate) fn new(
        closure: Function,
        (line, tail_call): (i64, bool),
        continuation: Function,
    ) -> Self {
        Self {
            closure,
            line,
            tail_call,
            continuation,
        }
    }
}

/// Registers `origin` in `lua` for the continuation it holds.
pub(crate) fn register(lua: &Lua, origin: Origin) {
    let key = origin.continuation.to_pointer().addr();
    if lua.app_data_mut::<Registry>().is_none() {
        lua.set_app_data(Registry::default());
    }
    if let Some(mut registry) = lua.app_data_mut::<Registry>() {
        registry.0.insert(key, origin);
    }
}

/// The origins registered in `lua`, by the address of their continuation.
pub(super) fn all(lua: &Lua) -> HashMap<usize, Origin> {
    lua.app_data_ref::<Registry>()
        .map(|registry| registry.0.clone())
        .unwrap_or_default()
}
