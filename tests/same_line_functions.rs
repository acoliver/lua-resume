//! Functions that start and end on the same line are told apart by their parameters, vararg
//! flag and upvalue count, which capture records and restore matches against the source.

mod restore_support;

use lua_resume::capture::Snapshot;
use lua_resume::restore::RestoreError;
use restore_support::{Script, round_trip_all, run, through_json};

const DISTINGUISHABLE: Script = Script {
    hooked: false,
    name: "two functions on one line",
    source: "function one(a) local g = coroutine.yield('one', a) return a + g end function two() local g = coroutine.yield('two') return g * 2 end
function main()
  local x = one(1)
  local y = two()
  count = x + y
  return x, y
end
",
    args: "",
    inputs: &["5", "7"],
};

const SAME_SHAPE: Script = Script {
    hooked: false,
    name: "two functions on one line with the same shape",
    source: "function one() local g = coroutine.yield('one') return g + 1 end function two() local g = coroutine.yield('two') return g * 2 end
function main()
  local x = one()
  local y = two()
  return x, y
end
",
    args: "",
    inputs: &["5", "7"],
};

fn without_shapes(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("shape");
            map.values_mut().for_each(without_shapes);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(without_shapes),
        _ => {}
    }
}

#[test]
fn each_function_on_a_shared_line_resumes_where_it_stopped() {
    assert_eq!(round_trip_all(&[DISTINGUISHABLE]), 2);
}

#[test]
fn a_save_without_shapes_is_still_ambiguous() {
    let (host, thread, _) = run(&DISTINGUISHABLE, Some(0));
    let mut json = serde_json::to_value(host.capture(&thread)).unwrap();
    without_shapes(&mut json);
    let snapshot: Snapshot = serde_json::from_value(json).unwrap();
    let error = host.restore(&snapshot).unwrap_err();
    assert!(
        matches!(error, RestoreError::Continuation { .. }),
        "{error}"
    );
}

#[test]
fn functions_with_the_same_lines_and_shape_are_refused() {
    let (host, thread, _) = run(&SAME_SHAPE, Some(0));
    let snapshot = through_json(&host.capture(&thread));
    let error = host.restore(&snapshot).unwrap_err();
    assert!(
        matches!(error, RestoreError::Continuation { .. }),
        "{error}"
    );
}
