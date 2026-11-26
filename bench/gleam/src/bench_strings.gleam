// Strings built by joining smaller ones, in the shape of fib: every call
// makes a new string.

import gleam/io

fn words(n: Int) -> String {
  case n < 2 {
    True -> "x"
    False -> words(n - 1) <> words(n - 2)
  }
}

pub fn main() {
  io.println(words(25))
}
