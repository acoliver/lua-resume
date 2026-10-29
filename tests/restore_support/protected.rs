//! Scripts that yield inside `pcall`, `xpcall` and an alias of `pcall`. Errors use level 0 so
//! messages do not depend on the line a continuation reports.

use crate::restore_support::Script;

pub const SCRIPTS: &[Script] = &[
    Script {
        hooked: false,
        name: "a yield in a function called by pcall",
        source: "local function wait() return coroutine.yield('wait') end
function main()
  local ok, value = pcall(function() return wait() end)
  count = (count or 0) + 1
  trace = tostring(ok) .. ':' .. tostring(value)
  return ok, value
end
",
        args: "",
        inputs: &["'fine'"],
    },
    Script {
        hooked: false,
        name: "a Lua frame between pcall and the yield",
        source: "local function wait(tag)
  local got = coroutine.yield(tag)
  return got
end
function main()
  local ok, value = pcall(function()
    local got = wait('wait')
    return got
  end)
  count = (count or 0) + 1
  trace = tostring(ok) .. ':' .. tostring(value)
  return ok, value
end
",
        args: "",
        inputs: &["'fine'"],
    },
    Script {
        hooked: false,
        name: "an error after the resume is caught by the restored pcall",
        source: "local function risky(tag)
  local got = coroutine.yield(tag)
  if got == 'boom' then error('bad ' .. got, 0) end
  return got
end
function main()
  local ok, err = pcall(risky, 'first')
  local after = coroutine.yield('caught', ok, err)
  count = (count or 0) + 1
  trace = tostring(ok) .. ':' .. tostring(err) .. ':' .. tostring(after)
  return ok, err, after
end
",
        args: "",
        inputs: &["'boom'", "'later'"],
    },
    Script {
        hooked: false,
        name: "pcall in a loop with errors before and after the yield",
        source: "local function step(i)
  if i == 2 then error('early ' .. i, 0) end
  local got = coroutine.yield('step', i)
  if got == 'boom' then error('late ' .. i, 0) end
  return i * 10 + got
end
function main()
  local results = {}
  for i = 1, 4 do
    local ok, value = pcall(step, i)
    results[#results + 1] = tostring(ok) .. ':' .. tostring(value)
  end
  trace = table.concat(results, ',')
  return trace
end
",
        args: "",
        inputs: &["1", "'boom'", "3"],
    },
    Script {
        hooked: false,
        name: "nested pcall",
        source: "local function leaf(n)
  local got = coroutine.yield('leaf', n)
  if got == 'boom' then error('leaf ' .. got, 0) end
  return got
end
local function mid(n)
  local ok, value = pcall(leaf, n)
  local again = coroutine.yield('mid', ok, value)
  if again == 'boom' then error('mid ' .. again, 0) end
  return value, again
end
function main()
  local ok, a, b = pcall(mid, 1)
  local second = function()
    local inner_ok, inner = pcall(leaf, 2)
    local text = tostring(inner)
    error('outer ' .. text, 0)
  end
  local ok2, c = pcall(second)
  trace = tostring(ok) .. tostring(a) .. tostring(b) .. tostring(ok2) .. tostring(c)
  return trace
end
",
        args: "",
        inputs: &["'boom'", "'boom'", "'fine'"],
    },
    Script {
        hooked: false,
        name: "xpcall with a handler whose result matters",
        source: "local function risky(tag)
  local got = coroutine.yield('risky', tag)
  if got == 'boom' then error('bad ' .. got, 0) end
  return got
end
function main()
  local seen = 0
  local handler = function(message)
    seen = seen + 1
    count = (count or 0) + 1
    return 'handled ' .. message .. ' ' .. seen
  end
  local ok, first = xpcall(risky, handler, 'a')
  local ok2, second = xpcall(risky, handler, 'b')
  trace = tostring(ok) .. ':' .. tostring(first) .. ':' .. tostring(ok2) .. ':' .. tostring(second)
  return trace, seen
end
",
        args: "",
        inputs: &["'boom'", "'fine'"],
    },
    Script {
        hooked: false,
        name: "the game's sprocall alias of pcall, with varargs",
        source: "sprocall = pcall
local function work(label, ...)
  local extra = select('#', ...)
  local got = coroutine.yield(label, extra)
  if got == 'boom' then error('failed ' .. label, 0) end
  return label, got, ...
end
function main()
  local ok, a, b, c = sprocall(work, 'one', 'x', 'y')
  local ok2, d = sprocall(work, 'two')
  trace = tostring(ok) .. tostring(a) .. tostring(b) .. tostring(c) .. tostring(ok2) .. tostring(d)
  return trace
end
",
        args: "",
        inputs: &["'boom'", "7"],
    },
];
