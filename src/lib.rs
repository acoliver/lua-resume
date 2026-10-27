//! Save and resume suspended Lua 5.4 coroutines from source.
//!
//! See the crate README for the design: a load-time source rewrite makes every point a
//! coroutine can stop at visible in the source, saves read frames through Lua's debug library,
//! and restores generate continuation functions from the game's own source.
