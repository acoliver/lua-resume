//! A host that runs a normalized script in a Lua state, drives its coroutine and saves it.

#[path = "../support/mod.rs"]
mod normalizer;

use lua_resume::capture::{Debug, Permanents, Snapshot, capture};
use lua_resume::restore::{RestoreError, Sources, restore};
use lua_resume::syntax::ast::Block;
use lua_resume::syntax::print;
use lua_resume_debug::new_lua_with_private_debug;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Thread};

pub const CHUNK: &str = "=script.lua";
const HELPERS: &str = include_str!("helpers.lua");

pub struct Script {
    pub name: &'static str,
    pub source: &'static str,
    /// A Lua list of arguments for the first resume.
    pub args: &'static str,
    /// One Lua list of values per later resume.
    pub inputs: &'static [&'static str],
}

pub struct Host {
    pub lua: Lua,
    pub debug: Debug,
    pub permanents: Permanents,
    pub tree: Block,
}

impl Host {
    /// A new Lua state with `source` normalized and loaded as the chunk `script.lua`.
    pub fn new(source: &str) -> Self {
        let (lua, table) =
            new_lua_with_private_debug(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
        let debug = Debug::new(&table).unwrap();
        let mut permanents = Permanents::standard(&lua).unwrap();
        let (aux, _, _): (mlua::Value, mlua::Value, mlua::Value) =
            lua.load("return ipairs({})").eval().unwrap();
        permanents.insert("ipairs iterator", aux).unwrap();
        lua.load(HELPERS).exec().unwrap();
        let tree = normalizer::normalize_source(source.as_bytes()).unwrap();
        lua.load(print(&tree)).set_name(CHUNK).exec().unwrap();
        Self {
            lua,
            debug,
            permanents,
            tree,
        }
    }

    pub fn sources(&self) -> Sources {
        let mut sources = Sources::new();
        sources.insert(CHUNK, self.tree.clone());
        sources
    }

    /// A coroutine running the global function `main`.
    pub fn start(&self) -> Thread {
        let main: Function = self.lua.globals().get("main").unwrap();
        self.lua.create_thread(main).unwrap()
    }

    /// Resumes `thread` with the Lua list `values` and describes what came back.
    pub fn step(&self, thread: &Thread, values: &str) -> String {
        let args: Function = self.lua.globals().get("__args").unwrap();
        let step: Function = self.lua.globals().get("__step").unwrap();
        let values: Table = args.call(values).unwrap();
        let values: mlua::MultiValue = values
            .sequence_values::<mlua::Value>()
            .map(Result::unwrap)
            .collect();
        step.call((thread.clone(), values)).unwrap()
    }

    pub fn state(&self) -> String {
        self.lua
            .globals()
            .get::<Function>("__state")
            .unwrap()
            .call(())
            .unwrap()
    }

    pub fn dump_globals(&self) -> String {
        let dump: Function = self.lua.globals().get("__dump_globals").unwrap();
        dump.call(()).unwrap()
    }

    pub fn capture(&self, thread: &Thread) -> Snapshot {
        capture(&self.lua, &self.debug, thread, &self.permanents).unwrap()
    }

    pub fn restore(&self, snapshot: &Snapshot) -> Result<Thread, RestoreError> {
        restore(
            &self.lua,
            &self.debug,
            snapshot,
            &self.sources(),
            &self.permanents,
        )
    }
}

pub fn through_json(snapshot: &Snapshot) -> Snapshot {
    serde_json::from_str(&serde_json::to_string(snapshot).unwrap()).unwrap()
}

/// The events of an uninterrupted run and the globals at its end.
pub struct Run {
    pub events: Vec<String>,
    pub state: String,
}

/// Runs `script` until its coroutine returns or fails. With `stop`, runs to that yield and
/// returns the host and thread still suspended there.
pub fn run(script: &Script, stop: Option<usize>) -> (Host, Thread, Run) {
    let host = Host::new(script.source);
    let thread = host.start();
    let mut events = vec![host.step(&thread, script.args)];
    while events
        .last()
        .is_some_and(|event| event.starts_with("yield:"))
    {
        if stop == Some(events.len() - 1) {
            break;
        }
        let input = script.inputs[events.len() - 1];
        events.push(host.step(&thread, input));
    }
    let state = host.state();
    (host, thread, Run { events, state })
}

pub mod scripts;
