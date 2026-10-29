//! Differential suite and refusals. At every yield of each script, the coroutine is captured, serialized
//! as JSON, restored in a fresh Lua state and continued to the end. What it yields, returns
//! and raises, and the globals it leaves, must equal the uninterrupted run. The restored
//! coroutine is captured and restored again, and the second snapshot must equal the first.

mod restore_support;
#[path = "restore_support/scripts.rs"]
mod scripts;

use lua_resume::capture::{Permanents, Snapshot};
use lua_resume::restore::{RestoreError, Sources, check_restore, restore};
use restore_support::{Host, round_trip_all, run};
use scripts::SCRIPTS;

#[test]
fn every_yield_of_every_script_restores_in_a_fresh_state() {
    let points = round_trip_all(SCRIPTS);
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

/// The closures script where `get` also uses `inc`, a local of `counter` that `get` did not
/// capture when the snapshot was taken. Lua would compile that `inc` as a global.
fn with_new_upvalue_use(source: &str) -> String {
    let changed = source.replace(
        "local function get() return n end",
        "local function get() return inc and n end",
    );
    assert_ne!(changed, source);
    changed
}

#[test]
fn a_function_that_gained_an_enclosing_local_is_refused() {
    let (snapshot, source) = saved();
    let host = Host::new(&with_new_upvalue_use(source));
    let Err(RestoreError::SourceChanged { reason, .. }) = host.restore(&snapshot) else {
        panic!("expected SourceChanged");
    };
    assert!(reason.contains("[inc, n]"), "{reason}");
    assert!(reason.contains("the save has [n]"), "{reason}");
}

#[test]
fn check_restore_reports_a_function_that_gained_an_enclosing_local() {
    let (snapshot, source) = saved();
    let host = Host::new(&with_new_upvalue_use(source));
    let found = check_restore(
        &host.lua,
        &host.debug,
        &snapshot,
        &host.sources(),
        &host.permanents,
    );
    assert!(
        found.iter().any(|refusal| matches!(
            &refusal.error,
            RestoreError::SourceChanged { reason, .. } if reason.contains("inc")
        )),
        "{found:?}"
    );
}

#[test]
fn a_global_with_the_name_of_no_enclosing_local_is_not_an_upvalue() {
    let (snapshot, source) = saved();
    let changed = source.replace(
        "local function get() return n end",
        "local function get() return print and n end",
    );
    let host = Host::new(&changed);
    let result = host.restore(&snapshot);
    assert!(result.is_ok(), "{result:?}");
}
