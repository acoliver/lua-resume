//! Mapping suspended Lua 5.3 frames onto the stripped, normalized source.
//!
//! `sample.luac` is the stock Lua 5.3.6 compile of `sample.stripped.lua`, which is
//! `sample.lua` without its `assert` and `dbg` statements (see the fixture README).

use lua_resume::compiled::Compiled;
use lua_resume::lua53::chunk::read_chunk;
use lua_resume::lua53::frame::{FrameError, MapError, describe_frame, map_frame, vararg_slots};
use lua_resume::lua53::op::{CallKind, call_kind};
use lua_resume::lua53::sites::{FunctionIndex, SiteError, SiteKind};
use lua_resume::rewrite::normalize;
use lua_resume::strip::strip_debug_calls;
use lua_resume::syntax::ast::Block;
use lua_resume::syntax::parse;
use mlua::{ChunkMode, Lua};

const ORIGINAL: &[u8] = include_bytes!("fixtures/lua53/sample.lua");
const STRIPPED: &[u8] = include_bytes!("fixtures/lua53/sample.stripped.lua");
const COMPILED: &[u8] = include_bytes!("fixtures/lua53/sample.luac");

fn stripped_tree() -> Block {
    let mut tree = parse(ORIGINAL).unwrap();
    strip_debug_calls(&mut tree);
    tree
}

fn normalized_tree() -> Block {
    let dump = Lua::new()
        .load(STRIPPED)
        .set_mode(ChunkMode::Text)
        .into_function()
        .unwrap()
        .dump(true);
    normalize(&stripped_tree(), &Compiled::from_dump(&dump).unwrap()).unwrap()
}

#[test]
fn stripping_the_original_gives_the_source_the_game_compiled() {
    assert_eq!(stripped_tree(), parse(STRIPPED).unwrap());
}

#[test]
fn every_compiled_function_matches_its_source_calls() {
    let index = FunctionIndex::new(&stripped_tree());
    let chunk = read_chunk(COMPILED).unwrap();
    let mut total = 0;
    for function in chunk.main.flatten() {
        let sites = index.function_for(&function.0).unwrap();
        total += sites.calls.len();
        let lines: Vec<u32> = function
            .0
            .code
            .iter()
            .zip(&function.0.line_info)
            .filter(|(word, _)| call_kind(**word).is_some())
            .map(|(_, line)| *line)
            .collect();
        let expected: Vec<u32> = sites.calls.iter().map(|call| call.line).collect();
        assert_eq!(lines, expected, "lines {}", function.0.line_defined);
    }
    assert_eq!(total, 11);
}

#[test]
fn the_generic_for_iterator_call_comes_after_the_loop_body() {
    let index = FunctionIndex::new(&stripped_tree());
    let run = index.function(7, 23).unwrap();
    let kinds: Vec<SiteKind> = run.calls.iter().map(|call| call.kind).collect();
    assert_eq!(
        kinds.iter().filter(|k| **k == SiteKind::Iterator).count(),
        1
    );
    let at = kinds.iter().position(|k| *k == SiteKind::Iterator).unwrap();
    assert_eq!(
        run.calls[at - 1].line,
        12,
        "the body call precedes the iterator call"
    );
}

#[test]
fn normalized_chunk_gives_each_call_its_statement_number() {
    let index = FunctionIndex::with_normalized(&stripped_tree(), &normalized_tree()).unwrap();
    let run = index.function(7, 23).unwrap();
    assert!(run.calls.iter().all(|call| call.marker.is_some()));
    let numbers: Vec<u32> = run.calls.iter().filter_map(|call| call.marker).collect();
    // ipairs, the loop body, the iterator call at the top of the normalized loop, then the
    // two calls of the `obj:bump(note(total))` statement and `tostring`.
    assert_eq!(numbers, [1, 2, 4, 3, 5, 5, 6]);
}

#[test]
fn a_statement_level_call_maps_to_its_site() {
    let index = FunctionIndex::new(&stripped_tree());
    let chunk = read_chunk(COMPILED).unwrap();
    let run = &chunk.main.flatten()[2].0;
    // `local total = note(a)`: the first call, with a and b live and nothing else.
    let pc = run
        .code
        .iter()
        .position(|w| call_kind(*w).is_some())
        .unwrap();
    let registers: Vec<i32> = (100..100 + i32::from(run.max_stack)).collect();
    let mapped = map_frame(&index, run, pc, &registers, &[7, 8]).unwrap();
    assert_eq!(mapped.shape.ordinal, 0);
    assert_eq!(mapped.shape.kind, CallKind::Call);
    assert_eq!(mapped.site.line, 9);
    assert_eq!(mapped.shape.line, Some(9));
    let names: Vec<&[u8]> = mapped
        .shape
        .locals
        .iter()
        .map(|l| l.name.as_slice())
        .collect();
    assert_eq!(names, [b"a".as_slice(), b"b".as_slice()]);
    assert_eq!(mapped.shape.locals[1].value, 101);
    assert_eq!(mapped.shape.varargs, [7, 8]);
    assert_eq!(mapped.shape.pending_temporaries, 0);
}

