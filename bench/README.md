# Benchmarks

`make bench` builds `ludo` in release mode and times it with
[hyperfine](https://github.com/sharkdp/hyperfine) (`cargo install hyperfine
--locked`). Each comparison is a Markdown table in `target/bench`, and
`target/bench/results.md` has them all.

| Comparison | What runs |
| --- | --- |
| `parser` | `ludo check` on a 30000 line program that `big.sh` writes, with `--parser=ladder` and `--parser=pratt`. Checking is the same for both, so the difference is the parser. |
| `fib` | `fib(28)`, on the interpreter and on the VM: almost all calls and arithmetic. |
| `lists` | `map`, `filter` and `fold` over 300000 Ints with functions of ludo: natives calling back into ludo. |
| `structs` | 200 particles falling for 500 steps: structs built and read, as a game world is. |
| `pong` | `examples/pong.ludo` played for 3600 frames, a minute of game, with `ludo play --frames=3600` and no window: `update`, the sequences and `draw` every frame. |

Every time includes starting `ludo` and loading the host prelude, which takes
about a millisecond.

## Results

On an AMD Ryzen 5 3600, with Rust 1.99.1, on 2026-11-09:

| | Interpreter or ladder | VM or Pratt | |
| --- | ---: | ---: | --- |
| `parser` | 172.1 ± 3.8 ms | 154.9 ± 4.1 ms | Pratt 10% faster |
| `fib` | 235.9 ± 7.8 ms | 194.0 ± 6.9 ms | VM 18% faster |
| `lists` | 108.8 ± 5.9 ms | 120.7 ± 6.0 ms | VM 11% slower |
| `structs` | 109.2 ± 2.2 ms | 112.7 ± 9.0 ms | about the same |
| `pong` | 173.0 ± 2.4 ms | 164.5 ± 5.2 ms | VM 5% faster |

The VM is ahead where a program mostly calls functions and computes, and falls
behind where natives call back into ludo many times, as `map` does. The likely
reason, not yet profiled, is that each of those calls goes through `Caller`:
it pushes the call onto the stack and enters the VM's loop again, which a
call made by the bytecode does not. Where most of the time goes into building
values, as structs and lists, both backends run about the same.
