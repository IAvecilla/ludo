-- Recursive calls and arithmetic: almost all the time goes to calling.

local function fib(n)
  if n < 2 then return n end
  return fib(n - 1) + fib(n - 2)
end

print(fib(25))
