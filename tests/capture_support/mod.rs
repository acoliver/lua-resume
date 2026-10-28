//! A host for the capture tests: a Lua state with a private debug library, the standard
//! permanents and a loader that normalizes fixtures.

#[path = "../support/mod.rs"]
mod normalizer;

use lua_resume::capture::{Debug, Permanents};
use lua_resume::syntax::print;
use lua_resume_debug::new_lua_with_private_debug;
use mlua::{Function, Lua, LuaOptions, StdLib, Thread};

pub struct Host {
    pub lua: Lua,
    pub debug: Debug,
    pub permanents: Permanents,
}

pub fn host() -> Host {
    let (lua, table) = new_lua_with_private_debug(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
    let debug = Debug::new(&table).unwrap();
    let permanents = Permanents::standard(&lua).unwrap();
    Host {
        lua,
        debug,
        permanents,
    }
}

impl Host {
    /// Normalizes `source` and runs it as the chunk `=name`.
    pub fn run(&self, source: &str, name: &str) {
        let tree = normalizer::normalize_source(source.as_bytes()).unwrap();
        self.lua
            .load(print(&tree))
            .set_name(format!("={name}"))
            .exec()
            .unwrap();
    }

    pub fn global(&self, name: &str) -> Function {
        self.lua.globals().get(name).unwrap()
    }

    /// A coroutine of the global function `entry`, resumed once with `args`.
    pub fn started(&self, entry: &str, args: impl mlua::IntoLuaMulti) -> Thread {
        let thread = self.lua.create_thread(self.global(entry)).unwrap();
        thread.resume::<mlua::MultiValue>(args).unwrap();
        thread
    }

    pub fn capture(
        &self,
        thread: &Thread,
    ) -> Result<lua_resume::capture::Snapshot, lua_resume::capture::CaptureError> {
        lua_resume::capture::capture(&self.lua, &self.debug, thread, &self.permanents)
    }
}
