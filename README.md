# ludo

A small statically typed language for writing games.

## Types

| | |
| --- | --- |
| `Int` | 64-bit integer |
| `Float` | 64-bit float |
| `Bool` | `true` / `false` |
| `String` | text, no escapes |
| `Symbol` | an interned name: `:jump` |
| `Vec2` | a 2D vector |
| `List[T]` | immutable list |
| `Option[T]` | `Some(x)` / `None` |
| `struct` | named record with typed fields |
| `enum` | sum type, variants may carry values |

A number written against an `s` is a duration: `0.2s`, `2s`. It is the only
numeric suffix, and with a space it is a separate identifier.

## Usage

```sh
make                              # list every target
make repl                         # start the REPL
make run FILE=game.ludo           # run a program
make test                         # test suite
make lint
```
