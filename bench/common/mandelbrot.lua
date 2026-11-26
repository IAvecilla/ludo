-- Float arithmetic: how many points of a 60 by 60 grid are in the Mandelbrot
-- set, following each one for at most 50 steps.

local function steps(cr, ci)
  local zr, zi, n = 0.0, 0.0, 0
  while n < 50 and zr * zr + zi * zi <= 4.0 do
    zr, zi = zr * zr - zi * zi + cr, 2.0 * zr * zi + ci
    n = n + 1
  end
  return n
end

local function inside(size)
  local count = 0
  for y = 0, size - 1 do
    for x = 0, size - 1 do
      if steps(x * 3.0 / size - 2.0, y * 3.0 / size - 1.5) == 50 then
        count = count + 1
      end
    end
  end
  return count
end

print(inside(60))
