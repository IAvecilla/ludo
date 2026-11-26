// The loops benchmark of ploxb: the sum of i * j for every i below 1000 and
// j below 100. Gleam has no loops, so one recursion goes over the cells, and
// the cell k is row k / 100 and column k % 100.

import gleam/int
import gleam/io

fn cells(k: Int, total: Int) -> Int {
  case k == 100_000 {
    True -> total
    False -> cells(k + 1, total + { k / 100 } * { k % 100 })
  }
}

pub fn main() {
  io.println(int.to_string(cells(0, 0)))
}
