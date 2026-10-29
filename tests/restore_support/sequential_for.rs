//! Scripts for the sequential loop suite: several numeric `for` loops in one function, in
//! sequence, nested, after a generic `for` and inside a `while`, each yielding in its body.

use crate::restore_support::Script;

pub const SCRIPTS: &[Script] = &[
    Script {
        hooked: false,
        name: "two integer loops with the same variable",
        source: "function main()
  local acc = 0
  for i = 1, 3 do
    acc = acc + coroutine.yield('a', i)
  end
  for i = 1, 3 do
    acc = acc + coroutine.yield('b', i)
  end
  count = acc
  return acc
end
",
        args: "",
        inputs: &["1", "2", "3", "4", "5", "6"],
    },
    Script {
        hooked: false,
        name: "three loops mixing integer and float",
        source: "function main()
  local acc = 0
  for i = 1, 2 do
    acc = acc + coroutine.yield('a', i)
  end
  for x = 0.5, 2.0, 0.5 do
    acc = acc + coroutine.yield('b', x)
  end
  for j = 3, 1, -1 do
    acc = acc + coroutine.yield('c', j)
  end
  count = acc
  return acc
end
",
        args: "",
        inputs: &["1", "2", "3", "4", "5", "6", "7", "8", "9"],
    },
    Script {
        hooked: false,
        name: "nested numeric loops followed by another loop",
        source: "function main()
  local acc = 0
  for i = 1, 2 do
    for j = 1, 2 do
      acc = acc + coroutine.yield('in', i, j)
    end
    acc = acc + coroutine.yield('mid', i)
  end
  for k = 1, 2 do
    for m = 1, 2 do
      acc = acc + coroutine.yield('again', k, m)
    end
  end
  count = acc
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
        name: "a numeric loop after a generic loop",
        source: "function main()
  local acc = 0
  for _, v in ipairs({ 10, 20 }) do
    acc = acc + coroutine.yield('gen', v)
  end
  for i = 1, 3 do
    acc = acc + coroutine.yield('num', i)
  end
  count = acc
  return acc
end
",
        args: "",
        inputs: &["1", "2", "3", "4", "5"],
    },
    Script {
        hooked: false,
        name: "numeric loops inside a while loop",
        source: "function main()
  local acc, round = 0, 0
  while round < 2 do
    round = round + 1
    for i = 1, 2 do
      acc = acc + coroutine.yield('first', round, i)
    end
    for i = 1, 2 do
      acc = acc + coroutine.yield('second', round, i)
    end
  end
  count = acc
  return acc
end
",
        args: "",
        inputs: &["1", "2", "3", "4", "5", "6", "7", "8"],
    },
    Script {
        hooked: false,
        name: "loops in a called function with a computed limit",
        source: "local function work(n)
  local acc = 0
  for i = 1, n do
    acc = acc + coroutine.yield('a', i)
  end
  for i = 1, n + 1 do
    acc = acc + coroutine.yield('b', i)
  end
  return acc
end
function main()
  local first = work(2)
  local second = work(1)
  count = first + second
  return first, second
end
",
        args: "",
        inputs: &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"],
    },
    Script {
        hooked: false,
        name: "second loop calls a function that yields, with locals between loops",
        source: "local function ask(tag, v)
  local got = coroutine.yield(tag, v)
  return got
end
function main()
  local acc = 0
  for i = 1, 2 do
    acc = acc + ask('a', i)
  end
  local mid = acc * 2
  local label = 'm'
  for i = 1, 2 do
    local sq = i * i
    acc = acc + ask(label, i) + mid + sq
  end
  for x = 1.5, 2.5 do
    acc = acc + ask('f', x)
  end
  count = acc
  return acc
end
",
        args: "",
        inputs: &["1", "2", "3", "4", "5", "6", "7", "8"],
    },
    Script {
        hooked: false,
        name: "loops with calls in the headers",
        source: "local function lim(n) return n end
function main()
  local acc = 0
  for i = 1, lim(2) do
    acc = acc + coroutine.yield('a', i)
  end
  for i = lim(1), lim(3), lim(1) do
    acc = acc + coroutine.yield('b', i)
  end
  count = acc
  return acc
end
",
        args: "",
        inputs: &["1", "2", "3", "4", "5", "6", "7", "8"],
    },
];
