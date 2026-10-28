//! Differential suite and refusals. At every yield of each script, the coroutine is captured, serialized
//! as JSON, restored in a fresh Lua state and continued to the end. What it yields, returns
//! and raises, and the globals it leaves, must equal the uninterrupted run. The restored
//! coroutine is captured and restored again, and the second snapshot must equal the first.

mod restore_support;

use lua_resume::capture::{Permanents, Snapshot};
use lua_resume::restore::{RestoreError, Sources, restore};
use restore_support::scripts::SCRIPTS;
use restore_support::{Host, Script, run, through_json};

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
    let host = Host::new(script.source);
    host.lua.load(globals).exec().unwrap();
    host
}

/// Round-trips yield `index`; returns nothing but panics on any difference.
fn round_trip(script: &Script, index: usize, expected: &restore_support::Run) {
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

#[test]
fn every_yield_of_every_script_restores_in_a_fresh_state() {
    let mut points = 0;
    for script in SCRIPTS {
        let (_, _, expected) = run(script, None);
        let yields = expected.events.len() - 1;
        assert!(yields > 0, "{} never yields", script.name);
        for index in 0..yields {
            round_trip(script, index, &expected);
            points += 1;
        }
    }
    eprintln!("{points} resume points");
    assert!(points >= 20, "only {points} resume points");
}

// A snapshot is refused when the host cannot supply what it names or the source changed.

/// The closures script, saved at its first yield.
fn saved() -> (Snapshot, &'static str) {
    let script = &SCRIPTS[2];
    let (host, thread, _) = run(script, Some(0));
    (host.capture(&thread), script.source)
}

#[test]
fn a_missing_permanent_is_refused_by_name() {
    let (snapshot, source) = saved();
    let host = Host::new(source);
    let result = restore(
        &host.lua,
        &host.debug,
        &snapshot,
        &host.sources(),
        &Permanents::new(),
    );
    assert!(matches!(result, Err(RestoreError::MissingPermanent(_))));
}

#[test]
fn a_chunk_without_source_is_refused() {
    let (snapshot, source) = saved();
    let host = Host::new(source);
    let result = restore(
        &host.lua,
        &host.debug,
        &snapshot,
        &Sources::new(),
        &host.permanents,
    );
    assert!(matches!(result, Err(RestoreError::UnknownChunk(chunk)) if chunk == "=script.lua"));
}

#[test]
fn an_unknown_function_identity_is_refused() {
    let (mut snapshot, source) = saved();
    snapshot.closures[0].function.line = 99;
    let host = Host::new(source);
    let result = host.restore(&snapshot);
    assert!(matches!(
        result,
        Err(RestoreError::UnknownFunction { line: 99, .. })
    ));
}

#[test]
fn a_function_whose_upvalues_changed_since_the_save_is_refused() {
    let (snapshot, source) = saved();
    let changed = source
        .replace("local n = 0", "local m = 0")
        .replace("n = n + d", "m = m + d")
        .replace("'inc', n)", "'inc', m)")
        .replace("return n\n", "return m\n")
        .replace("get() return n end", "get() return m end");
    assert_ne!(changed, source);
    let host = Host::new(&changed);
    let result = host.restore(&snapshot);
    assert!(
        matches!(result, Err(RestoreError::SourceChanged { .. })),
        "{result:?}"
    );
}

#[test]
fn a_function_that_moved_since_the_save_is_refused() {
    let (snapshot, source) = saved();
    let moved = format!("\n{source}");
    let host = Host::new(&moved);
    let result = host.restore(&snapshot);
    assert!(matches!(
        result,
        Err(RestoreError::UnknownFunction { .. } | RestoreError::SourceChanged { .. })
    ));
}
