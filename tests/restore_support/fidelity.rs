//! Scripts for the frame fidelity suite: anonymous functions in every position, numeric
//! `for` loops that yield, and locals that closures share with their creating frame.

use crate::restore_support::Script;

pub const SCRIPTS: &[Script] = &[
    Script {
        hooked: false,
        name: "anonymous functions as arguments, fields, results and methods",
        source: "local threads = {}
local function CreateThread(f)
  local co = coroutine.create(f)
  threads[#threads + 1] = co
  return co
end
local obj = { n = 1 }
function obj:bump(d)
  local got = coroutine.yield('bump', self.n)
  self.n = self.n + d + got
  return self.n
end
local a = { b = { c = {} } }
function a.b.c.run(x)
  local y = coroutine.yield('run', x)
  return x + y
end
local function maker()
  return function(v)
    local w = coroutine.yield('made', v)
    return v * w
  end
end
function main()
  local co = CreateThread(function()
    local r = coroutine.yield('thread')
    trace = 'thread ' .. tostring(r)
    return r
  end)
  coroutine.resume(co)
  local t = { handler = function(z) local q = coroutine.yield('field', z) return q end }
  local f1 = t.handler(5)
  local m = obj:bump(2)
  local r = a.b.c.run(3)
  local made = maker()(4)
  local ok, back = coroutine.resume(co, 'fin')
  count = f1 + m + r + made
  return f1, m, r, made, ok, back
end
",
        args: "",
        inputs: &["10", "20", "30", "40"],
    },
    Script {
        hooked: false,
        name: "numeric for loops that yield",
        source: "function main()
  local acc = {}
  for i = 1, 4 do
    local got = coroutine.yield('int', i)
    acc[#acc + 1] = i * got
  end
  for x = 1.0, 2.0, 0.5 do
    local got = coroutine.yield('float', x)
    acc[#acc + 1] = x + got
  end
  for n = 6, 0, -3 do
    local got = coroutine.yield('neg', n)
    acc[#acc + 1] = n - got
  end
  for k = 1, 3 do
    k = k * 2
    local got = coroutine.yield('mod', k)
    acc[#acc + 1] = k + got
  end
  count = #acc
  return acc
end
",
        args: "",
        inputs: &[
            "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13",
        ],
    },
    Script {
        hooked: false,
        name: "closures that share locals with their creating frame",
        source: "function main()
  local n = 0
  local function inc() n = n + 1 return n end
  local function get() return n end
  inc()
  local got = coroutine.yield('one', n)
  inc()
  local seen = get()
  local after = coroutine.yield('two', seen)
  inc()
  local fns = {}
  for i = 1, 3 do
    local cap = i * 10
    fns[#fns + 1] = function() cap = cap + 1 return cap + i end
    local y = coroutine.yield('loop', cap)
    fns[#fns]()
  end
  local results = {}
  for j = 1, #fns do results[j] = fns[j]() end
  trace = n .. ':' .. table.concat(results, ',')
  count = n
  return n, get(), results, got, after
end
",
        args: "",
        inputs: &["1", "2", "3", "4", "5"],
    },
];
