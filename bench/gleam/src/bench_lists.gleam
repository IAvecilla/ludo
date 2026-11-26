// map, filter and fold over long lists, each calling a function it is given.

import gleam/int
import gleam/io
import gleam/list

/// The Ints from `from` to `to - 1`.
fn range(from: Int, to: Int) -> List(Int) {
  down(to - 1, from, [])
}

fn down(k: Int, from: Int, acc: List(Int)) -> List(Int) {
  case k < from {
    True -> acc
    False -> down(k - 1, from, [k, ..acc])
  }
}

fn square(n: Int) -> Int {
  n * n
}

fn odd(n: Int) -> Bool {
  n % 2 == 1
}

fn add(a: Int, b: Int) -> Int {
  a + b
}

pub fn main() {
  range(0, 300_000)
  |> list.map(square)
  |> list.filter(odd)
  |> list.fold(0, add)
  |> int.to_string
  |> io.println
}
