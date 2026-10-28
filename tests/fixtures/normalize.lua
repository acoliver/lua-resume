local concat, pack, tostring = table.concat, table.pack, tostring
local trace, results = {}, {}
local function mark(tag, ...)
  trace[#trace + 1] = tag
  return ...
end
local function record(...)
  local args = pack(...)
  for i = 1, args.n do results[#results + 1] = tostring(args[i]) end
end
local function many() return mark('many', 4, nil, 6) end
local function args(...) return ... end

record(args(mark('first', 1), mark('second', 2), many()))
record((many()))
local x, y, z = many()
record(x, y, z)
x, y, z = mark('a', 2), many()
record(x, y, z)
local t = {mark('item1', 1), mark('item2', 2), many()}
record(t[1], t[2], t[3], t[4], t[5])
t = {mark('item', 1), named = mark('named', 2), [mark('key', 'k')] = mark('value', 3), many()}
record(t[1], t[2], t[3], t[4], t.named, t.k)
t = {[1] = 2, mark('overwritten', 3)}
record(t[1])
t = {mark('pending', 3), [1] = mark('early-store', 2)}
record(t[1])

record(false and mark('unreachable', 1))
record(true or mark('unreachable', 2))
record(mark('left', false) or mark('right', 7) + 1)
record(mark('left', true) and (mark('middle', false) or mark('last', 8)))
record(not mark('unary', false), -mark('negative', 3))

local a = 1
local function mutate() a = 10; return mark('mutate', 2) end
record(a + mutate(), a)
a = 1
record(a .. mutate(), a)
a = 1
local function upvalue() return a + mutate() end
record(upvalue())
a = 1
record((function() return a .. mutate() end)())

local object = {x = 1}
local function change() object = {x = 10}; return mark('change', 'x') end
record(object[change()])
object = {x = 1}
record((function() return object[change()] end)())
local function number_change() object = {x = 10}; return mark('number-change', 2) end
object = {x = 1}
record(object.x + number_change())

local old = {}
object = old
local key = 'x'
local function assign_change() object = {}; key = 'y'; return mark('assign-change', 9) end
object[key] = assign_change()
record(old.x, old.y, object.x, object.y)
object = old
key = 'x';
(function() object[key] = assign_change() end)()
record(old.x, old.y, object.x, object.y)
local array, i = {}, 1
i, array[i] = 2, mark('conflict', 12)
record(i, array[1], array[2])
local function table_target() return mark('target-table', array) end
local function key_target() return mark('target-key', 3) end
table_target()[key_target()], array[mark('second-key', 4)] = mark('rhs', 15), many()
record(array[3], array[4])

local methods = {}
local function first(self, value) return mark('first-method', value) end
local function second(self, value) return mark('second-method', value) end
methods.method = first
local function replace() methods.method = second; return mark('replace', 3) end
record(methods:method(replace() + 1))
methods.method = first
record(methods:method(replace()))

local n = 0
while mark('while', n < 2) do n = n + 1 end
repeat
  local visible = n
  n = n + 1
until mark('repeat', visible >= 3)
record(n)
if mark('if', false) then
  record('wrong')
elseif mark('elseif1', false) then
  record('wrong')
elseif mark('elseif2', true) then
  record('branch')
else
  record('wrong')
end
for j = mark('start', 1), mark('limit', 3), mark('step', 1) do
  record(j)
end
local closures = {}
for j = 1, 3 do closures[j] = function() return j end end
record(closures[1](), closures[2](), closures[3]())
local function iterator(state, control)
  local next = control + 1
  if next > 2 then return nil end
  return mark('iterator', next, next * 2)
end
for j, value in mark('header', iterator, {}, 0) do record(j, value) end
local finalizer = setmetatable({}, {__close = function() mark('closed') end})
local function header() return iterator, {}, 0, finalizer end
for j in header() do break end
for j in header() do record(j) end

local count = 0
::again::
count = count + 1
record(mark('backward', count))
if count < 2 then goto again end
do
  goto done
  local skipped = mark('unreachable', 1)
  ::done::
end
repeat
  local skipped = 0
  ::skip::
  skipped = skipped + 1
  if skipped < 2 then goto skip end
until true

return concat(results, ',') .. '|' .. concat(trace, ',')