#[test]
fn the_tail_call_is_a_call_site() {
    let index = FunctionIndex::new(&stripped_tree());
    let chunk = read_chunk(COMPILED).unwrap();
    let main = &chunk.main.0;
    let pc = main
        .code
        .iter()
        .position(|w| call_kind(*w) == Some(CallKind::TailCall))
        .unwrap();
    let registers = vec![0; usize::from(main.max_stack)];
    let mapped = map_frame(&index, main, pc, &registers, &[]).unwrap();
    assert_eq!(mapped.shape.kind, CallKind::TailCall);
    assert_eq!(mapped.shape.ordinal, 0);
    assert_eq!(mapped.site.line, 25);
}

#[test]
fn calls_with_pending_temporaries_are_refused() {
    let index = FunctionIndex::new(&stripped_tree());
    let chunk = read_chunk(COMPILED).unwrap();
    let run = &chunk.main.flatten()[2].0;
    let registers = vec![0; usize::from(run.max_stack)];
    let refused = (0..run.code.len())
        .filter(|pc| call_kind(run.code[*pc]).is_some())
        .filter(|pc| {
            matches!(
                map_frame(&index, run, *pc, &registers, &[]),
                Err(MapError::Frame(FrameError::PendingTemporaries { .. }))
            )
        })
        .count();
    // `obj:bump(note(total))` calls `note` with `obj`, `bump` and `obj` already in registers.
    assert!(refused >= 1);
}

#[test]
fn describe_frame_reports_temporaries_without_refusing() {
    let chunk = read_chunk(COMPILED).unwrap();
    let run = &chunk.main.flatten()[2].0;
    let registers = vec![0u8; usize::from(run.max_stack)];
    let temporaries: Vec<usize> = (0..run.code.len())
        .filter(|pc| call_kind(run.code[*pc]).is_some())
        .map(|pc| {
            describe_frame(run, pc, &registers, &[])
                .unwrap()
                .pending_temporaries
        })
        .collect();
    assert!(
        temporaries.iter().any(|count| *count > 0),
        "{temporaries:?}"
    );
    assert_eq!(temporaries[0], 0);
}

#[test]
fn bad_frames_name_the_problem() {
    let chunk = read_chunk(COMPILED).unwrap();
    let run = &chunk.main.flatten()[2].0;
    let registers = vec![0u8; usize::from(run.max_stack)];
    let not_call = run
        .code
        .iter()
        .position(|w| call_kind(*w).is_none())
        .unwrap();
    assert_eq!(
        describe_frame(run, not_call, &registers, &[]),
        Err(FrameError::NotACall { pc: not_call })
    );
    assert!(matches!(
        describe_frame(run, run.code.len(), &registers, &[]),
        Err(FrameError::PcOutOfRange { .. })
    ));
    let pc = run
        .code
        .iter()
        .position(|w| call_kind(*w).is_some())
        .unwrap();
    assert!(matches!(
        describe_frame(run, pc, &[0u8; 0], &[]),
        Err(FrameError::MissingRegisters { .. })
    ));
}

#[test]
fn source_that_differs_from_the_save_is_refused() {
    let chunk = read_chunk(COMPILED).unwrap();
    let run = &chunk.main.flatten()[2].0;
    // Without the strip, the source has calls the save never compiled.
    let unstripped = FunctionIndex::new(&parse(ORIGINAL).unwrap());
    assert!(matches!(
        unstripped.function_for(run),
        Err(SiteError::CallCountMismatch {
            compiled: 7,
            in_source: 10
        })
    ));
    let other = FunctionIndex::new(&parse(b"local x = 1").unwrap());
    assert!(matches!(
        other.function_for(run),
        Err(SiteError::UnknownFunction { .. })
    ));
}

#[test]
fn a_changed_call_kind_is_refused() {
    let chunk = read_chunk(COMPILED).unwrap();
    let main = &chunk.main.0;
    let source = parse(b"return (run(1, 2, 3))").unwrap();
    let index = FunctionIndex::new(&source);
    // A parenthesized call compiles to CALL, not TAILCALL, but both are calls: still accepted.
    assert!(index.function_for(main).is_ok());
    let loop_source = parse(b"for _ in f do end").unwrap();
    assert!(matches!(
        FunctionIndex::new(&loop_source).function_for(main),
        Err(SiteError::CallKindMismatch { .. } | SiteError::CallCountMismatch { .. })
    ));
}

#[test]
fn functions_on_the_same_lines_are_ambiguous() {
    let source = parse(b"\nlocal a, b = function() end, function() end\n").unwrap();
    assert!(matches!(
        FunctionIndex::new(&source).function(2, 2),
        Err(SiteError::AmbiguousFunction { count: 2, .. })
    ));
}

#[test]
fn vararg_extras_sit_between_the_fixed_slots_and_the_base() {
    let chunk = read_chunk(COMPILED).unwrap();
    let run = &chunk.main.flatten()[2].0;
    // func at 10, two fixed parameters, three extras: base is 10 + 1 + 5.
    assert_eq!(vararg_slots(run, 10, 16), 13..16);
    assert_eq!(vararg_slots(run, 10, 13), 0..0);
    let note = &chunk.main.flatten()[1].0;
    assert_eq!(vararg_slots(note, 4, 7), 5..7);
    let main = &chunk.main.0;
    assert_eq!(vararg_slots(main, 0, 1), 0..0);
}
