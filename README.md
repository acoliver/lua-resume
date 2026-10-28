# lua-resume

lua-resume saves suspended Lua 5.4 coroutines to plain data and resumes them later, in a new process, at the statement where they stopped. It runs on stock Lua 5.4. It reads no VM internals, and a save file holds no bytecode.

It is written for games that run their original Lua scripts and save running script threads mid-function, as the Pluto and Eris libraries did for Lua 5.1 to 5.3. Those libraries copy the VM's internal stack and bytecode, so each Lua version needs a new port, and a save file is only safe to load if it is trusted.

## How it works

Loading, saving and restoring are separate steps.

**Loading.** Before a chunk is compiled, lua-resume rewrites its source into an equivalent form in which every point a coroutine can stop at is visible in the source:

- Each call inside an expression is moved into its own statement, and its result goes into a new local variable. A thread that yields inside `local x = a + Sleep(10)` is then stopped in a statement that only assigns a local, and the values computed before the call are held in named locals.
- `and` and `or` keep their short-circuit order, and `while`, `repeat` and numeric and generic `for` loops keep their state in named locals.
- Each call statement records a site number in a local before it runs, so a saved frame says which call it is waiting in.

The rewritten chunk behaves the same as the original: it computes the same values in the same order and calls the same functions.

**Saving.** A suspended coroutine is read through Lua's debug library: for each frame, its function, site number, locals and varargs. Functions are identified by chunk name and position in the source, tables are copied as data with shared references and cycles kept, and closures are saved as their function plus their upvalues, with shared upvalues kept shared. Engine functions are saved by name.

**Restoring.** For each saved function and site, lua-resume generates, from the trusted game source, a version of that function that assigns the saved locals and continues at the saved site. Restoring a coroutine creates a new coroutine that calls these functions from the outermost frame to the innermost, then waits in the innermost call as the original did.

A save file contains only data and names. Code always comes from the game's own source, so loading a save cannot run code the game did not ship.

## Status

