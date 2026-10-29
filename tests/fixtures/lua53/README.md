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

`cases.lua` holds one small function per frame shape the import tests rebuild (nested call, numeric `for`,
`ipairs` loop, vararg function, tail call, `pcall`, open upvalue, temporaries, an open upvalue read after
the closure writes it, and a generic `for` stopped in a Lua iterator). It has no `assert` or `dbg`
statements. `cases.luac` is its stock Lua 5.3.6 compile with chunk name `@Lua/cases.lua`, made with the same
`luac53` driver:

    luac53 @Lua/cases.lua cases.lua cases.luac

The last function of `cases.lua`, `stash`, uses two chunk-level locals. Lua 5.3 numbers its upvalues
`seed, _ENV, count`, and the normalized source numbers them `_ENV, seed, count`. The import tests use it to
check that a restore binds upvalues by name. It was appended, so earlier line numbers are unchanged.

`pick` and `guarded` were appended after that. `guarded` calls `pcall(leaf, pick(total))` in a loop, so the
protected call has a call in its last argument. `cases.luac` was recompiled with the same driver; the
earlier part of the file compiles to the same bytes as before.
