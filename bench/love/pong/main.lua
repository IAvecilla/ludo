-- examples/pong.ludo written for LÖVE, to time against
-- `ludo play --frames=N`: `love bench/love/pong N` runs N frames of 1/60 s
-- with no key down, and each one updates the game, moves the serve on and
-- builds the shapes to draw, without drawing them. It is written as a LÖVE
-- game usually is: the world is a table that changes, and the serve is a
-- coroutine.

local width, height = 640.0, 480.0
local margin = 30.0
local paddle_w, paddle_h = 10.0, 80.0
local player_speed, cpu_speed = 360.0, 200.0
local ball_r = 7.0
local ball_speed, max_speed = 320.0, 700.0

local world
local serving_run

local function centered()
  return { x = width / 2, y = height / 2, vx = 0.0, vy = 0.0 }
end

local function clamp(x, lo, hi)
  return math.max(lo, math.min(hi, x))
end

-- Waits 0.6 s, grows the ball for 0.4 s and sends it off: 36 frames and then
-- 24, as `wait` and `over` take them in ludo, which sends the ball off in the
-- frame of the last pass.
local function serve(dir)
  return coroutine.create(function()
    world.pop = 0.0
    for _ = 1, 36 do coroutine.yield() end
    for pass = 1, 24 do
      world.pop = pass / 24
      if pass < 24 then coroutine.yield() end
    end
    local up = (world.left.score + world.right.score) % 2 == 0
    world.ball.vx = ball_speed * dir
    world.ball.vy = ball_speed * (up and -0.4 or 0.4)
    world.serving = false
  end)
end

local function axis(negative, positive)
  if negative and not positive then return -1.0 end
  if positive and not negative then return 1.0 end
  return 0.0
end

local function move_paddle(y, dy)
  return clamp(y + dy, paddle_h / 2, height - paddle_h / 2)
end

local function walls(b)
  if b.y < ball_r then
    b.y, b.vy = ball_r, math.abs(b.vy)
  elseif b.y > height - ball_r then
    b.y, b.vy = height - ball_r, -math.abs(b.vy)
  end
end

local function hit(b, paddle_y, x, dir)
  local towards = b.vx * dir < 0
  local near_x = math.abs(b.x - x) < ball_r + paddle_w / 2
  local near_y = math.abs(b.y - paddle_y) < ball_r + paddle_h / 2
  if towards and near_x and near_y then
    local offset = (b.y - paddle_y) / (paddle_h / 2)
    b.vx = math.min(math.abs(b.vx) * 1.05, max_speed) * dir
    b.vy = offset * ball_speed
  end
end

local function score()
  if world.ball.x < 0 then
    world.right.score = world.right.score + 1
    world.ball, world.serving = centered(), true
    serving_run = serve(-1.0)
  elseif world.ball.x > width then
    world.left.score = world.left.score + 1
    world.ball, world.serving = centered(), true
    serving_run = serve(1.0)
  end
end

function love.load()
  world = {
    left = { y = height / 2, score = 0 },
    right = { y = height / 2, score = 0 },
    ball = centered(),
    serving = true,
    pop = 0.0,
  }
  serving_run = serve(-1.0)
end

function love.update(dt, keys)
  local dy = axis(keys.w or keys.up, keys.s or keys.down) * player_speed * dt
  world.left.y = move_paddle(world.left.y, dy)
  world.right.y = move_paddle(world.right.y,
    clamp(world.ball.y - world.right.y, -cpu_speed * dt, cpu_speed * dt))
  if not world.serving then
    local b = world.ball
    b.x, b.y = b.x + b.vx * dt, b.y + b.vy * dt
    walls(b)
    hit(b, world.left.y, margin, 1.0)
    hit(b, world.right.y, width - margin, -1.0)
    score()
  end
  if serving_run and coroutine.status(serving_run) ~= "dead" then
    assert(coroutine.resume(serving_run))
  end
end

local function paddle(x, y)
  return { kind = "rect", x = x - paddle_w / 2, y = y - paddle_h / 2,
           w = paddle_w, h = paddle_h, color = "white" }
end

-- What love.draw would paint, as a list of shapes.
local function shapes()
  local list = {}
  for i = 0, math.floor(height / 32) do
    list[#list + 1] = { kind = "rect", x = width / 2 - 2, y = i * 32.0,
                        w = 4.0, h = 16.0, color = { 90, 90, 120 } }
  end
  list[#list + 1] = { kind = "line", x1 = 0.0, y1 = 2.0, x2 = width, y2 = 2.0,
                      thickness = 4.0, color = { 60, 60, 90 } }
  list[#list + 1] = { kind = "line", x1 = 0.0, y1 = height - 2, x2 = width,
                      y2 = height - 2, thickness = 4.0, color = { 60, 60, 90 } }
  list[#list + 1] = paddle(margin, world.left.y)
  list[#list + 1] = paddle(width - margin, world.right.y)
  list[#list + 1] = { kind = "circle", x = world.ball.x, y = world.ball.y,
                      r = ball_r * world.pop, color = "yellow" }
  list[#list + 1] = { kind = "text", x = width / 2 - 70, y = 60.0, size = 48.0,
                      text = tostring(world.left.score), color = "white" }
  list[#list + 1] = { kind = "text", x = width / 2 + 45, y = 60.0, size = 48.0,
                      text = tostring(world.right.score), color = "white" }
  return list
end

function love.run()
  local frames = tonumber(love.arg.parseGameArguments(arg)[1]) or 3600
  local keys = {}
  love.load()
  for _ = 1, frames do
    love.update(1 / 60, keys)
    shapes()
  end
  return function() return 0 end
end
