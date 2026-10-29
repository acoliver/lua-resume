//! A host that runs a normalized script in a Lua state, drives its coroutine and saves it.

#[path = "../hook_support/mod.rs"]
mod hook_support;
#[path = "../support/mod.rs"]
mod normalizer;

use lua_resume::capture::{Debug, Permanents, Snapshot, capture};
use lua_resume::restore::{RestoreError, Sources, restore};
use lua_resume::syntax::ast::Block;
use lua_resume::syntax::{OperatorHooks, print_with};
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
    /// Whether the source is loaded with `/` printed as a call of the host division
    /// function `__lr_div`.
    pub hooked: bool,
}

pub struct Host {
    pub lua: Lua,
    pub debug: Debug,
    pub permanents: Permanents,
    pub tree: Block,
    pub hooks: OperatorHooks,
}

impl Host {
    /// A new Lua state with `source` normalized and loaded as the chunk `script.lua`.
    pub fn new(source: &str) -> Self {
        Self::with_hooks(source, OperatorHooks::none())
    }

    /// Like [`Host::new`], printing the source with `hooks`; a division hook is installed.
    pub fn with_hooks(source: &str, hooks: OperatorHooks) -> Self {
        let (lua, table) =
            new_lua_with_private_debug(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
        let debug = Debug::new(&table).unwrap();
        let mut permanents = Permanents::standard(&lua).unwrap();
        let (aux, _, _): (mlua::Value, mlua::Value, mlua::Value) =
            lua.load("return ipairs({})").eval().unwrap();
        permanents.insert("ipairs iterator", aux).unwrap();
        lua.load(HELPERS).exec().unwrap();
        let tree = normalizer::normalize_source(source.as_bytes()).unwrap();
        hook_support::install(&lua, &hooks);
        lua.load(print_with(&tree, &hooks))
            .set_name(CHUNK)
            .exec()
            .unwrap();
        Self {
            lua,
            debug,
            permanents,
            tree,
            hooks,
        }
    }

    /// A host for `script`, with the division hook if the script asks for it.
    pub fn for_script(script: &Script) -> Self {
        if script.hooked {
            Self::with_hooks(script.source, hook_support::division_hooks())
        } else {
            Self::new(script.source)
        }
    }

    pub fn sources(&self) -> Sources {
        let mut sources = Sources::new();
        sources.insert(CHUNK, self.tree.clone());
        sources.with_hooks(self.hooks.clone())
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
    let host = Host::for_script(script);
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

/// Continues `thread` in `host` through the remaining inputs from yield `from`.
fn finish(host: &Host, thread: &mlua::Thread, script: &Script, from: usize) -> Vec<String> {
    let mut events = Vec::new();
    let mut index = from;
    loop {
        events.push(host.step(thread, script.inputs[index]));
        if !events.last().unwrap().starts_with("yield:") {
            return events;
        }
        index += 1;
    }
}

fn fresh_host(script: &Script, globals: &str) -> Host {
    let host = Host::for_script(script);
    host.lua.load(globals).exec().unwrap();
    host
}

/// Round-trips yield `index`; returns nothing but panics on any difference.
fn round_trip(script: &Script, index: usize, expected: &Run) {
    let (before, thread, run) = run(script, Some(index));
    assert_eq!(run.events[..], expected.events[..=index], "{}", script.name);
    let globals = before.dump_globals();
    let first = through_json(&before.capture(&thread));

    let second_host = fresh_host(script, &globals);
    let restored = second_host.restore(&first).unwrap();
    let again = through_json(&second_host.capture(&restored));
    assert_eq!(again, first, "{}: re-capture at yield {index}", script.name);

    let rest = finish(&second_host, &restored, script, index);
    let label = format!("{}: restored at yield {index}", script.name);
    assert_eq!(rest[..], expected.events[index + 1..], "{label}");
    assert_eq!(second_host.state(), expected.state, "{label}: globals");

    let third_host = fresh_host(script, &globals);
    let twice = third_host.restore(&again).unwrap();
    let rest = finish(&third_host, &twice, script, index);
    assert_eq!(
        rest[..],
        expected.events[index + 1..],
        "{label}, restored twice"
    );
    assert_eq!(
        third_host.state(),
        expected.state,
        "{label}: globals, twice"
    );
}

/// Round-trips every yield of every script and returns how many resume points it checked.
pub fn round_trip_all(scripts: &[Script]) -> usize {
    let mut points = 0;
    for script in scripts {
        let (_, _, expected) = run(script, None);
        let yields = expected.events.len() - 1;
        assert!(yields > 0, "{} never yields", script.name);
        for index in 0..yields {
            round_trip(script, index, &expected);
            points += 1;
        }
    }
    points
}
