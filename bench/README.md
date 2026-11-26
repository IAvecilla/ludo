# Benchmarks

`make bench` builds `ludo` in release mode and times it with
[hyperfine](https://github.com/sharkdp/hyperfine) (`cargo install hyperfine
--locked`), in three parts. Each comparison is a Markdown table in
`target/bench`, and `target/bench/results.md` has them all.

1. **Common problems**: programs that can be written in ludo and in other
   languages, on ludo, on three implementations of Lox, on Lua and LuaJIT, and
   on Gleam.
2. **Pong**: a minute of game, on ludo and on LÖVE.
3. **Lists and structs**: on ludo, Lua, LuaJIT and Gleam.

The Lox implementations are fetched and built by `bench/setup.sh`, each at a
fixed commit, into `target/bench/lox`. It needs `git`, `make`, a C compiler
and [uv](https://docs.astral.sh/uv/). It also compiles the Gleam programs.
Lua, LuaJIT, LÖVE, Gleam and Erlang have to be installed.

| | What it is | How it is built |
| --- | --- | --- |
| clox | The bytecode VM of [Crafting Interpreters](https://github.com/munificent/craftinginterpreters), in C. | As `make clox` does: `-O3 -flto`, with NaN boxing and no tracing. |
| plox | The course's [tree-walking interpreter](https://github.com/FdelMazo/plox), in Python. | `uv sync` from its lockfile, which picks Python 3.12. |
| ploxb | The course's [bytecode VM](https://github.com/FdelMazo/ploxb), in Python, from the branch `fns-and-closures`, which has functions. | The same. |
| lua | [Lua](https://www.lua.org) 5.4, a bytecode VM in C. | As installed. |
| luajit | [LuaJIT](https://luajit.org), Lua with a JIT compiler. | As installed. |
| gleam | [Gleam](https://gleam.run), compiled to Erlang and run on the BEAM. | `gleam build`, run with `erl`. |
| love | [LÖVE](https://love2d.org), a game engine that runs Lua on LuaJIT. | As installed. |

jlox, the book's tree-walking interpreter in Java, is left out, since it needs
a JDK.

The Python implementations take seconds on each problem, so `make bench` takes
a few minutes, most of them in plox and ploxb.

## 1. Common problems

Each problem is in `bench/common` once in ludo, Lox and Lua, and in
`bench/gleam` in Gleam, and all print the same result. `fib` and `strings` are
the same algorithm in all of them. In `loops` and `mandelbrot` each language
loops its own way: Lox with `while`, Lua with `for` and `while`, Gleam, which
has no loops, with recursion, and ludo, which has none either, with `range` and
`fold`, natives that call back into ludo, and with recursion. The arithmetic of
each step is the same.

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

## 2. Pong

`examples/pong.ludo` played for 3600 frames, a minute of game, with
`ludo play --frames=3600` and no window: `update`, the sequences and `draw`
every frame. `bench/love/pong` is the same game for LÖVE, written as a LÖVE
game usually is, with a world that changes and the serve as a coroutine.
`love bench/love/pong 3600` runs it without a window: each frame updates the
game, moves the serve on and builds the shapes to draw. After 3600 frames both
are in the same state.

## 3. Lists and structs

| Program | What runs |
| --- | --- |
| `lists` | `map`, `filter` and `fold` over 300000 Ints with functions of ludo: natives calling back into ludo. In Lua they are written in Lua, and Gleam has them in its standard library. |
| `structs` | 200 particles falling for 500 steps: structs built and read, as a game world is. Each step builds a new particle in every language, a table in Lua and a record in Gleam. |

## Results

On an Apple M3 Max with macOS 26.7, with Rust 1.100, Apple clang 21, Python
3.12, Lua 5.4.9, LuaJIT 2.1, Gleam 1.19 on Erlang/OTP 28 and LÖVE 11.5, on
2026-11-26.

### 1. Common problems

| | ludo | clox | plox | ploxb | lua | luajit | gleam |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `fib` | 29.2 ± 1.8 ms | 7.9 ± 0.3 ms | 2.87 ± 0.03 s | 1.92 ± 0.12 s | 4.4 ± 0.2 ms | 2.1 ± 0.3 ms | 98.8 ± 1.7 ms |
| `loops` | 13.8 ± 0.8 ms | 4.3 ± 0.2 ms | 1.60 ± 0.02 s | 1.02 ± 0.01 s | 2.6 ± 0.1 ms | 1.6 ± 0.1 ms | 98.1 ± 1.2 ms |
| `mandelbrot` | 17.5 ± 0.8 ms | 4.4 ± 0.2 ms | 2.02 ± 0.01 s | 1.42 ± 0.02 s | 3.1 ± 0.1 ms | 1.9 ± 0.1 ms | 99.4 ± 3.4 ms |
| `strings` | 31.6 ± 0.4 ms | 13.1 ± 0.3 ms | 2.86 ± 0.00 s | 1.90 ± 0.15 s | 6.9 ± 0.2 ms | 3.5 ± 0.1 ms | 105.3 ± 2.6 ms |

clox is 2.4 to 4 times faster than ludo, Lua 4.6 to 6.7 times and LuaJIT 8.5
to 14 times, and ludo is 60 to 116 times faster than plox and ploxb. Gleam's times
are almost all the start of the BEAM: `erl` takes about 106 ms to start and
stop doing nothing, so these problems are too short to time Gleam itself.

### 2. Pong

| | ludo | love |
| --- | ---: | ---: |
| 3600 frames | 58.8 ± 0.8 ms | 131.8 ± 4.7 ms |

Over a minute of game ludo takes less, but only because it starts sooner:
ludo starts in about 5 ms and LÖVE in about 135 ms, as long as a minute of
game takes it. Each frame takes ludo about 15 µs and LÖVE about 3.3 µs, 4.5
times less: 36000 frames take ludo 0.54 s, and 360000 take LÖVE 1.33 s. Both
are far below the 16.7 ms that a frame has at 60 frames per second.

### 3. Lists and structs

| | ludo | lua | luajit | gleam |
| --- | ---: | ---: | ---: | ---: |
| `lists` | 60.0 ± 1.1 ms | 17.0 ± 0.4 ms | 7.0 ± 0.2 ms | 110.7 ± 1.4 ms |
| `structs` | 42.4 ± 0.9 ms | 22.8 ± 0.8 ms | 9.1 ± 0.1 ms | 101.3 ± 1.5 ms |

In `lists`, natives call back into ludo many times, as `map` does. Each of
those calls pushes the call onto the stack and enters the VM's loop again,
which a call made by the bytecode does not.
