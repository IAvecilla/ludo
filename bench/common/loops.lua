-- The loops benchmark of ploxb: the sum of i * j for every i below 1000 and
-- j below 100, with two loops.

local total = 0
for i = 0, 999 do
  for j = 0, 99 do
    total = total + i * j
  end
end

print(total)
