// Float arithmetic: how many points of a 60 by 60 grid are in the Mandelbrot
// set, following each one for at most 50 steps. Gleam has no loops, so the
// steps of a point are a recursion and so is the grid, where point k is in
// row k / 60 and column k % 60.

import gleam/int
import gleam/io

fn steps(zr: Float, zi: Float, cr: Float, ci: Float, n: Int) -> Int {
  case n == 50 || zr *. zr +. zi *. zi >. 4.0 {
    True -> n
    False ->
      steps(zr *. zr -. zi *. zi +. cr, 2.0 *. zr *. zi +. ci, cr, ci, n + 1)
  }
}

fn points(k: Int, count: Int) -> Int {
  case k == 60 * 60 {
    True -> count
    False -> {
      let x = int.to_float(k % 60)
      let y = int.to_float(k / 60)
      let n = steps(0.0, 0.0, x *. 3.0 /. 60.0 -. 2.0, y *. 3.0 /. 60.0 -. 1.5, 0)
      let count = case n == 50 {
        True -> count + 1
        False -> count
      }
      points(k + 1, count)
    }
  }
}

pub fn main() {
  io.println(int.to_string(points(0, 0)))
}
