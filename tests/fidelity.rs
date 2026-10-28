//! Frame fidelity: functions found wherever they are written, `for` loops that yield, and
//! locals that closures share with their frame. Each script is saved at every yield, restored
//! in a fresh state through JSON and finished; the result must equal the uninterrupted run.

mod restore_support;
#[path = "restore_support/fidelity.rs"]
mod scripts;

use lua_resume::continuation::{ContinuationError, FunctionKey, function_source};
use lua_resume::syntax::parse;
use restore_support::round_trip_all;

#[test]
fn anonymous_functions_in_any_position_resume() {
    assert_eq!(round_trip_all(&scripts::SCRIPTS[..1]), 4);
}

#[test]
fn numeric_for_loops_resume_inside_their_bodies() {
    assert_eq!(round_trip_all(&scripts::SCRIPTS[1..2]), 13);
}

#[test]
fn closures_keep_sharing_the_locals_of_their_frame() {
    assert_eq!(round_trip_all(&scripts::SCRIPTS[2..3]), 5);
}

#[test]
fn functions_that_share_both_lines_are_refused_as_ambiguous() {
    let tree = parse(b"f(function() return 1 end, function() return 2 end)").unwrap();
    let key = FunctionKey {
        line: 1,
        end_line: 1,
    };
    assert_eq!(
        function_source(&tree, key, &[]),
        Err(ContinuationError::Ambiguous {
            line: 1,
            end_line: 1,
            count: 2
        })
    );
}
