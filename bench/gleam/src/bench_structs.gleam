// Records built and read at every step, as the world of a game is.

import gleam/float
import gleam/int
import gleam/io
import gleam/list

type Particle {
  Particle(x: Float, y: Float, vx: Float, vy: Float)
}

fn spawn(i: Int) -> Particle {
  let f = int.to_float(i)
  Particle(x: f, y: 0.0, vx: f /. 100.0, vy: 0.0)
}

fn fall(p: Particle) -> Particle {
  let vy = p.vy +. 0.1
  let p = Particle(..p, x: p.x +. p.vx, y: p.y +. vy, vy: vy)
  case p.y >. 100.0 {
    True -> Particle(..p, y: 100.0, vy: 0.0 -. p.vy *. 0.8)
    False -> p
  }
}

fn steps(ps: List(Particle), i: Int) -> List(Particle) {
  case i == 500 {
    True -> ps
    False -> steps(list.map(ps, fall), i + 1)
  }
}

fn range(from: Int, to: Int) -> List(Int) {
  case from >= to {
    True -> []
    False -> [from, ..range(from + 1, to)]
  }
}

pub fn main() {
  range(0, 200)
  |> list.map(spawn)
  |> steps(0)
  |> list.fold(0.0, fn(total, p) { total +. p.y })
  |> float.to_string
  |> io.println
}