Under construction; [#1](https://github.com/acoliver/lua-resume/issues/1) tracks the plan.

- **Parser and printer: done.** `syntax::parse` reads Lua 5.4 source into a tree, and `syntax::print` writes a tree back as source that compiles to the same code. Each statement and function keeps its original line, so error messages and `debug.getinfo` report the original lines. The tests compile the original and the printed source with Lua 5.4 and compare the bytecode. Over all 3,387 Lua files that ship with Stranded: Alien Dawn's Mod Tools (927,000 lines), the bytecode is identical for every file. That corpus is not included here; run it with `LUA_RESUME_CORPUS=<dir> cargo test --release -- --ignored`.
- **Normalization: done.** `rewrite::normalize(&Block, &Compiled)` stages explicit calls and their operands, preserves short-circuiting, rewrites loop conditions and generic iterators, and adds function-local call-site markers. Differential tests compare values, call traces, errors and coroutine yields on stock Lua 5.4. The original compiler's table-read decisions are required input, as described below.
- **Continuation generation: done for the shapes in old Stranded saves, except backward gotos.** `continuation::continuation_source` generates, from a normalized chunk, the function that finishes a frame from one call site (see below).
- **Capture of suspended coroutines and the snapshot model: done.** `capture::capture` reads a coroutine suspended in normalized code into a serializable `Snapshot` (see below). It uses the host's debug library through a `Debug` capability, and the `lua-resume-debug` crate creates a state in which only the host can reach that library.
- **Restore: done for coroutines that capture accepts.** `restore::restore(&Lua, &Debug, &Snapshot, &Sources, &Permanents)` rebuilds a suspended coroutine (see below). The round-trip suite saves five scripts at all 20 of their yields, restores each in a fresh Lua state through JSON, and compares the rest of the run with the uninterrupted run.
- **Engine integration, frames inside `pcall` and `xpcall`, and capture inside numeric `for` bodies: not started.**

## Normalizing a chunk

Compile the original source with stock Lua 5.4 before rewriting it. The host obtains its dump through the public Lua `string.dump` API or mlua's equivalent `Function::dump`, then passes the parsed analysis to the normalizer. The crate has no runtime dependency on mlua; a caller can supply dump bytes from its own Lua host.

```rust
use lua_resume::{compiled::Compiled, rewrite::normalize, syntax::{parse, print}};
use mlua::{ChunkMode, Lua};

fn load(lua: &Lua, source: &str) -> Result<mlua::Function, Box<dyn std::error::Error>> {
    let tree = parse(source.as_bytes())?;
    let dump = lua.load(source)
        .set_mode(ChunkMode::Text)
        .set_name("=game")
        .into_function()?
        .dump(true);
    let compiled = Compiled::from_dump(&dump)?;
    let normalized = normalize(&tree, &compiled)?;
    Ok(lua.load(print(&normalized))
        .set_mode(ChunkMode::Text)
        .set_name("=game")
        .into_function()?)
}
```

Lua sometimes reads an assignment's table before its right-hand side and sometimes keeps it as a late upvalue read. For short string keys, this depends on whether the original constant index fits an 8-bit operand. Normalization changes the constant layout, so source syntax alone cannot determine the original read point.

`Compiled::from_dump` reads each function's table-store instructions. The normalizer traverses the original AST in source compilation order, including constructor stores, implicit globals and reverse-order multiple-assignment stores. It maps each assignment occurrence to its compiler decision and verifies the store count independently in every function. Nested prototypes are matched by definition order and `linedefined`. A count or prototype-line mismatch is an error. This also handles numeric keys and keys folded from `<const>` locals without guessing from their spelling.

The caller must supply a dump of exactly the same original chunk as the AST. Count and line validation detect mapping inconsistencies; they do not authenticate the source. Analysis is discarded after loading. Bytecode never goes in save data.

### Constraints

- The dump reader supports the official Lua 5.4 format with 32-bit instructions, little-endian 64-bit integers and IEEE 64-bit numbers. Stripped and debug dumps are accepted. Custom compiler formats are unsupported.
- Names beginning with `__lr` are reserved for generated locals. Normalization reports insufficient local capacity. Large expressions use private spill storage.
- The pass exposes explicit calls. Implicit metamethod suspension, garbage-collector observations and debug-library observations of generated locals are outside its contract. Operand errors can name a generated temporary.
- Corpus checks establish normal form and compilation. They do not execute the game's scripts or prove runtime equivalence over the corpus.

## Continuations

`continuation_source(chunk, function, site, locals, upvalues)` returns a chunk that evaluates to `function(__lr_v, __lr_next, __lr_unpack, ...)`. It declares `locals` from the table `__lr_v`, re-enters the enclosing blocks and continues after the call of `site`. The pending call is replaced by `__lr_next()`: the next inner frame, or `coroutine.yield` for the innermost frame. Functions are found by the lines their definition spans, and only functions defined by `function`, `local function`, or as the value of a `local` or assignment are found.

Supported:

- Enclosing `do`, `if`, `while`, `repeat` and numeric `for` blocks. Generic `for` is a `while` after normalization, so `ipairs`, `pairs` and `next` loops continue with their saved iterator state. A `repeat` condition sees the body locals.
- Site statements that are a call, a `local` or an assignment with the call anywhere in a chain such as `f(a, g())`, or a `return` (including tail calls `return f(...)` and several results). Field stores such as `t.x = f()` are normalized into a temporary, so their site is a plain assignment.
- Vararg functions: the continuation takes the saved varargs as its `...`.
- Numeric `for`: normalization keeps the limit and step in locals and the loop variable's value at the start of each iteration in a body local. The continuation restarts the loop at that saved value with the same limit and step. Lua derives the remaining iteration count from those three values, so integer and float loops run the iterations that were left. The first iteration runs the rest of the body, with the loop variable the frame saved.
- `goto continue` labels after the site.
- Frames suspended in `pcall(f, ...)` or `xpcall(f, h, ...)`. The site of the outer frame is the `pcall` call. The host builds that frame's `__lr_next` with `PROTECTED_NEXT`, which runs the continuation of the next frame inside a new `pcall` or `xpcall`, so an error raised after the restore is caught as in the original run.

Refused with an error that names the construct: a `goto` at or after the site to a label before it, a site statement of another kind, a statement with several pending calls, and loops that were not normalized.

## Capturing a coroutine

mlua opens the Lua `debug` library only through an `unsafe` constructor, and this crate forbids unsafe code. The workspace crate `lua-resume-debug` holds the one unsafe call. `new_lua_with_private_debug(libs, options)` creates the state, removes `debug` from the globals and from `package.loaded` before any script runs and returns the debug table to the host. The host builds a `Debug` from that table (`getinfo`, `getlocal`, `getupvalue`, `setupvalue`, `upvalueid`, `upvaluejoin`) and passes it to `capture`.

```rust
use lua_resume::capture::{capture, Debug, Permanents};
use lua_resume_debug::new_lua_with_private_debug;

let (lua, debug_table) = new_lua_with_private_debug(mlua::StdLib::ALL_SAFE, Default::default())?;
let debug = Debug::new(&debug_table)?;
let permanents = Permanents::standard(&lua)?; // add the engine's own natives with `insert`
// ... load normalized chunks, run a coroutine until it yields ...
let snapshot = capture(&lua, &debug, &thread, &permanents)?;
let json = serde_json::to_string(&snapshot)?;
```

`capture` accepts a coroutine whose top frame is a yielding native function (`coroutine.yield` or a native named in the permanents) and whose other frames are Lua functions in normalized code. For each frame the snapshot holds the function (chunk name and the lines of its definition), the closure, the site number from `__lrs`, every named local in stack order (generated `__lr` locals included), the varargs, the source line and the call the frame waits in. Frames run from the outermost call to the innermost.

Values are data:

- Tables are shared by identity, so cycles and aliases survive. Entries are ordered by key (booleans, integers, floats, strings, then reference keys in Lua's order) and metatables are recorded.
- Strings are exact bytes. Integers and floats stay distinct, and floats are stored as their bits, so NaN payloads, infinities and negative zero are exact.
- A Lua closure is its function plus its upvalue cells. Closures that share an upvalue (found with `debug.upvalueid`) share one cell.
- Coroutines reachable from the stack are captured too, once each. A finished one is recorded as dead. One suspended outside normalized code is refused.
- A value found in the `Permanents` (native functions, library tables, `_G`, named userdata) is saved by its name and not entered.

Capture refuses, with an error that names the path to the value (for example ``thread > level 2 > local `box` > ["inner"] > [1]``): a coroutine that is not suspended, a top frame that is not a named yield, a native frame below the top, a frame without an integer `__lrs`, unnamed stack values (`(for state)` and other temporaries), native functions and userdata that are not permanents, stripped functions, and different functions that share a chunk name, lines and shape. Lua keeps no record of `<close>` that the debug library can read, so a local holding a table with a `__close` metamethod is refused as a possible to-be-closed variable.

Limits of this step:

- Two different functions with the same chunk name and lines are told apart only by parameter count, vararg flag, upvalue count and the set of lines with code. Functions that match on all of these are not detected.
- Dead `(temporary)` slots that a vararg callee leaves in its caller's stack are recognized by count and skipped. Any other temporary is refused.
- A to-be-closed local that holds nil, false or userdata is not detected.

## Restoring a coroutine

```rust
use lua_resume::restore::{restore, Sources};

let mut sources = Sources::new();
sources.insert("=game.lua", normalized_tree); // the tree that was loaded under that chunk name
let snapshot: Snapshot = serde_json::from_str(&json)?;
let thread = restore(&lua, &debug, &snapshot, &sources, &permanents)?;
thread.resume::<mlua::MultiValue>(value)?; // continues as the original would have
```

`restore` rebuilds values first. Tables keep their identity, cycles and metatables. A Lua closure is the function that `Sources` defines at the saved chunk name and lines, and its upvalues are joined (`debug.upvaluejoin`) to one cell per saved cell, so closures that shared an upvalue still share it and writes stay visible. Permanents come from the `Permanents` by name. Coroutines reachable from the stack are restored too.

Each frame then gets a generated continuation (see above) bound to the saved closure's cells, with the saved locals and varargs. The frames are chained from the outermost to the innermost, and the innermost `__lr_next` is the native function the original waited in. The new coroutine is resumed once, so it waits in that function, and the next resume behaves like resuming the original.

The restore registers each continuation in the Lua state as the frame it continues. A later `capture` of the restored coroutine reports the original function, closure, line and locals, so save, restore, save gives equal snapshots. The tests check this at every yield.

`restore` refuses, with an error that says why: a permanent the host did not register, a chunk with no source, a function identity the source does not define, and a function whose lines, upvalue count or upvalue names differ from the saved closure.

Limits of this step:

- A function is found only when it is defined by `function`, `local function`, or as the value of a `local` or assignment. A function written directly as a call argument or table field value is not found and its frames cannot be restored.
- A new reference in the source to an enclosing local that the save did not record is read as a global. The check compares lines and the upvalue names and count the save recorded.
- A call with a nested last-argument call, such as `f(a, g())`, has one site, and the continuation takes `g` as the pending call. A coroutine waiting in `f` is continued wrongly.
- A continuation reports positions in error messages by the lines of its generated source. Re-entered loops can print their body on a later line than the original. Tests raise errors with level 0.
- Frames inside `pcall` and `xpcall` are not captured yet (capture refuses native frames), so the `PROTECTED_NEXT` path is only reached for errors raised after a restore, not for a restore from inside the protected call.
- Capture refuses `(for state)` slots, so a coroutine cannot be saved inside a numeric `for` body.

## License

MIT.
