-- Every Lua 5.4 construct the parser and printer must reproduce exactly.
local a, b <const>, c = 1, 2
local t <close> = nil
local s1, s2, s3 = "dq\t\"x\"", 'sq\'\\', [[
long first newline skipped]]
local s4 = [==[ with ]] inside ]==]
local s5 = "\x41\65\u{48}\u{7FFFFFFF}\z
           after z\
newline"
local n1, n2, n3, n4, n5, n6 = 0x10, 0xA.8p1, 1e3, .5, 3., 9223372036854775808
local n7 = 0xffffffffffffffffff

--[==[ block
comment ]==]
local function f(x, y, ...)
  local packed = { ... }
  return x + y * 2 ^ -3 ^ 2, select("#", ...), packed
end

function T(...) return ... end
G = { a = 1, [2] = "two", 3; "four", nested = { f = function(self) return self end } }
function G.nested.method(self, k) return k end
function G.nested:other(k) return self, k end

local x = a + b - c * a / b % c // a
x = (a + b) * c
x = a - (b - c)
x = a .. b .. c
x = (a .. b) .. c
x = a ^ b ^ c
x = (a ^ b) ^ c
x = -a ^ b
x = (-a) ^ b
x = - -a
x = not not a
x = #s1 + ~a
x = a & b | c ~ a << 1 >> 2
x = a < b and b <= c or a > c and c >= b or a == b or a ~= c
x = (f(1, 2))
x = (...)
x = G.nested:other(1)
x = G["nested"].method(G.nested, 3)
x = f "str" .. T { 1, 2 } [1]
x = ("s"):rep(3)
x = ({ 1 })[1]
x = (function() return 1 end)()

;(f)(1, 2)
;("x"):len()
local y = x
;(G).a = 1
G.a, G[1], x = 1, 2, 3

if a then x = 1 elseif b then x = 2 elseif c then x = 3 else x = 4 end
if a then end
while x < 10 do x = x + 1 if x == 5 then break end end
repeat local inner = x x = x - 1 until inner < 3
for i = 1, 10 do x = x + i end
for i = 10, 1, -2 do x = x - i end
for k, v in pairs(G) do x = k end
for _, v in ipairs({ 1, 2 }) do goto continue ::continue:: end
do local scoped = 1 goto out end
::out::
do ; end

local function rec(n) if n <= 0 then return 0 end return n + rec(n - 1) end
return x, rec(3), f(1, 2, 3, 4);
