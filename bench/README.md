# Benchmarks

`make bench` builds `ludo` in release mode and times it with
[hyperfine](https://github.com/sharkdp/hyperfine) (`cargo install hyperfine
--locked`), in four parts. Each comparison is a Markdown table in
`target/bench`, and `target/bench/results.md` has them all.

1. **Common problems**: programs that can be written both in ludo and in Lox,
   on ludo and on three implementations of Lox.
2. **Parser**: the Pratt parser against the precedence ladder.
3. **Pong**: a minute of game.
4. **Lists and structs**: problems of ludo alone.

The Lox implementations are fetched and built by `bench/setup.sh`, each at a
fixed commit, into `target/bench/lox`. It needs `git`, `make`, a C compiler
and [uv](https://docs.astral.sh/uv/).

| | What it is | How it is built |
| --- | --- | --- |
| clox | The bytecode VM of [Crafting Interpreters](https://github.com/munificent/craftinginterpreters), in C. | As `make clox` does: `-O3 -flto`, with NaN boxing and no tracing. |
| plox | The course's [tree-walking interpreter](https://github.com/FdelMazo/plox), in Python. | `uv sync` from its lockfile, which picks Python 3.12. |
| ploxb | The course's [bytecode VM](https://github.com/FdelMazo/ploxb), in Python, from the branch `fns-and-closures`, which has functions. | The same. |

jlox, the book's tree-walking interpreter in Java, is left out, since it needs
a JDK.

The Python implementations take seconds on each problem, so `make bench` takes
about three minutes, two of them in plox and ploxb.

## 1. Common problems

Each problem is in `bench/common`, once in ludo and once in Lox, and all
print the same result. `fib` and `strings` are the same algorithm in both. In
`loops` and `mandelbrot` each language loops its own way: Lox with `while`,
and ludo, which has no loops, with `range` and `fold`, natives that call back
into ludo, and with recursion. The arithmetic of each step is the same.

| Problem | What runs |
| --- | --- |
| `fib` | `fib(25)`, recursive: almost all calls and arithmetic. |
| `loops` | The loops benchmark of ploxb: the sum of `i * j` for every `i` below 1000 and `j` below 100. |
| `mandelbrot` | How many points of a 60 by 60 grid are in the Mandelbrot set, following each one for at most 50 steps: Float arithmetic. |
| `strings` | `fib(25)` with strings: each call joins the strings of the two before it, so every call makes a new one. |

Calls nest only a few levels deep, because clox allows at most 64 and ludo
200. Every time includes starting the program: about a millisecond for ludo
and clox, and about 0.1 s for plox and ploxb, which import the libraries of
their REPL.

## 2. Parser

`ludo run` on a 30000 line program that `big.sh` writes, with
`--parser=ladder` and `--parser=pratt`. The program is lines
of expressions that go through every level of precedence, so in each pair only
the parser changes.

## 3. Pong

`examples/pong.ludo` played for 3600 frames, a minute of game, with
`ludo play --frames=3600` and no window: `update`, the sequences and `draw`
every frame.

## 4. Lists and structs

| Program | What runs |
| --- | --- |
| `lists` | `map`, `filter` and `fold` over 300000 Ints with functions of ludo: natives calling back into ludo. |
| `structs` | 200 particles falling for 500 steps: structs built and read, as a game world is. |

## Results

On an AMD Ryzen 5 3600, with Rust 1.99.1, GCC 16.2.1 and Python 3.12.16, on
2026-11-09. They were taken when ludo still had a tree-walking interpreter
next to the VM, before the code was simplified; they are the VM's times.

### 1. Common problems

| | ludo | clox | plox | ploxb |
| --- | ---: | ---: | ---: | ---: |
| `fib` | 48.9 ± 4.6 ms | 10.9 ± 1.7 ms | 5.42 ± 0.01 s | 3.32 ± 0.01 s |
| `loops` | 24.3 ± 3.5 ms | 5.6 ± 1.4 ms | 2.92 ± 0.02 s | 2.03 ± 0.02 s |
| `mandelbrot` | 33.8 ± 3.7 ms | 6.2 ± 1.7 ms | 3.76 ± 0.02 s | 2.48 ± 0.02 s |
| `strings` | 54.0 ± 4.5 ms | 16.8 ± 2.9 ms | 5.42 ± 0.02 s | 3.36 ± 0.02 s |

clox is 3 to 5 times faster than ludo, and ludo is 60 to 120 times faster
than plox and ploxb. Most of that is the language each one is written in: C,
Rust or Python. Between a VM and a tree-walking interpreter in the same
language the gap is much smaller: ploxb is 1.4 to 1.6 times faster than plox. That depends on the
version of Python: on Python 3.14, plox runs `fib` in about 4.0 s and ploxb
in about 3.8 s.

### 2. Parser

| Ladder | Pratt | |
| ---: | ---: | --- |
| 216.4 ± 3.4 ms | 202.7 ± 8.4 ms | Pratt 6% faster |

### 3 and 4. Pong, lists and structs

| | Time |
| --- | ---: |
| `pong` | 166.7 ± 5.9 ms |
| `lists` | 122.2 ± 5.7 ms |
| `structs` | 113.1 ± 5.9 ms |

In `lists`, natives call back into ludo many times, as `map` does. Each of
those calls pushes the call onto the stack and enters the VM's loop again,
which a call made by the bytecode does not.
