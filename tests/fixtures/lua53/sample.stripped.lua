local log = {}
local function note(...)
  log[#log + 1] = select('#', ...)
  return ...
end

function run(a, b, ...)
                            
  local total = note(a)
                     
  for i, v in ipairs({ b, ... }) do
    total = total + note(v, i)
                     
  end
  local obj = { n = 1 }
  function obj:bump(by)
                  
    self.n = self.n + note(by)
    return note(self.n)
  end
  local r = obj:bump(note(total))
  return tostring(r)
end

return run(1, 2, 3)
