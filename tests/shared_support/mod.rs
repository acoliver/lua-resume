//! A host for the shared-graph tests: a two-coroutine round-robin scheduler over one world.

#[path = "../support/mod.rs"]
mod normalizer;

use lua_resume::capture::{CaptureError, Debug, Permanents, Snapshot, capture_all};
use lua_resume::restore::{Restored, Sources, restore_all};
use lua_resume::syntax::ast::Block;
use lua_resume::syntax::print;
use lua_resume_debug::new_lua_with_private_debug;
use std::ops::Range;

use mlua::{Function, Lua, LuaOptions, StdLib, Table, Thread, ThreadStatus, Value};

const CHUNK: &str = "=sched.lua";
const SOURCE: &str = include_str!("sched.lua");

/// The scheduler resumes the two coroutines in turn this many times in all.
pub const STEPS: usize = 8;

pub struct Host {
    pub lua: Lua,
    pub debug: Debug,
    pub permanents: Permanents,
    tree: Block,
}

/// The world table and the two coroutines of the scheduler.
pub struct World {
    pub table: Table,
    pub threads: [Thread; 2],
}

impl Host {
    pub fn new() -> Self {
        let (lua, table) =
            new_lua_with_private_debug(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
        let debug = Debug::new(&table).unwrap();
        let permanents = Permanents::standard(&lua).unwrap();
        let tree = normalizer::normalize_source(SOURCE.as_bytes()).unwrap();
        lua.load(print(&tree)).set_name(CHUNK).exec().unwrap();
        Self {
            lua,
            debug,
            permanents,
            tree,
        }
    }

    fn sources(&self) -> Sources {
        let mut sources = Sources::new();
        sources.insert(CHUNK, self.tree.clone());
        sources
    }

    pub fn new_world(&self) -> World {
        let new_world: Function = self.lua.globals().get("new_world").unwrap();
        let table: Table = new_world.call(()).unwrap();
        let threads = [table.get("a").unwrap(), table.get("b").unwrap()];
        World { table, threads }
    }

    /// Runs the scheduler steps `steps`, resuming the coroutine `step % 2` when it can still
    /// run, and returns what each resume gave back.
    pub fn drive(threads: &[Thread; 2], steps: Range<usize>) -> Vec<String> {
        let mut events = Vec::new();
        for step in steps {
            let thread = &threads[step % 2];
            if thread.status() == ThreadStatus::Resumable {
                let value = thread.resume::<Option<i64>>(()).unwrap();
                events.push(format!("{step}: {value:?}"));
            }
        }
        events
    }

    /// Calls the Lua global `name` with the world table.
    pub fn call(&self, name: &str, world: &Table) -> String {
        let function: Function = self.lua.globals().get(name).unwrap();
        function.call(world.clone()).unwrap()
    }

    pub fn try_capture(
        &self,
        roots: &[(&str, Value)],
        threads: &[Thread],
    ) -> Result<Snapshot, CaptureError> {
        capture_all(&self.lua, &self.debug, roots, threads, &self.permanents)
    }

    pub fn capture(&self, roots: &[(&str, Value)], threads: &[Thread]) -> Snapshot {
        self.try_capture(roots, threads).unwrap()
    }

    pub fn restore(&self, snapshot: &Snapshot) -> Restored {
        restore_all(
            &self.lua,
            &self.debug,
            snapshot,
            &self.sources(),
            &self.permanents,
        )
        .unwrap()
    }
}

pub fn through_json(snapshot: &Snapshot) -> Snapshot {
    serde_json::from_str(&serde_json::to_string(snapshot).unwrap()).unwrap()
}
