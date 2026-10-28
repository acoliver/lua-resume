//! A thread stopped inside a numeric `for` that follows another loop restores correctly. Each
//! script is saved at every yield, restored in a fresh state through JSON and finished; the
//! result must equal the uninterrupted run. A second suite restores the thread at every yield
//! in turn, so each save is taken from a thread that an earlier restore rebuilt.

mod restore_support;
#[path = "restore_support/sequential_for.rs"]
mod scripts;

use restore_support::{Host, Script, round_trip_all, run, through_json};

#[test]
fn loops_that_follow_other_loops_restore_inside_their_bodies() {
    let counts: Vec<usize> = scripts::SCRIPTS
        .iter()
        .map(|script| round_trip_all(std::slice::from_ref(script)))
        .collect();
    assert_eq!(counts, [6, 9, 10, 5, 8, 8, 6, 5]);
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
