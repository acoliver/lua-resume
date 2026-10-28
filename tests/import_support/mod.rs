//! Builds the stack and `CallInfo` records Lua 5.3 holds when a coroutine is stopped in a
//! yielding native function, from the compiled `cases.luac`.

#[path = "../support/mod.rs"]
mod normalizer;

use lua_resume::capture::{Cell, Debug, Permanents, Value};
use lua_resume::import::{SavedClosure, SavedFrame, SavedHeap, SavedThread, status};
use lua_resume::lua53::chunk::read_chunk;
use lua_resume::lua53::op::{CallKind, call_kind, register_a};
use lua_resume::syntax::ast::Block;
use lua_resume::syntax::print;
use lua_resume_debug::new_lua_with_private_debug;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Thread};

pub const CHUNK: &str = "=script.lua";
const HELPERS: &str = include_str!("../restore_support/helpers.lua");

/// A Lua 5.4 state with the normalized source loaded as the chunk `script.lua`.
pub struct Host {
    pub lua: Lua,
    pub debug: Debug,
    pub permanents: Permanents,
    pub tree: Block,
}

impl Host {
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
}

pub const SOURCE: &str = include_str!("../fixtures/lua53/cases.lua");
const COMPILED: &[u8] = include_bytes!("../fixtures/lua53/cases.luac");
pub const SAVED_SOURCE: &[u8] = b"@Lua/cases.lua";

/// What distinguishes a frame beyond its registers.
#[derive(Default)]
pub struct Extra {
    pub varargs: Vec<Value>,
    pub tail: bool,
}

pub struct World {
    pub heap: SavedHeap,
    pub thread: SavedThread,
    next_func: usize,
}

/// The line `function name(` or `local function name(` starts on.
pub fn line_of(name: &str) -> u32 {
    let line = SOURCE
        .lines()
        .position(|line| {
            line.trim_start()
                .trim_start_matches("local ")
                .starts_with(&format!("function {name}("))
        })
        .unwrap_or_else(|| panic!("no function {name}"));
    u32::try_from(line).unwrap() + 1
}

impl World {
    /// A heap with the prototypes of `cases.luac` and cell 0 holding `_G`.
    pub fn new() -> Self {
        let chunk = read_chunk(COMPILED).unwrap();
        let mut heap = SavedHeap {
            protos: chunk.main.flatten().iter().map(|f| f.0.erased()).collect(),
            ..SavedHeap::default()
        };
        heap.cells.push(Cell {
            value: Value::Permanent("_G".to_owned()),
        });
        let thread = SavedThread {
            stack: vec![Value::Nil],
            ..SavedThread::default()
        };
        Self {
            heap,
            thread,
            next_func: 1,
        }
    }

    pub fn cell(&mut self, value: Value) -> usize {
        self.heap.cells.push(Cell { value });
        self.heap.cells.len() - 1
    }

    /// A closure of the function starting at `line`; `_ENV` shares cell 0 and other upvalues
    /// are taken from `cells` by name.
    pub fn closure(&mut self, line: u32, cells: &[(&str, usize)]) -> usize {
        let proto = self
            .heap
            .protos
            .iter()
            .position(|p| p.line_defined == line)
            .unwrap_or_else(|| panic!("no prototype at line {line}"));
        let upvalues = self.heap.protos[proto]
            .upvalues
            .iter()
            .map(|upvalue| {
                let name = String::from_utf8_lossy(&upvalue.name).into_owned();
                if name == "_ENV" {
                    return 0;
                }
                cells
                    .iter()
                    .find(|(found, _)| *found == name)
                    .unwrap_or_else(|| panic!("no cell for upvalue {name}"))
                    .1
            })
            .collect();
        self.heap.closures.push(SavedClosure { proto, upvalues });
        self.heap.closures.len() - 1
    }

    /// Pushes the Lua frame running `closure`, stopped at the call with this ordinal.
    /// `registers` are the slots below the callee, `varargs` the extra arguments, and
    /// `callee` the value in the callee register. Returns the stack index of the first
    /// register.
    pub fn frame(
        &mut self,
        closure: usize,
        ordinal: usize,
        registers: &[Value],
        callee: Value,
    ) -> usize {
        self.frame_with(closure, ordinal, registers, callee, &Extra::default())
    }

    /// Like [`World::frame`], for a vararg function or a frame entered by a tail call.
    pub fn frame_with(
        &mut self,
        closure: usize,
        ordinal: usize,
        registers: &[Value],
        callee: Value,
        extra: &Extra,
    ) -> usize {
        let (varargs, tail) = (&extra.varargs, extra.tail);
        let proto = self.heap.protos[self.heap.closures[closure].proto].clone();
        let func = self.next_func;
        self.thread.stack.truncate(func);
        self.thread.stack.push(Value::Closure(closure));
        let base = if proto.is_vararg {
            let fixed = usize::from(proto.num_params);
            self.thread
                .stack
                .extend(std::iter::repeat_n(Value::Nil, fixed));
            self.thread.stack.extend(varargs.iter().cloned());
            self.thread.stack.len()
        } else {
            func + 1
        };
        let pc = proto
            .code
            .iter()
            .enumerate()
            .filter(|(_, word)| call_kind(**word).is_some())
            .nth(ordinal)
            .map(|(pc, _)| pc)
            .expect("the ordinal names a call");
        // A generic `for` calls a copy placed after its three hidden registers.
        let hidden = if call_kind(proto.code[pc]) == Some(CallKind::IterCall) {
            3
        } else {
            0
        };
        let window = usize::from(register_a(proto.code[pc])) + hidden;
        assert_eq!(registers.len(), window, "registers below the callee");
        self.thread.stack.extend(registers.iter().cloned());
        self.thread.stack.push(callee);
        self.next_func = base + window;
        let mut call_status = status::LUA;
        if tail {
            call_status |= status::TAIL;
        }
        self.thread.frames.push(SavedFrame {
            func,
            base,
            saved_pc: pc + 1,
            call_status,
            continuation: None,
        });
        base
    }

    /// The `__sprocall` frame between a Lua frame and the function it calls.
    pub fn protected_call(&mut self) {
        self.thread.frames.push(SavedFrame {
            func: self.next_func,
            base: 0,
            saved_pc: 0,
            call_status: status::YPCALL,
            continuation: Some("__finish_sprocall".to_owned()),
        });
        self.next_func += 1;
    }

    /// The yielding native on top of the innermost Lua frame.
    pub fn yielded(&mut self) {
        self.thread.frames.push(SavedFrame {
            func: self.next_func,
            base: 0,
            saved_pc: 0,
            call_status: 0,
            continuation: None,
        });
    }
}

pub fn yield_fn() -> Value {
    Value::Permanent("coroutine.yield".to_owned())
}

pub fn int(value: i64) -> Value {
    Value::Integer(value)
}

pub fn string(value: &str) -> Value {
    Value::String(value.as_bytes().to_vec())
}
