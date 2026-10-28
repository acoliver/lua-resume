//! Old saves restore: for each shape of frame, the Lua 5.3 stack is built as the 5.3 VM held
//! it at a yield of `cases.lua`, imported, restored in a fresh Lua 5.4 state and resumed. What
//! it yields and returns from there must equal the rest of the uninterrupted run.

mod import_support;

use import_support::{
    CHUNK, Extra, Host, SAVED_SOURCE, SOURCE, World, int, line_of, string, yield_fn,
};
use lua_resume::capture::Permanents;
use lua_resume::capture::{TableData, Value};
use lua_resume::import::{ImportChunk, ImportError, ImportSources, PermanentMap, import_thread};
use lua_resume::restore::{restore, restore_all};
use lua_resume::strip::strip_debug_calls;
use lua_resume::syntax::parse;
use mlua::{Function, Thread};

fn host() -> Host {
    Host::new(SOURCE)
}

fn sources(host: &Host) -> ImportSources {
    let mut stripped = parse(SOURCE.as_bytes()).unwrap();
    strip_debug_calls(&mut stripped);
    let mut sources = ImportSources::new();
    let chunk = ImportChunk::new(CHUNK, &stripped, host.tree.clone()).unwrap();
    sources.insert(SAVED_SOURCE, chunk);
    sources
}

fn start(host: &Host, name: &str) -> Thread {
    let function: Function = host.lua.globals().get(name).unwrap();
    host.lua.create_thread(function).unwrap()
}

/// What the function yields and returns when it runs from the start to the end.
fn uninterrupted(name: &str, args: &str, inputs: &[&str]) -> Vec<String> {
    let host = host();
    let thread = start(&host, name);
    let mut events = vec![host.step(&thread, args)];
    for input in inputs {
        assert!(events.last().unwrap().starts_with("yield:"), "{events:?}");
        events.push(host.step(&thread, input));
    }
    events
}

/// Imports `world`, restores it in a fresh state and feeds it `inputs` from yield `at`.
fn resumed(world: &World, name: &str, args: &str, inputs: &[&str], at: usize) {
    let expected = uninterrupted(name, args, inputs);
    let fresh = host();
    let imported = import_thread(
        &world.thread,
        &world.heap,
        &sources(&fresh),
        &fresh.permanents,
        &PermanentMap::default(),
    )
    .unwrap();
    let thread = restore(
        &fresh.lua,
        &fresh.debug,
        &imported.snapshot,
        &sources(&fresh).restore_sources(),
        &fresh.permanents,
    )
    .unwrap();
    let rest: Vec<String> = inputs[at..]
        .iter()
        .map(|input| fresh.step(&thread, input))
        .collect();
    assert_eq!(rest[..], expected[at + 1..], "{name}");
}

fn list(values: &[i64]) -> TableData {
    TableData {
        entries: (1..).zip(values).map(|(i, v)| (int(i), int(*v))).collect(),
        metatable: None,
    }
}

#[test]
fn a_nested_call_resumes_both_frames() {
    let mut world = World::new();
    let leaf_cell = world.cell(Value::Nil);
    let leaf = world.closure(line_of("leaf"), &[]);
    world.heap.cells[leaf_cell].value = Value::Closure(leaf);
    let nested = world.closure(line_of("nested"), &[("leaf", leaf_cell)]);
    world.frame(nested, 0, &[int(5)], Value::Closure(leaf));
    world.frame(leaf, 0, &[int(5)], yield_fn());
    world.yielded();
    resumed(&world, "nested", "5", &["7"], 0);
}

#[test]
fn an_imported_snapshot_restores_as_one_captured_coroutine_without_roots() {
    let mut world = World::new();
    let numfor = world.closure(line_of("numfor"), &[]);
    let registers = [int(3), int(10), int(2), int(3), int(1), int(2)];
    world.frame(numfor, 0, &registers, yield_fn());
    world.yielded();
    let expected = uninterrupted("numfor", "3", &["10", "20", "30"]);
    let fresh = host();
    let imported = import_thread(
        &world.thread,
        &world.heap,
        &sources(&fresh),
        &fresh.permanents,
        &PermanentMap::default(),
    )
    .unwrap();
    assert_eq!(imported.snapshot.captured, [0]);
    assert_eq!(imported.snapshot.roots.len(), 0);
    let restored = restore_all(
        &fresh.lua,
        &fresh.debug,
        &imported.snapshot,
        &sources(&fresh).restore_sources(),
        &fresh.permanents,
    )
    .unwrap();
    assert_eq!(restored.roots.len(), 0);
    let [thread] = restored.threads.as_slice() else {
        panic!("expected one captured coroutine");
    };
    let rest: Vec<String> = ["20", "30"]
        .iter()
        .map(|input| fresh.step(thread, input))
        .collect();
    assert_eq!(rest[..], expected[2..]);
}

#[test]
fn a_numeric_for_resumes_with_its_hidden_registers() {
    let mut world = World::new();
    let numfor = world.closure(line_of("numfor"), &[]);
    // Stopped in the second iteration of `for i = 1, 3`, after 10 was added to `sum`.
    let registers = [int(3), int(10), int(2), int(3), int(1), int(2)];
    world.frame(numfor, 0, &registers, yield_fn());
    world.yielded();
    resumed(&world, "numfor", "3", &["10", "20", "30"], 1);
}

