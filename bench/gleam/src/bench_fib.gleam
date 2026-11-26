// Recursive calls and arithmetic: almost all the time goes to calling.

import gleam/int
import gleam/io

fn fib(n: Int) -> Int {
  case n < 2 {
    True -> n
    False -> fib(n - 1) + fib(n - 2)
  }
}

pub fn main() {
  io.println(int.to_string(fib(25)))
}
