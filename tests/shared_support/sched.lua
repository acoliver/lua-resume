function make_pair()
  local c = 0
  local function inc()
    c = c + 1
    return c
  end
  local function get()
    return c
  end
  return inc, get
end

function worker(name, shared, step, n)
  local mine = 0
  local i = 0
  while i < n do
    i = i + 1
    mine = mine + step()
    shared.log[#shared.log + 1] = name .. i .. ':' .. mine
    shared.total = shared.total + 1
    coroutine.yield(i)
  end
  return mine
end

function reader(shared, get, n)
  local i = 0
  while i < n do
    i = i + 1
    shared.seen[#shared.seen + 1] = get() .. '/' .. shared.total
    coroutine.yield(i)
  end
  return get()
end

function new_world()
  local shared = { log = {}, seen = {}, total = 0 }
  local inc, get = make_pair()
  local function run_a()
    local r = worker('a', shared, inc, 3)
    return r
  end
  local function run_b()
    local r = reader(shared, get, 3)
    return r
  end
  local a = coroutine.create(run_a)
  local b = coroutine.create(run_b)
  return { shared = shared, inc = inc, get = get, a = a, b = b }
end

function poke(world)
  world.inc()
  world.shared.total = world.shared.total + 100
  return ''
end

function report(world)
  local shared = world.shared
  return table.concat(shared.log, ',') .. '|' .. table.concat(shared.seen, ',')
    .. '|' .. shared.total .. '|' .. world.get()
end
