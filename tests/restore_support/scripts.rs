//! Normalized-script fixtures for the round-trip suite. Errors use level 0 so messages do not
//! depend on the line a continuation reports.

use super::Script;

pub const SCRIPTS: &[Script] = &[
    Script {
        name: "nested calls, loops, goto continue and varargs",
        source: "local function leaf(a, b)
  local got = coroutine.yield('leaf', a)
  return got + b, a
end
local function mid(x, ...)
  local count = select('#', ...)
  local s, t = leaf(x, count)
  local u = coroutine.yield('mid', s)
  return s + u, t, ...
end
function main(n)
  local acc = {}
  local sum = 0
  for k = 1, 3 do sum = sum + k end
  for i, v in ipairs({ n, n + 1 }) do
    local p, q, r = mid(v, 'x', 'y')
    acc[#acc + 1] = p
    trace = (trace or '') .. tostring(q) .. tostring(r) .. ';'
  end
  local i = 0
  while i < 3 do
    i = i + 1
    if i % 2 == 0 then goto continue end
    acc[#acc + 1] = coroutine.yield('while', i)
    ::continue::
  end
  repeat
    i = i - 1
    local z = coroutine.yield('repeat', i)
  until z == 0 or i < 0
  if sum == 6 then
    count = (count or 0) + 1
  else
    count = -1
  end
  return acc, sum
end
",
        args: "4",
        inputs: &["10", "20", "30", "40", "50", "60", "70", "80", "90", "100"],
    },
    Script {
        name: "pcall and xpcall errors after the restore, and tail calls",
        source: "local function risky(v)
  if v == 'boom' then error('bad ' .. v, 0) end
  return v
end
local function tail(n)
  if n == 0 then return coroutine.yield('tail') end
  return tail(n - 1)
end
function main()
  local first = coroutine.yield('start')
  local ok, err = pcall(risky, first)
  local second = coroutine.yield(ok, err)
  local handler = function(m) return 'handled ' .. m end
  local ok2, err2 = xpcall(risky, handler, second)
  local third = coroutine.yield(ok2, err2)
  local t = tail(2)
  count = (count or 0) + 1
  trace = tostring(third) .. tostring(t)
  error({ code = 7, third = third, t = t })
end
",
        args: "",
        inputs: &["'boom'", "'boom'", "'last'", "'tailed'"],
    },
    Script {
        name: "closures sharing upvalues, cyclic tables and metatables",
        source: "local function counter()
  local n = 0
  local function inc(d)
    n = n + d
    coroutine.yield('inc', n)
    return n
  end
  local function get() return n end
  return inc, get
end
function main()
  local inc, get = counter()
  local node = { name = 'a' }
  node.self = node
  node.list = { node, node }
  local function index(t, k) return k .. '!' end
  local function name() return 'obj' end
  local mt = { __index = index, __tostring = name }
  setmetatable(node, mt)
  inc(2)
  node.seen = get()
  local r = inc(3)
  trace = tostring(node) .. node.missing
  return get(), node.self == node, node.list[1] == node.list[2], r, node
end
",
        args: "",
        inputs: &["1", "2"],
    },
    Script {
        name: "a coroutine holding another coroutine",
        source: "function main()
  local function body(a)
    local b = coroutine.yield('inner1', a)
    local c = coroutine.yield('inner2', b)
    return 'inner done', c
  end
  local inner = coroutine.create(body)
  local ok, v = coroutine.resume(inner, 1)
  local x = coroutine.yield('outer1', v)
  local ok2, w = coroutine.resume(inner, x)
  local y = coroutine.yield('outer2', w)
  local ok3, a, b = coroutine.resume(inner, y)
  return ok3, a, b, coroutine.status(inner)
end
",
        args: "",
        inputs: &["'x'", "'y'", "'z'"],
    },
    Script {
        name: "varargs, multiple results and return calls",
        source: "local function pack2(...) return select('#', ...), ... end
local function va(a, ...)
  local n = coroutine.yield('va', a, ...)
  return pack2(n, ...)
end
function main(...)
  local r1, r2, r3 = va(...)
  local m = coroutine.yield('mid', r1, r2, r3)
  return select('#', ...), m, r1, r2, r3
end
",
        args: "1, nil, 3, nil",
        inputs: &["'n'", "'m'"],
    },
];
