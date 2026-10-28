//! Setup shared by the rewrite tests: compiling a chunk with stock Lua and normalizing it.

use lua_resume::compiled::Compiled;
use lua_resume::rewrite::{RewriteError, normalize};
use lua_resume::syntax::ast::Block;
use lua_resume::syntax::parse;
use mlua::{ChunkMode, Lua};

/// Normalizes `source`, compiled by stock Lua 5.4 for the decisions only Lua can make.
pub fn normalize_source(source: &[u8]) -> Result<Block, RewriteError> {
    let tree = parse(source).expect("the source parses");
    let dump = Lua::new()
        .load(source)
        .set_mode(ChunkMode::Text)
        .into_function()
        .expect("the source compiles")
        .dump(true);
    normalize(&tree, &Compiled::from_dump(&dump).expect("the dump reads"))
}
