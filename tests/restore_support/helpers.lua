function __show(v, seen)
  seen = seen or { n = 0 }
  local kind = type(v)
  if kind == "number" then
    if math.type(v) == "integer" then return string.format("%d", v) end
    return string.format("%a", v)
  elseif kind == "string" then
    return string.format("%q", v)
  elseif kind == "table" then
    if seen[v] then return "@" .. seen[v] end
    seen.n = seen.n + 1
    seen[v] = seen.n
    local keys = {}
    for k in pairs(v) do keys[#keys + 1] = k end
    table.sort(keys, function(a, b) return __show(a) < __show(b) end)
    local out = {}
    for _, k in ipairs(keys) do
      out[#out + 1] = __show(k, seen) .. "=" .. __show(rawget(v, k), seen)
    end
    local meta = getmetatable(v)
    local tail = meta and ("<" .. __show(meta, seen) .. ">") or ""
    return "#" .. seen[v] .. "{" .. table.concat(out, ",") .. "}" .. tail
  elseif kind == "function" or kind == "thread" then
    return kind
  end
  return tostring(v)
end

function __step(co, ...)
  local r = table.pack(coroutine.resume(co, ...))
  local tag = "error"
  if r[1] then tag = coroutine.status(co) == "dead" and "return" or "yield" end
  local out = {}
  for i = 2, r.n do out[#out + 1] = __show(r[i]) end
  return tag .. ":" .. table.concat(out, ",")
end

function __args(text)
  return load("return {" .. text .. "}")()
end

function __state()
  return __show({ trace = trace, count = count })
end

function __dump_globals()
  local out = {}
  for _, name in ipairs({ "trace", "count" }) do
    local v = _G[name]
    if v ~= nil then out[#out + 1] = name .. "=" .. string.format("%q", v) end
  end
  return table.concat(out, ";")
end
