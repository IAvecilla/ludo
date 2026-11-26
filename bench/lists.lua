-- map, filter and fold over long lists, each calling a function it is given.

local function range(from, to)
  local xs = {}
  for i = from, to - 1 do xs[#xs + 1] = i end
  return xs
end

local function map(xs, f)
  local out = {}
  for i = 1, #xs do out[i] = f(xs[i]) end
  return out
end

local function filter(xs, keep)
  local out = {}
  for i = 1, #xs do
    if keep(xs[i]) then out[#out + 1] = xs[i] end
  end
  return out
end

local function fold(xs, acc, f)
  for i = 1, #xs do acc = f(acc, xs[i]) end
  return acc
end

local function square(n) return n * n end
local function odd(n) return n % 2 == 1 end
local function add(a, b) return a + b end

print(fold(filter(map(range(0, 300000), square), odd), 0, add))
