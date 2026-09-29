# ludo

A small statically typed language for writing small games, inspired by Elixir,
Gleam, Jai and PICO-8.

## Core

- **No mutation.** A struct is changed by building a new one, and a game is a function from one world to the next.
- **Time as a construct.** A `sequence` is an action that spans frames, with `wait` and `over` instead of timers and callbacks.
- **Games with hot reload.** `ludo play` runs a game in a window and reloads it when the file is saved, keeping the game where it was.
- **Functional oriented** Everything is an expression, there are no loops or `return`, and calls chain with pipes like `range(0, 10) |> filter(even) |> map(double)`.

## A game in a few lines

This is `examples/square.ludo`, a yellow square that moves with the left and right
arrow keys.

```
World :: struct { x: Float, y: Float }

init :: () -> World { World { x: 320.0, y: 240.0 } }

update :: (w: World, input: Input, dt: Float) -> World {
    let speed = 200.0 * dt
    let x = if held(input, :left) { w.x - speed } else if held(input, :right) { w.x + speed } else { w.x }
    World { w | x: x }
}

draw :: (w: World) -> List[Shape] {
    [Rect { x: w.x, y: w.y, w: 20.0, h: 20.0, color: :yellow }]
}
```

```sh
ludo play examples/square.ludo
```

`init` builds the world, `update` makes a new one 60 times per second, and
`draw` says what to show. The language is explained in more detail [Language](#language),
and games in detail in [Games](#games).

## Work in progress

These are planned but not implemented yet.

- Enums and `case`. Enums as closed sets of symbols, like `State :: enum { :idle, :serving }`, and a `case` that reports a missing symbol before the program runs, with `when` guards.
- Type checking Types are written but not checked yet. A checker will report a wrong type before the program runs.
- Option: `Some(x)` and `None` for a value that may be missing, instead of an error.
- Errors as values, a function that can fail returns `Ok(value)` or `Error(reason)`, and the caller handles both with `case`. There are no exceptions.
- Vec2: A 2D vector with `+`, `-` and `*`, for positions and velocities.
- Checking names before running: an undefined name, a call with the wrong number of arguments or a missing field is reported when the file loads, and a function can only use a top level `let` declared before it.
- Accessing lists by index
- Sound and sprites 

## Installation

### Prerequisites

ludo is built from source, so it needs [Rust](https://rust-lang.org/tools/install/)
(stable, 1.70 or newer) and `make`. On Linux, the game window also needs a few
system libraries.

```sh
sudo apt install pkg-config libx11-dev libxi-dev libgl1-mesa-dev
```

macOS and Windows need nothing else.

### Building

From the repository, run

```sh
make install
```

That builds `ludo` and puts it in `~/.cargo/bin`. The Rust installer usually adds
that folder to the `PATH`. If `ludo` is not found afterwards, add it by hand and
open a new terminal.

```sh
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc    # or ~/.bashrc
```

The installed `ludo` is a copy, so after changing the code run `make install`
again.

### Usage

```sh
ludo                    # start the REPL
ludo run <file>         # run a program
ludo play <file>        # play a game in a window
```

### REPL mode

Each expression shows its value, and an input that is not finished, like a
function whose `{` is still open, continues with `...`. A blank line gives up on
it.

```
$ ludo
> 1 + 2 * 3
7
> let name = "ludo"
> "hello, " + name
hello, ludo
> double :: (x: Int) -> Int {
...   x * 2
... }
> [1, 2, 3] |> map(double)
[2, 4, 6]
```

### Running a file

`ludo run` runs a file from top to bottom and shows only what `print` writes. A
game file only declares its functions, so running it just prints what is at its
top level.

```
$ ludo run examples/pong.ludo
This is a game file to play pong
```

### Playing a game

```sh
ludo play examples/pong.ludo
```

![Pong](examples/pong.gif)

See `examples/README.md`.

## Makefile

The same commands, always with the current code and without installing.

```sh
make run FILE=examples/pong.ludo
make play FILE=examples/pong.ludo
make test
make lint
```

## Language

### Comments

A comment starts with `//` and runs to the end of the line.

```
let speed = 320.0   // pixels per second
```

### Types

| | | |
| --- | --- | --- |
| `Int` | 64 bit integer | |
| `Float` | 64 bit float | |
| `Bool` | `true` / `false` | |
| `String` | text | |
| `Symbol` | a name used as a label, like `:jump` ||
| `List[T]` | immutable list ||
| `struct` | named record with typed fields | |

Types are written in signatures and fields, but nothing checks them yet, the
type checker is a work in progress and is not yet fully implemented.

### Variables

`let` gives a name to a value. Values never change, so to update a name you bind it
again with a new `let`, which can use the old value.

```
let x = 1
let x = x + 1       // x is 2
```

### Numbers

```
7 / 2               // 3
7.0 / 2.0           // 3.5
7 % 2               // 1
to_float(3)         // 3.0
```

A number with an `s` right after it is a time in seconds, like `0.2s` or `2s`.

### Operators

| | Operators |
| --- | --- |
| `or` | `or` |
| `and` | `and` |
| equality | `==` `!=` |
| comparison | `<` `<=` `>` `>=` |
| pipe | `\|>` |
| term | `+` `-` |
| factor | `*` `/` `%` |
| unary | `-` `!` |
| postfix | `f(x)` `a.b` `a.b(x)` |

```
1 < 2 and !false    // true
"con" + "cat"       // concat
:idle == :idle      // true
```

`and`, `or`, `!` and `if` need a `Bool`, so `if 0 { ... }` is an error and not
false.

### Lines

A statement ends at the end of its line. Inside `( )` and `[ ]` you can break
lines freely.

```
let p = player
  |> handle(input)
  |> physics(dt)
```

### Blocks

A block runs its lines in order, and its value is the last one.

```
let area = {
    let w = 4
    let h = 5
    w * h
}
```

### Scope

Names bound inside a block exist only there, and a `let` inside a block does
not change the name outside it.

```
let x = 1
{
    let x = 5
    x               // 5
}
x                   // 1
```

A function can call any function of the file, even one declared after it, so two
functions can call each other. It sees the names at the top level of the file,
but not the names of the code that calls it.

```
is_even :: (n: Int) -> Bool { n == 0 or is_odd(n - 1) }
is_odd :: (n: Int) -> Bool { n != 0 and is_even(n - 1) }
is_even(10)         // true

peek :: () -> Int { hidden }
{
    let hidden = 1
    peek()          // error: undefined variable `hidden`
}
```

### If

`if` is an expression. It has a value, so it always needs an `else`. Conditions
chain with `else if`.

```
let size = if score > 20 { :huge } else if score > 10 { :big } else { :small }
```

### Functions

A function is declared at the top level, with a type for every parameter and
for the result. Its value is the last line of its body. There is no `return`,
so an early exit is a branch of an `if`.

```
square :: (x: Int) -> Int { x * x }

fact :: (n: Int) -> Int {
    if n == 0 { 1 } else { n * fact(n - 1) }
}
```

Functions are values, so they can be bound with `let` and passed to other
functions. Recursion more than 200 calls deep is an error, so to go over a long
list use `map`, `filter` and `fold`.

### Pipes and calls

`|>` and `.` put the first argument before the function. These three are the
same call.

```
square(4)
4 |> square
4.square()
```

A `.` without parentheses reads a field, as in `p.pos`.

### Structs

A struct is a named group of fields. Every field must be given when it is
built, and a struct is "changed" by building a copy with some fields replaced.
Struct names start with an uppercase letter.

```
Ball :: struct { x: Float, y: Float, vx: Float }

let b = Ball { x: 0.0, y: 0.0, vx: 1.0 }
let b = Ball { b | x: b.x + b.vx }
b.x                 // 1.0
```

In the condition of an `if`, put a struct in parentheses, as in
`if (Ball { x: 0.0, y: 0.0, vx: 1.0 }) == b { ... }`.

### Symbols

A symbol is a label, useful for states and names that are not text.

```
let state = :serving
state == :serving   // true
```

### Lists

A list cannot be changed either, and operations return a new one. There are no
loops. A list is traversed with `map`, `filter` and `fold`, and `range(a, b)`
gives the numbers from `a` to `b - 1`.

```
even :: (x: Int) -> Bool { x % 2 == 0 }
add :: (a: Int, b: Int) -> Int { a + b }

range(0, 5)                         // [0, 1, 2, 3, 4]
range(0, 5) |> filter(even)         // [0, 2, 4]
range(1, 11) |> fold(0, add)        // 55
concat([1], [2, 3])                 // [1, 2, 3]
[:a, :b] |> contains(:a)            // true
```

### Sequences

Some things in a game take more than one frame. A serve waits a second before
the ball moves, and an animation grows something little by little. Without help
you would keep a timer in the world and check it in every `update`. A sequence
lets you write those steps in order, as if the game waited for them.

```
serve :: sequence (w: World) {
    wait 1.0s                               // do nothing for one second
    over 0.4s as t {                        // every frame for 0.4 seconds
        w.pop = t                           // grow the ball, t goes from 0 to 1
    }
    w.ball = Ball { w.ball | vx: 300.0 }    // then send it off
}
```

The first parameter is the world of the game. Every frame the sequence gets the
world as it is at that moment, and a line like `w.pop = t` changes one of its
fields. Those lines are the only way to set a field in ludo, and they only work
on that first parameter.

A sequence does not run when it is declared. `start serve(w)` starts it, usually
from `update` when something happens, and from then on it moves forward a
little every frame until it ends. Starting it again while it runs begins it
again from the top.

`wait` and `over` go directly inside the sequence, not inside an `if` or a
function. Sequences only run inside a game, see [Games](#games).

### Native functions

| | |
| --- | --- |
| `print(x)` | writes `x` to the terminal and returns it |
| `map`, `filter`, `fold`, `range`, `concat`, `contains` | lists |
| `abs`, `min`, `clamp` | numbers |
| `to_float`, `to_int`, `to_string` | conversions |

```
abs(-3)             // 3
min(2.5, 1.0)       // 1.0
clamp(15, 0, 10)    // 10
to_int(2.9)         // 2
to_string(42)       // "42"
```

## Grammar

The grammar follows the notation of Crafting Interpreters (Appendix I). Words in
quotes are written as they are, names in capitals are tokens, `*` means any
number of times, `+` one or more times and `?` optional.

### Syntax grammar

A program is a list of lines. Declarations only go at the top level.

```
program        → ( topLevel? NEWLINE )* EOF ;
topLevel       → declaration | statement ;
```

#### Declarations

```
declaration    → funDecl | structDecl | seqDecl ;
funDecl        → IDENTIFIER "::" function ;
structDecl     → IDENTIFIER "::" "struct" "{" ( field ( separator field )* separator? )? "}" ;
seqDecl        → IDENTIFIER "::" "sequence" "(" parameters ")" "{" ( step NEWLINE )* "}" ;
```

#### Statements

```
statement      → letDecl | startStmt | exprStmt ;
letDecl        → "let" IDENTIFIER "=" expression ;
startStmt      → "start" IDENTIFIER "(" arguments? ")" ;
exprStmt       → expression ;
step           → "wait" expression
               | "over" expression ( "as" IDENTIFIER )? "{" ( passStep NEWLINE )* "}"
               | passStep ;
passStep       → IDENTIFIER "." IDENTIFIER "=" expression | statement ;
```

#### Expressions

```
expression     → logic_or ;
logic_or       → logic_and ( "or" logic_and )* ;
logic_and      → equality ( "and" equality )* ;
equality       → comparison ( ( "==" | "!=" ) comparison )* ;
comparison     → pipe ( ( "<" | "<=" | ">" | ">=" ) pipe )* ;
pipe           → term ( "|>" term )* ;
term           → factor ( ( "+" | "-" ) factor )* ;
factor         → unary ( ( "*" | "/" | "%" ) unary )* ;
unary          → ( "-" | "!" ) unary | call ;
call           → primary ( "(" arguments? ")" | "." IDENTIFIER ( "(" arguments? ")" )? )* ;
primary        → INT | FLOAT | STRING | SYMBOL | IDENTIFIER | "true" | "false"
               | "(" expression ")" | ifExpr | block | structLit | list ;
ifExpr         → "if" expression block "else" ( ifExpr | block ) ;
block          → "{" ( statement NEWLINE )* expression "}" ;
structLit      → IDENTIFIER "{" ( expression "|" )? ( fieldValue ( separator fieldValue )* separator? )? "}" ;
list           → "[" ( expression ( "," expression )* ","? )? "]" ;
```

#### Utility rules

```
function       → "(" parameters? ")" "->" type block ;
parameters     → IDENTIFIER ":" type ( "," IDENTIFIER ":" type )* ","? ;
arguments      → expression ( "," expression )* ","? ;
type           → IDENTIFIER ( "[" type ( "," type )* ","? "]" )? ;
field          → IDENTIFIER ":" type ;
fieldValue     → IDENTIFIER ":" expression ;
separator      → "," | NEWLINE ;
```

### Lexical grammar

```
INT            → DIGIT+ ;
FLOAT          → DIGIT+ "." DIGIT+ | DIGIT+ ( "." DIGIT+ )? "s" ;
STRING         → "\"" <any char except "\"" and a newline>* "\"" ;
SYMBOL         → ":" ( LOWER | "_" ) ( ALPHA | DIGIT )* ;
IDENTIFIER     → ALPHA ( ALPHA | DIGIT )* ;
ALPHA          → "a" ... "z" | "A" ... "Z" | "_" ;
LOWER          → "a" ... "z" ;
DIGIT          → "0" ... "9" ;
```

## Errors

Errors say what went wrong and where. A file with syntax errors does not run,
and every error in it is reported at once. This program

```
let x = 1 +
let 2 = y
x = 5
```

prints

```
[line 1] Parsing Error: expected an expression, at end of line
[line 2] Parsing Error: expected a name after `let`, found `2`
[line 3] Parsing Error: there is no assignment: bind a new value with `let`, or change a field of the state of a sequence, found `=`
```

A runtime error stops the program at the line where it happened, even inside a
function. This program

```
f :: (n: Int) -> Int {
    10 / n
}
print(f(2))
print(f(0))
```

prints `5` and then stops

```
5
[line 2] Runtime Error: division by zero
```

## Games

`ludo play` opens a window and runs a game. A game is a file that
defines these three functions, and the host calls them. Statements at the top
level of the file run once when it loads, and again on every reload.

```
init :: () -> World
update :: (w: World, input: Input, dt: Float) -> World
draw :: (w: World) -> List[Shape]
```

- `config`, if the game defines it, is called once before the window opens, as in
  `config :: () -> Config { Config { width: 800, height: 600, title: "Pong" } }`.
  Without it the window is 640×480 and titled "ludo". Width and height are
  `Int`s from 1 to 4096.
- `update` is called 60 times per second of game time, always with
  `dt = 1.0 / 60.0`. After each call, every running sequence takes one step over
  the new world, and what the last one leaves is the world for the next step.
- `draw` is called once per frame and returns what to show, from the back, as
  `Rect { x, y, w, h, color }`, `Circle { x, y, r, color }`,
  `Line { x1, y1, x2, y2, thickness, color }` and
  `Text { x, y, size, text, color }`. All numbers are `Float`s and `(0, 0)` is
  the top left corner.
- `held(input, :j)` says whether a key is down, and `pressed(input, :j)`
  whether it went down in this step, once per press however long it is held.
  The keys are the letters `:a` to `:z`, `:up`, `:down`, `:left`, `:right`,
  `:space`, `:enter`, `:escape`, `:tab`, `:backspace`, `:shift` and `:control`.
  `input.up`, `input.w`, `input.space` and the other arrow and WASD keys are
  also plain `Bool` fields, as a shorthand for `held`.
- A color is `rgb(r, g, b)` or `rgba(r, g, b, a)`, with each channel an `Int`
  from 0 to 255 and `a` being the opacity, or one of ten names as a symbol, which
  are `:white`,
  `:black`, `:gray`, `:red`, `:green`, `:blue`, `:yellow`, `:orange`, `:purple`,
  `:pink`.

A runtime error stops the game and shows the message at the top of the window.

Saving the file while the game runs reloads it. The functions and `let`s are
replaced and the game goes on from where it was, so a change to a speed or a
color shows up at once. A file with errors is not loaded, the errors are shown
and the game keeps running the previous version. If a struct changed its fields,
the game restarts with `init()`, since the old world no longer fits. Sequences
that were running go on through the steps they started with, and use the new
functions and `let`s from then on. F5 restarts the game by hand.

`examples/pong.ludo` is a complete game.

## Differences with Lox

ludo follows the structure of Crafting Interpreters, but the language is quite
different from Lox. These are the main design differences with Lox and its
implementations, jlox and plox.

| | Lox | ludo |
| --- | --- | --- |
| Variables | `var`, and `=` changes them | `let` only. Values never change, and a new `let` with the same name hides the old one |
| Scope | a block's variables live in a table that assignments and closures can change later | a block's names disappear when it ends, and nothing can change them in the meantime |
| Functions | closures that capture the scope where they were declared | declared at the top level. They see the top level when called, never the caller's variables |
| Values of blocks | `if` and blocks are statements, and functions `return` | everything is an expression. `if` needs an `else`, and there is no `return` |
| Truthiness | `nil` and `false` are false, everything else is true | conditions must be a `Bool` |
| `nil` | a value | does not exist |
| Types | dynamic | written in every signature and field |
| Objects | classes, methods, `this` and inheritance | structs with fields, and plain functions. `x.f(y)` is `f(x, y)` |
| Loops | `while` and `for` | none. Lists are traversed with `map`, `filter` and `fold` |
| `print` | a statement | a function |
