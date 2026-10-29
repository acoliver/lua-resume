//! Coroutines suspended inside `pcall` and `xpcall`. Each script is saved at every yield,
//! restored in a fresh state through JSON and finished; the result must equal the
//! uninterrupted run. A second suite restores the thread at every yield in turn, so each save
//! is taken from a thread that an earlier restore rebuilt with its protected calls.

mod restore_support;
#[path = "restore_support/protected.rs"]
mod scripts;

use lua_resume::capture::{Callee, CaptureError, ThreadData, capture};
use restore_support::{Host, Script, round_trip_all, run, through_json};

#[test]
fn every_yield_inside_a_protected_call_restores_in_a_fresh_state() {
    let counts: Vec<usize> = scripts::SCRIPTS
        .iter()
        .map(|script| round_trip_all(std::slice::from_ref(script)))
        .collect();
    assert_eq!(counts, [1, 1, 2, 3, 3, 2, 2]);
}

/// Runs `script` to the end, saving the thread and restoring it into a fresh state at every
/// yield, and returns what it yielded and returned with the globals at the end.
fn restored_at_every_yield(script: &Script) -> (Vec<String>, String) {
    let mut host = Host::new(script.source);
    let mut thread = host.start();
    let mut events = vec![host.step(&thread, script.args)];
    while events.last().unwrap().starts_with("yield:") {
        let globals = host.dump_globals();
        let snapshot = through_json(&host.capture(&thread));
        host = Host::new(script.source);
        host.lua.load(&globals).exec().unwrap();
        thread = host.restore(&snapshot).unwrap();
        events.push(host.step(&thread, script.inputs[events.len() - 1]));
    }
    (events, host.state())
}

#[test]
fn a_thread_restored_at_every_yield_in_turn_matches_the_uninterrupted_run() {
    for script in scripts::SCRIPTS {
        let (_, _, expected) = run(script, None);
        let (events, state) = restored_at_every_yield(script);
        assert_eq!(events, expected.events, "{}", script.name);
        assert_eq!(state, expected.state, "{}: globals", script.name);
    }
}

#[test]
fn the_frame_that_calls_pcall_waits_in_pcall_and_the_inner_frame_continues_inside_it() {
    let (host, thread, _) = run(&scripts::SCRIPTS[0], Some(0));
    let snapshot = host.capture(&thread);
    let ThreadData::Suspended { frames } = &snapshot.threads[0] else {
        panic!("the thread is not suspended");
    };
    // The function pcall calls ends in a tail call, so `wait` takes its place.
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].pending, Callee::Native("pcall".to_owned()));
    assert!(matches!(frames[1].pending, Callee::Native(ref name) if name == "coroutine.yield"));
}

#[test]
fn frames_inside_the_protected_call_follow_the_pcall_frame() {
    let (host, thread, _) = run(&scripts::SCRIPTS[1], Some(0));
    let snapshot = host.capture(&thread);
    let ThreadData::Suspended { frames } = &snapshot.threads[0] else {
        panic!("the thread is not suspended");
    };
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].pending, Callee::Native("pcall".to_owned()));
    assert!(matches!(frames[1].pending, Callee::Lua(_)));
    assert!(matches!(frames[2].pending, Callee::Native(ref name) if name == "coroutine.yield"));
}

#[test]
fn an_xpcall_frame_holds_its_message_handler() {
    let (host, thread, _) = run(&scripts::SCRIPTS[5], Some(0));
    let snapshot = host.capture(&thread);
    let ThreadData::Suspended { frames } = &snapshot.threads[0] else {
        panic!("the thread is not suspended");
    };
    assert!(matches!(frames[0].pending, Callee::Xpcall { .. }));
}

#[test]
fn a_protected_call_that_is_the_outermost_frame_is_still_refused() {
    let host = Host::new(scripts::SCRIPTS[0].source);
    let pcall: mlua::Function = host.lua.globals().get("pcall").unwrap();
    let main: mlua::Function = host.lua.globals().get("main").unwrap();
    let thread = host.lua.create_thread(pcall).unwrap();
    thread.resume::<mlua::MultiValue>(main).unwrap();
    let error = capture(&host.lua, &host.debug, &thread, &host.permanents).unwrap_err();
    assert!(
        matches!(error, CaptureError::NativeFrame { ref name, .. } if name == "pcall"),
        "{error}"
    );
}
