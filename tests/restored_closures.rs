//! A closure that a restored continuation creates is saved and restored like any other. The
//! thread is restored at every yield in turn, so closures made after a restore are captured by
//! the next save.

mod restore_support;

use restore_support::{Host, Script, round_trip_all, run, through_json};

const SCRIPTS: &[Script] = &[
    Script {
        hooked: false,
        name: "a closure made in a re-entered while loop",
        source: "function main()
  local made = {}
  local i = 0
  while i < 3 do
    i = i + 1
    local k = i
    made[#made + 1] = function()
      k = k + 10
      return k
    end
    coroutine.yield('made', i)
  end
  local sum = 0
  for _, fn in ipairs(made) do sum = sum + fn() end
  count = sum
  return sum, made[1](), made[3]()
end
",
        args: "",
        inputs: &["0", "0", "0"],
    },
    Script {
        hooked: false,
        name: "a closure made after the call in the same iteration",
        source: "function main()
  local made = {}
  for i = 1, 3 do
    coroutine.yield('before', i)
    made[i] = function() return i * 2 end
  end
  local out = 0
  for i = 1, 3 do out = out + made[i]() end
  count = out
  return out
end
",
        args: "",
        inputs: &["0", "0", "0"],
    },
];

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
fn closures_made_after_a_restore_survive_the_next_save() {
    for script in SCRIPTS {
        let (_, _, expected) = run(script, None);
        let (events, state) = restored_at_every_yield(script);
        assert_eq!(events, expected.events, "{}", script.name);
        assert_eq!(state, expected.state, "{}: globals", script.name);
    }
}

#[test]
fn every_yield_of_the_closure_scripts_round_trips() {
    assert_eq!(round_trip_all(SCRIPTS), 6);
}
