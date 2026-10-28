local named = { [next] = "next", [(ipairs({}))] = "(ipairs({}))" }

local function ser(v)
  local kind = type(v)
  if kind == "number" then
    if math.type(v) == "integer" then return string.format("%d", v) end
    return string.format("%a", v)
  elseif kind == "string" then
    return string.format("%q", v)
  elseif kind == "table" then
    local parts = {}
    local length = #v
    for i = 1, length do parts[#parts + 1] = ser(v[i]) end
    for k, x in pairs(v) do
      local inside = math.type(k) == "integer" and k >= 1 and k <= length
      if not inside then parts[#parts + 1] = "[" .. ser(k) .. "]=" .. ser(x) end
    end
    return "{" .. table.concat(parts, ",") .. "}"
  elseif kind == "function" then
    if named[v] then return named[v] end
    for name, value in pairs(_G) do
      if value == v then return string.format("_G[%q]", name) end
    end
    error("unserializable function")
  end
  return tostring(v)
end

local function show(...)
  local t = table.pack(...)
  local out = {}
  for i = 1, t.n do out[i] = tostring(t[i]) end
  return table.concat(out, ",")
end

function __globals_text()
  local g = {}
  for _, name in ipairs(__globals) do g[name] = _G[name] end
  return ser(g)
end

function __snap(site, locals, varargs)
  __last = { site, ser(locals), ser(varargs), __globals_text() }
end

function __restore(text)
  for name, value in pairs(load("return " .. text)()) do _G[name] = value end
end

function __load(text) return load("return " .. text)() end

function __start(continuation, values, varargs)
  local next = function(...) return coroutine.yield(...) end
  return coroutine.create(function()
    return continuation(values, next, table.unpack, table.unpack(varargs, 1, varargs.n))
  end)
end

-- Returns the log of every resume, and the snapshot taken before each suspension.
function __drive(co, args, inputs)
  local log, snaps = {}, {}
  local call, i = args, 0
  repeat
    local r = table.pack(coroutine.resume(co, table.unpack(call, 1, call.n or #call)))
    log[#log + 1] = show(table.unpack(r, 1, r.n))
    snaps[#snaps + 1] = __last
    i = i + 1
    assert(i < 200, "runaway coroutine")
    call = inputs[i] or {}
  until coroutine.status(co) == "dead"
  return log, snaps
end
