-- Strings built by joining smaller ones, in the shape of fib: every call
-- makes a new string.

local function words(n)
  if n < 2 then return "x" end
  return words(n - 1) .. words(n - 2)
end

print(words(25))
