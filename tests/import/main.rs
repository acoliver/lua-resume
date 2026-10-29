//! Old saves restore: Lua 5.3 stacks are imported, restored in a fresh Lua 5.4 state and
//! resumed, and what they do from there equals the rest of the uninterrupted run.

mod frames;
mod heap;
mod support;
mod upvalues;
