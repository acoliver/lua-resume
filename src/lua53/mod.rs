//! Lua 5.3 prototypes from old Stranded saves, and the mapping of their suspended frames
//! onto call sites of the Lua 5.4 source.
//!
//! - [`chunk`] reads standard `luac` 5.3 chunks and the `MapData` variant of the header.
//! - [`persist`] decodes the prototype layout inside a persist save, handing constants and
//!   nested prototypes to the host's own value decoder.
//! - [`op`] identifies the call instructions and their registers.
//! - [`frame`] turns a program counter and register values into a call ordinal, named locals
//!   and varargs.
//! - [`sites`] lists the calls of each source function in evaluation order, so an ordinal from
//!   a saved frame finds its call node.

pub mod chunk;
pub mod frame;
pub mod op;
pub mod persist;
mod proto;
mod read;
pub mod sites;

pub use proto::{Constant, LocalVar, Proto, Upvalue};
pub use read::{Cursor, ReadError};