#[test]
fn an_ipairs_loop_resumes_with_the_iterator_permanent() {
    let mut world = World::new();
    world.heap.tables.push(list(&[10, 20, 30]));
    world.heap.tables.push(list(&[111]));
    let listloop = world.closure(line_of("listloop"), &[]);
    let aux = Value::Permanent("__ipairs_aux__".to_owned());
    let registers = [
        Value::Table(0),
        Value::Table(1),
        aux,
        Value::Table(0),
        int(2),
        int(2),
        int(20),
    ];
    world.frame(listloop, 1, &registers, yield_fn());
    world.yielded();
    resumed(
        &world,
        "listloop",
        "{10, 20, 30}",
        &["111", "222", "333"],
        1,
    );
}

#[test]
fn varargs_come_from_the_slots_between_the_parameters_and_the_base() {
    let mut world = World::new();
    let va = world.closure(line_of("va"), &[]);
    let varargs = [int(2), int(3)];
    let extra = Extra {
        varargs: varargs.to_vec(),
        tail: false,
    };
    world.frame_with(va, 1, &[int(1), int(2)], yield_fn(), &extra);
    world.yielded();
    resumed(&world, "va", "1, 2, 3", &["9"], 0);
}

#[test]
fn a_frame_entered_by_a_tail_call_stopped_in_a_tail_call_resumes() {
    let mut world = World::new();
    let tailer = world.closure(line_of("tailer"), &[]);
    let extra = Extra {
        varargs: Vec::new(),
        tail: true,
    };
    world.frame_with(tailer, 0, &[int(4)], yield_fn(), &extra);
    world.yielded();
    resumed(&world, "tailer", "4", &["8"], 0);
}

#[test]
fn a_yieldable_pcall_becomes_a_protected_frame() {
    let mut world = World::new();
    let leaf_cell = world.cell(Value::Nil);
    let leaf = world.closure(line_of("leaf"), &[]);
    world.heap.cells[leaf_cell].value = Value::Closure(leaf);
    let protected = world.closure(line_of("protected"), &[("leaf", leaf_cell)]);
    let sprocall = Value::Permanent("__sprocall".to_owned());
    world.frame(protected, 0, &[int(6)], sprocall);
    world.protected_call();
    world.frame(leaf, 0, &[int(6)], yield_fn());
    world.yielded();
    resumed(&world, "protected", "6", &["20"], 0);
}

#[test]
fn an_open_upvalue_gives_its_closure_the_slot_value() {
    let mut world = World::new();
    let count_cell = world.cell(Value::Nil);
    let bump = world.closure(line_of("bump"), &[("count", count_cell)]);
    let upv = world.closure(line_of("upv"), &[]);
    let registers = [int(10), int(10), Value::Closure(bump)];
    let base = world.frame(upv, 0, &registers, yield_fn());
    world.yielded();
    world.thread.open_upvalues.push((base + 1, count_cell));
    let imported = import_thread(
        &world.thread,
        &world.heap,
        &sources(&host()),
        &host().permanents,
        &PermanentMap::default(),
    )
    .unwrap();
    assert_eq!(imported.aliased, [(0, "count".to_owned())]);
    resumed(&world, "upv", "10", &["5"], 0);
}

fn refused(world: &World, permanents: &Permanents) -> ImportError {
    import_thread(
        &world.thread,
        &world.heap,
        &sources(&host()),
        permanents,
        &PermanentMap::default(),
    )
    .unwrap_err()
}

#[test]
fn temporaries_below_the_callee_are_refused_with_the_frame() {
    let mut world = World::new();
    let temps = world.closure(line_of("temps"), &[]);
    let registers = [Value::Table(0), Value::Nil, Value::Nil];
    world.heap.tables.push(TableData::default());
    world.frame(temps, 0, &registers, yield_fn());
    world.yielded();
    let error = refused(&world, &host().permanents);
    assert_eq!(error, ImportError::Temporaries { frame: 0, count: 2 });
    assert!(error.to_string().contains("frame 0"));
}

#[test]
fn a_native_the_host_did_not_register_is_refused() {
    let mut world = World::new();
    let tailer = world.closure(line_of("tailer"), &[]);
    let sleep = Value::Permanent("cthread.Sleep".to_owned());
    world.frame(tailer, 0, &[string("x")], sleep);
    world.yielded();
    let error = refused(&world, &host().permanents);
    assert!(
        matches!(error, ImportError::MissingPermanent { ref name, .. } if name == "cthread.Sleep")
    );
}

#[test]
fn an_unknown_prototype_and_a_native_frame_in_the_middle_are_refused() {
    let mut world = World::new();
    let tailer = world.closure(line_of("tailer"), &[]);
    world.frame(tailer, 0, &[int(1)], yield_fn());
    world.yielded();
    world.thread.frames[0].call_status = 0;
    let error = refused(&world, &host().permanents);
    assert!(
        error.to_string().starts_with("frame 0: a native frame"),
        "{error}"
    );
    world.thread.frames[0].call_status = lua_resume::import::status::LUA;
    world.heap.closures[tailer].proto = 999;
    assert!(matches!(
        refused(&world, &host().permanents),
        ImportError::UnknownProto { index: 999, .. }
    ));
}
