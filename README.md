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

Under construction. The parser and printer come first, then the rewrite, then the runtime. See the issues for progress.

## License

MIT.
