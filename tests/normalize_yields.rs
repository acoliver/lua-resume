//! Normalized code keeps coroutine suspension order and the values passed through calls.

mod support;

use support::normalize_source;

use lua_resume::{rewrite::check_normal_form, syntax::print};
use mlua::{Lua, ThreadStatus};

fn suspensions(source: &str) -> Vec<String> {
    let lua = Lua::new();
    let function = lua.load(source).into_function().unwrap();
    let thread = lua.create_thread(function).unwrap();
    let mut values = Vec::new();
    while thread.status() == ThreadStatus::Resumable {
        assert!(values.len() < 100, "unexpected loop");
        values.push(thread.resume::<String>(()).unwrap());
    }
    values
}

#[test]
fn yield_order_and_resume_results_are_unchanged() {
    let source = r"
local function pause(n)
  coroutine.yield(tostring(n))
  return n, n + 10
end
local a = pause(1) + pause(2)
local b = {pause(3), pause(4)}
local c = pause(5) and (false or pause(6))
local i = 0
while pause(i) < 2 do i = i + 1 end
repeat i = i - 1 until pause(i) == 0
if pause(0) > 1 then a = 0 elseif pause(1) > 0 then a = a + 1 end
for j = pause(1), pause(2) do a = a + j end
local function iter(_, n)
  if n >= 2 then return nil end
  return pause(n + 1)
end
for j, value in iter, nil, 0 do a = a + value end
return table.concat({a, b[1], b[2], b[3], c}, ',')
";
    let tree = normalize_source(source.as_bytes()).unwrap();
    check_normal_form(&tree).unwrap();
    let actual = suspensions(&print(&tree));
    assert_eq!(actual, suspensions(source));
    assert_eq!(actual.last().unwrap(), "30,3,4,14,6");
}
