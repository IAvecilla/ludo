-- Without a window: the benchmark times the game, not the screen.
function love.conf(t)
  t.window = false
  t.modules.window = false
  t.modules.graphics = false
  t.modules.audio = false
  t.modules.sound = false
  t.modules.joystick = false
  t.modules.physics = false
end
