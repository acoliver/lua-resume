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
- **Continuation generation: partly done.** `continuation::continuation_source` generates, from a normalized chunk, the function that finishes a frame from one call site (see below).
- **Capture of suspended coroutines, value snapshots, frame rebuilding and engine integration: not started.** Capture needs the Lua `debug` library. mlua loads it only through `unsafe`, and this crate forbids unsafe code, so the runtime API will take the host's debug functions as an argument.

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

`continuation_source(chunk, function, site, locals, upvalues)` returns a chunk that evaluates to `function(__lr_v, __lr_next, __lr_unpack, ...)`. It declares `locals` from the table `__lr_v`, re-enters the enclosing `do`, `if` and `while` blocks (which covers generic `for` after normalization) and continues after the call statement of `site`. The call's results come from `__lr_next()`: the next inner frame, or `coroutine.yield` for the innermost frame. Functions are found by the lines their definition spans, and only functions defined by `function`, `local function`, or as the value of a `local` or assignment are found.

Supported call statements are a plain call, `local names = call` and `names = call`. Refused with an error: sites inside `repeat`, numeric `for`, or a block with a label before the site, and other statement forms.

## License

MIT.
