//! `/` printed as a call of a host function: the call is not a site. Saving at every yield of
//! scripts that divide, restoring in a fresh state and continuing must equal the uninterrupted
//! run with the host division installed, and site numbering must not change.

mod restore_support;

use lua_resume::rewrite::check_normal_form;
use lua_resume::syntax::{OperatorHooks, parse, print, print_with};
use restore_support::{Script, round_trip_all, run};

#[path = "support/mod.rs"]
mod normalizer;

const EXPRESSIONS: Script = Script {
    name: "division inside expressions, loops and calls, with yields after it",
    source: "local function scale(v, d) return v / d end
function main(n)
  local out = {}
  local q = n / 2 + coroutine.yield('half', n / 4)
  out[#out + 1] = q
  for i = 1, 3 do
    local got = coroutine.yield('loop', (i * 10) / 4, scale(i, 2.0))
    out[#out + 1] = got / 3 + (-7) / 2
  end
  local m = (coroutine.yield('last', 9 / 2)) / 2
  trace = (trace or '') .. tostring(q / 2) .. ';'
  return q, m, #out, out[1], out[2], out[3], 1 / 2
end",
    args: "5",
    inputs: &["10", "9", "12", "7", "8"],
    hooked: true,
};

const ORDER: Script = Script {
    name: "operands with side effects are evaluated once, left then right",
    source: "trace = ''
local function t(tag, v) trace = trace .. tag; return v end
function main()
  local r = t('a', 12) / t('b', 5)
  local s = t('c', 9) / t('d', 2) / t('e', 2)
  local u = coroutine.yield('mid', r, s)
  local w = t('f', 7) / (coroutine.yield('inner') + t('g', 2))
  count = (count or 0) + 1
  return r, s, u, w
end",
    args: "",
    inputs: &["3", "4"],
    hooked: true,
};

#[test]
fn every_yield_after_a_division_restores_in_a_fresh_state() {
    assert_eq!(round_trip_all(&[EXPRESSIONS, ORDER]), 7);
}

#[test]
fn the_host_function_decides_the_result() {
    let (_, _, expressions) = run(&EXPRESSIONS, None);
    assert_eq!(expressions.events[0], "yield:\"half\",1");
    let last = expressions.events.last().unwrap();
    assert!(last.starts_with("return:"), "{last}");
    assert!(last.ends_with(",0"), "1 / 2 divides as integers: {last}");
}

#[test]
fn operands_are_evaluated_once_in_order() {
    let (_, _, order) = run(&ORDER, None);
    assert!(order.state.contains("\"abcdefg\""), "{}", order.state);
    assert_eq!(order.events.last().unwrap(), "return:2,2,3,1");
}

#[test]
fn a_division_adds_no_call_site() {
    let source = ORDER.source;
    let tree = normalizer::normalize_source(source.as_bytes()).unwrap();
    check_normal_form(&tree).unwrap();
    let hooks = OperatorHooks::none().with_divide("__lr_div").unwrap();
    let plain = print(&tree);
    let hooked = print_with(&tree, &hooks);
    let markers = |text: &str| -> Vec<String> {
        text.match_indices("__lrs = ")
            .map(|(at, _)| text[at..].chars().take(10).collect())
            .collect()
    };
    assert_eq!(markers(&plain), markers(&hooked));
    assert!(hooked.contains("(__lr_div("), "{hooked}");
    assert!(!plain.contains("__lr_div"));
    let reparsed = parse(hooked.as_bytes()).unwrap();
    assert_ne!(
        reparsed, tree,
        "the printed source holds calls the tree does not"
    );
}

#[test]
fn without_hooks_the_printed_source_is_unchanged() {
    let tree = normalizer::normalize_source(EXPRESSIONS.source.as_bytes()).unwrap();
    assert_eq!(print_with(&tree, &OperatorHooks::none()), print(&tree));
    let (_, _, plain) = run(
        &Script {
            hooked: false,
            ..EXPRESSIONS
        },
        None,
    );
    assert_eq!(plain.events[0], "yield:\"half\",0x1.4p+0");
}

#[test]
fn a_hook_name_must_be_a_reserved_lua_name() {
    for name in ["div", "", "__lr div", "__lr_div(", "__lr-d"] {
        assert!(OperatorHooks::none().with_divide(name).is_err(), "{name:?}");
    }
    let hooks = OperatorHooks::none().with_divide("__lr_div").unwrap();
    assert_eq!(hooks.divide(), Some("__lr_div"));
    assert_eq!(OperatorHooks::none().divide(), None);
}
