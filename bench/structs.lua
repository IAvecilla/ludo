-- Structs built and read at every step, as the world of a game is: each step
-- makes a new particle, as in ludo, instead of changing the old one.

local function spawn(i)
  return { x = i, y = 0.0, vx = i / 100.0, vy = 0.0 }
end

local function fall(p)
  local vy = p.vy + 0.1
  local q = { x = p.x + p.vx, y = p.y + vy, vx = p.vx, vy = vy }
  if q.y > 100.0 then
    return { x = q.x, y = 100.0, vx = q.vx, vy = -q.vy * 0.8 }
  end
  return q
end

local particles = {}
for i = 0, 199 do particles[#particles + 1] = spawn(i + 0.0) end
for _ = 1, 500 do
  local next = {}
  for i = 1, #particles do next[i] = fall(particles[i]) end
  particles = next
end

local total = 0.0
for i = 1, #particles do total = total + particles[i].y end
print(total)
