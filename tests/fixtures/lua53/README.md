Fixtures for the Lua 5.3 import tests. All source is synthetic.

- `sample.lua`: a function with `assert(...)` and `dbg(...)` statements, as the Stranded sources have.
- `sample.stripped.lua`: the same file with those statements blanked and line numbers kept. It was written
  by `strip_assert.py` from the save-spike probe (a text heuristic, used here only to produce the
  expected output of the exact AST pass in `src/strip.rs`).
- `sample.luac`: `sample.stripped.lua` compiled with stock Lua 5.3.6, chunk name `@Lua/sample.lua`,
  debug information kept.

Regeneration: build a minimal `luac` against the Lua 5.3.6 sources vendored in the `lua-src` crate
(`lua-src-547.0.0/lua-5.3.6`, every `.c` file except `lua.c` and `luac.c`) with a 25-line driver that calls
`luaL_loadbufferx` and `lua_dump(L, writer, file, 0)`, then run

    luac53 @Lua/sample.lua sample.stripped.lua sample.luac
