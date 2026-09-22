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

## Grammar

Expressions, from loosest to tightest. Every binary level is left-associative.

| | Operators |
| --- | --- |
| `or` | `or` |
| `and` | `and` |
| equality | `==` `!=` |
| comparison | `<` `<=` `>` `>=` |
| pipe | `\|>` |
| term | `+` `-` |
| factor | `*` `/` `%` |
| unary | `-` `not` |
| postfix | `f(x)` `a.b` `a.b(x)` |

```
expression     → or ;
or             → and ( "or" and )* ;
and            → equality ( "and" equality )* ;
equality       → comparison ( ( "==" | "!=" ) comparison )* ;
comparison     → pipe ( ( "<" | "<=" | ">" | ">=" ) pipe )* ;
pipe           → term ( "|>" term )* ;
term           → factor ( ( "+" | "-" ) factor )* ;
factor         → unary ( ( "*" | "/" | "%" ) unary )* ;
unary          → ( "-" | "not" ) unary
               | postfix ;
postfix        → primary ( "(" arguments? ")"
                         | "." IDENT ( "(" arguments? ")" )? )* ;
primary        → INT | FLOAT | STRING | SYMBOL | IDENT
               | "true" | "false"
               | "(" expression ")" ;
arguments      → expression ( "," expression )* ;
```

Pipe sits between comparison and term so that `a + b |> f` is `f(a + b)` and
`x |> valid() and y` is `(x |> valid()) and y`.

`|>` and `.` are the same operator — put the receiver first — and both are sugar
for a call, so these three parse to the same tree:

```
physics(p, dt)        p |> physics(dt)        p.physics(dt)
```

A `.` not followed by `(` is field access: `p.pos`.

## Usage

```sh
make                              # list every target
make repl                         # start the REPL
make run FILE=game.ludo           # run a program
make test                         # test suite
make lint
```
