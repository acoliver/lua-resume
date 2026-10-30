//! Normalization stages a call into its own statement unless it is the last argument of
//! another call, where its several results are all passed on, or inside a table constructor,
//! which holds the table it builds. A frame suspended in a staged call has no pending value
//! below its callee, so capture never meets an unnamed stack value. A frame suspended in the
//! other two shapes has the outer callee, the earlier arguments or the new table pending below
//! the call, and capture refuses it.

mod restore_support;

use lua_resume::capture::{CaptureError, capture};
use restore_support::{Host, Script, round_trip_all};

const STAGED_YIELDS: Script = Script {
    hooked: false,
    name: "yields inside expressions",
    source: "function main()
  local r = {}
  r[1] = 10 + (coroutine.yield('op')) * 2
  r[2] = 'a' .. coroutine.yield('cat1') .. 'b' .. coroutine.yield('cat2')
  r[3] = 7 - (coroutine.yield('sub'))
  r[4] = ((coroutine.yield('and')) > 5 and 1) or 0
  for i = 1, 2 do r[#r + 1] = i + (coroutine.yield('for')) end
  count = #r
  return r
end
",
    args: "",
    inputs: &["10", "20", "30", "40", "50", "60", "70", "80"],
};

#[test]
fn no_temporary_is_live_below_a_staged_call() {
    assert_eq!(round_trip_all(&[STAGED_YIELDS]), 7);
}

#[test]
fn calls_with_values_pending_below_them_are_refused_as_unnamed_temporaries() {
    let shapes = [
        "function main() return tostring(coroutine.yield('last')) end",
        "function main() local t = {(coroutine.yield('field')), 3} return t end",
    ];
    for source in shapes {
        let host = Host::new(source);
        let thread = host.start();
        host.step(&thread, "");
        let error = capture(&host.lua, &host.debug, &thread, &host.permanents).unwrap_err();
        let CaptureError::Unnamed { path, name } = error else {
            panic!("{source}: unexpected {error}");
        };
        assert_eq!(name, "(temporary)", "{source}");
        assert!(path.starts_with("thread > level 1 > local `"), "{path}");
    }
}
