//! Runs a program on either backend, so the CLI and `play` can choose one:
//! the VM, which compiles each statement to bytecode first, or the
//! interpreter, which walks its tree.

use std::collections::HashMap;

use crate::ast::Stmt;
use crate::compiler::{compile, CompileError};
use crate::interpreter::Interpreter;
use crate::runtime::{RuntimeError, Sequences};
use crate::value::Value;
use crate::vm::Vm;

/// Which backend runs a program.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    Vm,
    Interpreter,
}

/// Why a statement did not run.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Compile(CompileError),
    Runtime(RuntimeError),
}

/// A program running on one of the backends.
pub enum Engine {
    Vm(Vm),
    Interpreter(Interpreter),
}

impl Engine {
    pub fn new(backend: Backend) -> Self {
        match backend {
            Backend::Vm => Engine::Vm(Vm::new()),
            Backend::Interpreter => Engine::Interpreter(Interpreter::new()),
        }
    }

    pub fn backend(&self) -> Backend {
        match self {
            Engine::Vm(_) => Backend::Vm,
            Engine::Interpreter(_) => Backend::Interpreter,
        }
    }

    /// Runs a top-level statement and gives the value it left, if any.
    pub fn execute(&mut self, stmt: &Stmt) -> Result<Option<Value>, Failure> {
        match self {
            Engine::Vm(vm) => {
                let script = compile(stmt).map_err(Failure::Compile)?;
                vm.run(script).map_err(Failure::Runtime)
            }
            Engine::Interpreter(interpreter) => interpreter.execute(stmt).map_err(Failure::Runtime),
        }
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match self {
            Engine::Vm(vm) => vm.call_function(name, args),
            Engine::Interpreter(interpreter) => interpreter.call_function(name, args),
        }
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        match self {
            Engine::Vm(vm) => vm.arity(name),
            Engine::Interpreter(interpreter) => interpreter.arity(name),
        }
    }

    pub fn struct_shapes(&self) -> HashMap<String, Vec<String>> {
        match self {
            Engine::Vm(vm) => vm.struct_shapes(),
            Engine::Interpreter(interpreter) => interpreter.struct_shapes(),
        }
    }

    pub fn run_sequences(&mut self, world: Value, dt: f64) -> Result<Value, RuntimeError> {
        match self {
            Engine::Vm(vm) => vm.run_sequences(world, dt),
            Engine::Interpreter(interpreter) => interpreter.run_sequences(world, dt),
        }
    }

    /// Takes over the sequences running in `from`, which runs on the same
    /// backend: a reload never changes it.
    pub fn adopt_sequences(&mut self, from: &mut Engine) {
        match (self, from) {
            (Engine::Vm(vm), Engine::Vm(from)) => vm.adopt_sequences(from),
            (Engine::Interpreter(interpreter), Engine::Interpreter(from)) => {
                interpreter.adopt_sequences(from)
            }
            _ => unreachable!("a reload keeps the backend"),
        }
    }

    pub fn stop_sequences(&mut self) {
        match self {
            Engine::Vm(vm) => vm.stop_sequences(),
            Engine::Interpreter(interpreter) => interpreter.stop_sequences(),
        }
    }
}

/// The language as it runs, on both backends at once: every statement, call
/// and frame of the game must give the same on each.
#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::parser::parse;
    use crate::scanner::scan_tokens;
    use crate::value::Instance;

    struct Both([Engine; 2]);

    impl Both {
        fn new(source: &str) -> Self {
            let mut both = Both([Engine::new(Backend::Vm), Engine::new(Backend::Interpreter)]);
            both.run(source);
            both
        }

        /// Runs `source` and shows what each statement gave, as the REPL
        /// does.
        fn run(&mut self, source: &str) -> String {
            let mut shown = Vec::new();
            for stmt in parse(scan_tokens(source).unwrap()).unwrap() {
                let [vm, interpreter] = &mut self.0;
                let result = show(vm.execute(&stmt));
                // A function is a tree in one backend and bytecode in the
                // other, so they are compared as they are shown.
                let other = show(interpreter.execute(&stmt));
                assert_eq!(result, other, "the backends disagree on `{stmt}`");
                shown.extend(result);
            }
            shown.join("\n")
        }

        fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
            let [vm, interpreter] = &mut self.0;
            let result = vm.call_function(name, args.clone());
            assert_eq!(result, interpreter.call_function(name, args), "{name}");
            result.map_err(|e| e.message)
        }

        /// Runs `count` frames of `dt`, the first over `world` and each next
        /// one over what the last left, as `play` does. Shows the `x` of the
        /// world after each frame, with `done` once no sequence is running,
        /// or the error a frame stops at.
        fn frames(&mut self, world: Value, dt: f64, count: usize) -> String {
            let mut world = world;
            let mut shown = Vec::new();
            for _ in 0..count {
                let [vm, interpreter] = &mut self.0;
                let result = vm.run_sequences(world.clone(), dt);
                assert_eq!(result, interpreter.run_sequences(world.clone(), dt));
                assert_eq!(running(vm), running(interpreter));
                match result {
                    Ok(next) => {
                        let done = if running(vm).is_empty() { " done" } else { "" };
                        shown.push(format!("{}{done}", x(&next)));
                        world = next;
                    }
                    Err(e) => {
                        shown.push(error(&e));
                        break;
                    }
                }
            }
            shown.join(", ")
        }
    }

    fn show(result: Result<Option<Value>, Failure>) -> Option<String> {
        match result {
            Ok(value) => value.map(|v| v.to_string()),
            Err(Failure::Runtime(e)) => Some(error(&e)),
            Err(Failure::Compile(e)) => panic!("{e}"),
        }
    }

    fn error(e: &RuntimeError) -> String {
        format!("[line {}] error: {}", e.line.unwrap(), e.message)
    }

    fn running(engine: &mut Engine) -> Vec<String> {
        match engine {
            Engine::Vm(vm) => vm.running(),
            Engine::Interpreter(interpreter) => interpreter.running(),
        }
    }

    fn runs(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(Both::new("").run(source), *expected, "{source:?}");
        }
    }

    #[test]
    fn evaluates_expressions() {
        runs(&[
            (
                "1 + 2 * 3\n7 / 2\n7 % 2\n7.0 / 2.0\n-(7 % 4)\n1.0 / 0.0\n\
                 1 < 2\n\"a\" + \"b\"\n:jump == :jump\n!true\nfalse and 1 / 0 == 1\ntrue or 1 / 0 == 1",
                "7\n3\n1\n3.5\n-3\ninf\ntrue\nab\ntrue\nfalse\nfalse\ntrue",
            ),
            (
                "1 + 2.0\n1 == 1.0\n1 / 0\n9223372036854775807 + 1\n-true\n\
                 1 and true\ntrue and 1\nfalse or :no\nx\nif 1 { 2 } else { 3 }",
                "[line 1] error: operands of `+` must be two Ints, two Floats or two Strings, got Int and Float\n\
                 [line 2] error: operands of `==` must be two values of the same type, got Int and Float\n\
                 [line 3] error: division by zero\n\
                 [line 4] error: integer overflow in `+`\n\
                 [line 5] error: operand of `-` must be an Int or a Float, got Bool\n\
                 [line 6] error: operands of `and` must be Bool, got Int\n\
                 [line 7] error: operands of `and` must be Bool, got Int\n\
                 [line 8] error: operands of `or` must be Bool, got Symbol\n\
                 [line 9] error: undefined variable `x`\n\
                 [line 10] error: the condition of `if` must be a Bool, got Int",
            ),
            (
                "if 1 < 2 { :yes } else { 1 / 0 }\nif false { 1 / 0 } else { :no }\n\
                 let n = 0\nif n < 0 { :neg } else if n == 0 { :zero } else { :pos }\n1 + if true { 2 } else { 3 }",
                ":yes\n:no\n:zero\n3",
            ),
        ]);
    }

    #[test]
    fn let_binds_names_that_blocks_scope() {
        runs(&[
            (
                "let x = 1\nlet x = x + 1\nx\nlet x = 1 / 0\nx",
                "2\n[line 4] error: division by zero\n2",
            ),
            (
                "let area = {\n  let w = 4\n  w * 5\n}\narea\nw",
                "20\n[line 6] error: undefined variable `w`",
            ),
            (
                "let x = 1\nlet y = {\n  let x = x + 10\n  x\n}\ny\nx",
                "11\n1",
            ),
            // An error inside a block reports its own line.
            (
                "let x = 1\n{\n  let x = 2\n  x / 0\n}\nx",
                "[line 4] error: division by zero\n1",
            ),
            (
                "{\n  let a = 1\n  let b = {\n    let a = a + 10\n    a * 2\n  }\n  a + b\n}",
                "23",
            ),
            ("{\n  let a = 1\n  let a = a + 1\n  a * 10\n  a\n}", "2"),
            // A block can start above a temporary.
            ("1 + {\n  let a = 2\n  a * 3\n}", "7"),
            (
                "let x = 10\n{\n  let a = 1\n  if a > 0 {\n    let b = a + x\n    b * 2\n  } else {\n    0\n  }\n}",
                "22",
            ),
            (
                "{\n  1\n} + true\n",
                "[line 1] error: operands of `+` must be two Ints, two Floats or two Strings, got Int and Bool",
            ),
        ]);
    }

    #[test]
    fn calls_functions() {
        let add = "add :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[
            (
                &format!("{add}add(2, 3)\n2 |> add(3)\n2.add(3)\n1 + add(2, 3) * 2"),
                "5\n5\n5\n11",
            ),
            (
                "is_even :: (n: Int) -> Bool { n == 0 or is_odd(n - 1) }\n\
                 is_odd :: (n: Int) -> Bool { n != 0 and is_even(n - 1) }\nis_even(10)\nis_odd(7)",
                "true\ntrue",
            ),
            // Globals are looked up when the function runs.
            (
                "scaled :: (x: Int) -> Int { x * SCALE }\nlet SCALE = 10\nscaled(3)",
                "30",
            ),
            (
                "fact :: (n: Int) -> Int { if n == 0 { 1 } else { n * fact(n - 1) } }\nfact(10)",
                "3628800",
            ),
            // A function's locals sit above its parameters, and it does not
            // see the locals of its caller.
            (
                "f :: (x: Int) -> Int {\n  let y = x * 2\n  y + 1\n}\n{\n  let a = 3\n  f(a) + a\n}",
                "10",
            ),
            (
                "peek :: () -> Int { secret }\n{\n  let secret = 1\n  peek()\n}",
                "[line 1] error: undefined variable `secret`",
            ),
            ("let x = 100\nid :: (x: Int) -> Int { x }\nid(1)\nx", "1\n100"),
            (
                "double :: (x: Int) -> Int { x * 2 }\ntwice :: (f: Fn, x: Int) -> Int { f(f(x)) }\ntwice(double, 3)\ndouble",
                "12\n<fn double>",
            ),
        ]);
    }

    #[test]
    fn reports_errors_in_calls() {
        let add = "add :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[
            (
                &format!("{add}id :: (x: Int) -> Int {{ x }}\nadd(1)\nid(1, 2)\nlet n = 1\nn(2)\nid == id"),
                "[line 3] error: `add` expects 2 arguments, got 1\n\
                 [line 4] error: `id` expects 1 argument, got 2\n\
                 [line 6] error: can only call functions, got Int\n\
                 [line 7] error: functions cannot be compared",
            ),
            // An error inside a function keeps its own line, even when it
            // comes back through a native, and leaves the caller's names.
            (
                "let x = 1\nboom :: (x: Int) -> Int {\n  1 / x\n}\nboom(0)\nrange(0, 1) |> map(boom)\nx",
                "[line 3] error: division by zero\n[line 3] error: division by zero\n1",
            ),
            (
                "loop_forever :: (n: Int) -> Int { loop_forever(n + 1) }\nloop_forever(0)",
                "[line 1] error: stack overflow: more than 200 nested calls",
            ),
        ]);
    }

    #[test]
    fn natives_work_on_numbers_and_lists() {
        let fns = "double :: (x: Int) -> Int { x * 2 }\n\
                   even :: (x: Int) -> Bool { x % 2 == 0 }\n\
                   add :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[
            (
                "abs(-3)\nmin(2.0, 1.5)\nclamp(5, 0, 3)\nto_int(-2.7)\nto_float(3)\nto_string(3) + \"!\"\n3 |> print |> abs",
                "3\n1.5\n3\n-2\n3.0\n3!\n3",
            ),
            (
                "[\"a\", :b]\n[[1], [2, 3]]\n[1, 2] == [1, 2]\nconcat([1], [2, 3])\n[:a, :b] |> contains(:b)\n[1] |> contains(1.0)",
                "[\"a\", :b]\n[[1], [2, 3]]\ntrue\n[1, 2, 3]\ntrue\nfalse",
            ),
            (
                "{\n  let a = 1\n  [a, a + 1] |> concat([:b, \"c\"])\n}",
                "[1, 2, :b, \"c\"]",
            ),
            // Natives are globals like any other.
            ("abs\nlet abs = 1\nabs + 1", "<fn abs>\n2"),
            (
                &format!(
                    "{fns}range(0, 5)\nrange(3, 1)\n[] |> fold(7, add)\n\
                     range(0, 10) |> filter(even) |> map(double) |> fold(0, add)\n[-1, 2] |> map(abs)\nrange(1, 1001) |> fold(0, add)"
                ),
                "[0, 1, 2, 3, 4]\n[]\n7\n40\n[1, 2]\n500500",
            ),
            (
                "big :: (n: Int) -> Bool { n > 5 }\nfind(range(0, 10), big)\nfind(range(0, 3), big)",
                "Some(6)\nNone",
            ),
            (
                "parse_int(\"42\")\nparse_int(\" -7 \")\nparse_int(\"4x\")",
                "Ok(42)\nOk(-7)\nError(\"\\\"4x\\\" is not an Int\")",
            ),
        ]);
    }

    #[test]
    fn reports_errors_in_natives() {
        let fns = "double :: (x: Int) -> Int { x * 2 }\nadd :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[(
            &format!(
                "{fns}abs(true)\nclamp(1, 3, 0)\nmin(1)\nabs == abs\n1 |> map(double)\n[1] |> filter(double)\n\
                 [1] |> map(add)\nrange(0, 1.0)\nrange(0, 2000000)\nfind([1], abs)\nparse_int(4)"
            ),
            "[line 3] error: `abs` expects an Int or a Float, got Bool\n\
             [line 4] error: `clamp` expects its lower bound to be at most its upper bound\n\
             [line 5] error: `min` expects 2 arguments, got 1\n\
             [line 6] error: functions cannot be compared\n\
             [line 7] error: `map` expects a List, got Int\n\
             [line 8] error: `filter` expects a function that returns a Bool, got Int\n\
             [line 9] error: `add` expects 2 arguments, got 1\n\
             [line 10] error: `range` expects two Ints, got Int and Float\n\
             [line 11] error: `range` can make at most 1000000 numbers, got 2000000\n\
             [line 12] error: `find` expects a function that returns a Bool, got Int\n\
             [line 13] error: `parse_int` expects a String, got Int",
        )]);
    }

    #[test]
    fn builds_reads_and_updates_structs() {
        let ball = "Ball :: struct { x: Float, y: Float }\n";
        runs(&[
            (
                &format!("{ball}let b = Ball {{ y: 2.0, x: 1.0 }}\nb\nb.y\nBall {{ b | y: 5.0 }}\nb == Ball {{ x: 1.0, y: 2.0 }}"),
                "Ball { x: 1.0, y: 2.0 }\n2.0\nBall { x: 1.0, y: 5.0 }\ntrue",
            ),
            (
                "Label :: struct { text: String }\nLabel { text: \"hi\" }\nEmpty :: struct {}\nEmpty {}",
                "Label { text: \"hi\" }\nEmpty {}",
            ),
            // A literal looks its struct up when it runs, so a function can
            // build a struct declared after it.
            (
                "origin :: () -> P { P { x: 0, y: 0 } }\nP :: struct { x: Int, y: Int }\norigin()",
                "P { x: 0, y: 0 }",
            ),
            (
                &format!(
                    "{ball}World :: struct {{ ball: Ball, score: Int }}\n\
                     bounce :: (w: World) -> World {{\n  let b = w.ball\n  World {{ w | ball: Ball {{ b | x: -b.x }}, score: w.score + 1 }}\n}}\n\
                     let w = World {{ ball: Ball {{ x: 3.0, y: 0.0 }}, score: 0 }}\nbounce(w)"
                ),
                "World { ball: Ball { x: -3.0, y: 0.0 }, score: 1 }",
            ),
            (
                &format!(
                    "{ball}Paddle :: struct {{ y: Float }}\nlet n = 1\nState :: enum {{ :idle }}\n\
                     Ball {{ x: 1.0, y: 2.0 }} == Paddle {{ y: 2.0 }}\n\
                     Ball {{ x: 1.0 }}\n\
                     Ball {{ x: 1.0, y: 2.0, z: 3.0 }}\n\
                     Wall {{ y: 1.0 }}\n\
                     Ball {{ 1 | x: 2.0 }}\n\
                     n.x\n\
                     Ball {{ x: 1.0, y: 2.0 }}.z\n\
                     Ball\n\
                     State"
                ),
                "[line 5] error: operands of `==` must be two values of the same type, got Ball and Paddle\n\
                 [line 6] error: missing field `y` in `Ball`\n\
                 [line 7] error: `Ball` has no field `z`\n\
                 [line 8] error: undefined struct `Wall`\n\
                 [line 9] error: `Ball { x | ... }` needs a Ball as `x`, got Int\n\
                 [line 10] error: Int has no fields\n\
                 [line 11] error: `Ball` has no field `z`\n\
                 [line 12] error: `Ball` is a struct, not a value: build one with `Ball { ... }`\n\
                 [line 13] error: `State` is an enum, not a value: its values are its symbols, like `:idle`",
            ),
        ]);
    }

    #[test]
    fn case_takes_the_first_branch_that_matches() {
        runs(&[
            (
                "let s = :serving\ncase s {\n  :idle -> 1\n  :serving -> 2\n}",
                "2",
            ),
            // Guards, names that bind the value, and `_` for the rest.
            (
                "describe :: (n: Int) -> String {\n  case n {\n    0 -> \"zero\"\n    m when m < 0 -> \"negative\"\n    m when m % 2 == 0 -> to_string(m) + \" is even\"\n    _ -> \"odd\"\n  }\n}\n\
                 describe(0)\ndescribe(-3)\ndescribe(4)\ndescribe(7)",
                "zero\nnegative\n4 is even\nodd",
            ),
            // A value of another type is no match, and no error.
            ("case 1 {\n  1.0 -> :float\n  \"1\" -> :string\n  1 -> :int\n}", ":int"),
            // Inside an expression, with locals around it and in a branch.
            (
                "{\n  let a = 10\n  1 + case a {\n    n when n > 5 -> {\n      let d = n * 2\n      d + a\n    }\n    _ -> 0\n  }\n}",
                "31",
            ),
            ("case true { false -> 0, true -> 1 }", "1"),
            (
                "case :c {\n  :a -> 1\n  :b -> 2\n}\ncase \"x\" { \"y\" -> 1 }",
                "[line 1] error: no branch of `case` matches :c\n[line 5] error: no branch of `case` matches \"x\"",
            ),
            (
                "case 1 {\n  n when n -> 1\n  _ -> 2\n}",
                "[line 2] error: the condition of `when` must be a Bool, got Int",
            ),
            ("case 1 {\n  :a -> 1\n  _ -> 1 / 0\n}", "[line 3] error: division by zero"),
        ]);
    }

    #[test]
    fn options_and_results_are_built_compared_and_read_by_case() {
        runs(&[
            (
                "Some(1)\nNone\nOk(\"yes\")\nError(:nope)\n[Some(1), None]\nSome(Some(2))\n\
                 Some(1) == Some(1)\nSome(1) == None\nOk(1) == Error(1)\nrange(0, 3) |> map(Some)\nSome",
                "Some(1)\nNone\nOk(\"yes\")\nError(:nope)\n[Some(1), None]\nSome(Some(2))\n\
                 true\nfalse\nfalse\n[Some(0), Some(1), Some(2)]\n<fn Some>",
            ),
            (
                "Some(1) + 1\nSome()\nSome(1) == Ok(1)",
                "[line 1] error: operands of `+` must be two Ints, two Floats or two Strings, got Option and Int\n\
                 [line 2] error: `Some` expects 1 argument, got 0\n\
                 [line 3] error: operands of `==` must be two values of the same type, got Option and Result",
            ),
            (
                "describe :: (r: Result[Int, String]) -> String {\n  case r {\n    Ok(0) -> \"zero\"\n    Ok(n) when n < 0 -> \"negative\"\n    Ok(n) -> to_string(n)\n    Error(reason) -> \"error: \" + reason\n  }\n}\n\
                 describe(Ok(0))\ndescribe(Ok(-1))\ndescribe(Ok(7))\ndescribe(Error(\"bad\"))",
                "zero\nnegative\n7\nerror: bad",
            ),
            // Nested patterns, and a guard that fails after a binding.
            (
                "case Some(Ok(3)) { Some(Error(_)) -> 1, Some(Ok(n)) -> n, None -> 0 }\n\
                 case Some(5) { Some(x) when x > 10 -> :big, Some(x) -> x, None -> 0 }\n\
                 case None { Some(x) when x > 1 -> 1, _ -> 2 }\ncase None {\n  Some(x) -> x\n}",
                "3\n5\n2\n[line 4] error: no branch of `case` matches None",
            ),
        ]);
    }

    const BALL: &str = "Ball :: struct { x: Float, y: Float }\n";
    const START: &str = "Ball { x: 0.0, y: 0.0 }";

    fn ball(x: f64) -> Value {
        Value::Struct(Rc::new(Instance {
            name: "Ball".into(),
            fields: vec![
                ("x".into(), Value::Float(x)),
                ("y".into(), Value::Float(0.0)),
            ],
        }))
    }

    fn x(world: &Value) -> String {
        match world {
            Value::Struct(ball) => ball.fields[0].1.to_string(),
            other => other.to_string(),
        }
    }

    /// Both backends with `Ball` and the sequence `s` with these parameters
    /// and steps, started over a ball.
    fn sequence(params: &str, steps: &str, args: &str) -> Both {
        Both::new(&format!(
            "{BALL}half :: (x: Float) -> Float {{ x / 2.0 }}\n\
             s :: sequence ({params}) {{\n  {steps}\n}}\nstart s({START}{args})"
        ))
    }

    #[test]
    fn wait_and_over_take_whole_frames() {
        let cases = [
            ("b.x = 1.0\n  b.x = b.x + 1.0", 1, "2.0 done"),
            ("wait 1.0s\n  b.x = 5.0", 4, "0.0, 0.0, 0.0, 5.0 done"),
            (
                "over 1.0s as t {\n    b.x = t\n  }",
                4,
                "0.25, 0.5, 0.75, 1.0 done",
            ),
            ("over 0.3s as t {\n    b.x = t\n  }", 2, "0.5, 1.0 done"),
            ("wait 0.0s\n  b.x = 1.0", 1, "1.0 done"),
            (
                "b.x = 1.0\n  wait 0.5s\n  b.x = 2.0\n  wait 0.25s\n  b.x = 3.0",
                3,
                "1.0, 2.0, 3.0 done",
            ),
            (
                "over 0.5s as t {\n    b.x = half(t) |> min(0.3)\n  }",
                2,
                "0.25, 0.3 done",
            ),
            // Binding the state again replaces it.
            (
                "let b = Ball { x: 5.0, y: 0.0 }\n  b.x = b.x + 1.0\n  over 0.5s {\n    let b = Ball { b | x: b.x * 2.0 }\n  }",
                2,
                "12.0, 24.0 done",
            ),
        ];
        for (steps, count, expected) in cases {
            let frames = sequence("b: Ball", steps, "").frames(ball(0.0), 0.25, count);
            assert_eq!(frames, expected, "{steps:?}");
        }

        let mut both = sequence("b: Ball", "over 0.1s as t {\n    b.x = t\n  }", "");
        let at_60 = both.frames(ball(0.0), 1.0 / 60.0, 6);
        assert!(at_60.ends_with("0.8333333333333334, 1.0 done"), "{at_60}");

        // Locals of the sequence last between frames; those of the body of
        // `over` last a pass.
        let mut both = sequence(
            "b: Ball, to: Float",
            "let from = b.x\n  over 0.5s as t {\n    let d = to - from\n    b.x = from + d * t\n  }",
            ", 20.0",
        );
        assert_eq!(both.frames(ball(10.0), 0.25, 2), "15.0, 20.0 done");
    }

    #[test]
    fn running_sequences_step_over_the_current_world() {
        let mut both = Both::new(&format!(
            "{BALL}push :: sequence (b: Ball, by: Float) {{\n  wait 0.5s\n  b.x = b.x + by\n}}\n\
             kick :: (b: Ball) -> Ball {{\n  start push(b, 10.0)\n  b\n}}\nkick({START})"
        ));
        assert_eq!(running(&mut both.0[0]), ["push"]);
        assert_eq!(both.frames(ball(1.0), 0.25, 2), "1.0, 11.0 done");

        let mut both = sequence("b: Ball", "over 1.0s {\n    b.x = b.x + 1.0\n  }", "");
        assert_eq!(
            both.frames(ball(100.0), 0.25, 4),
            "101.0, 102.0, 103.0, 104.0 done"
        );

        // They run in the order they were started, and one started by
        // another joins after it, from the next frame on.
        let mut both = Both::new(&format!(
            "{BALL}double :: sequence (b: Ball) {{\n  b.x = b.x * 2.0\n}}\n\
             inc :: sequence (b: Ball) {{\n  b.x = b.x + 1.0\n}}\nstart double({START})\nstart inc({START})"
        ));
        assert_eq!(both.frames(ball(3.0), 0.25, 1), "7.0 done");
        let mut both = Both::new(&format!(
            "{BALL}second :: sequence (b: Ball) {{\n  b.x = b.x + 10.0\n}}\n\
             first :: sequence (b: Ball) {{\n  b.x = 1.0\n  start second(b)\n}}\nstart first({START})"
        ));
        assert_eq!(both.frames(ball(0.0), 0.25, 2), "1.0, 11.0 done");

        // `grow` runs first, so `pause` reads `b.x` for its second `over` on
        // the next frame, 0.5 and two passes, not on the first, 0.25.
        let mut both = Both::new(&format!(
            "{BALL}grow :: sequence (b: Ball) {{\n  over 1.0s {{\n    b.x = b.x + 0.25\n  }}\n}}\n\
             pause :: sequence (b: Ball) {{\n  wait 0.25s\n  over b.x {{}}\n  b.x = 100.0\n}}\n\
             start grow({START})\nstart pause({START})"
        ));
        assert_eq!(
            both.frames(ball(0.0), 0.25, 4),
            "0.25, 0.5, 100.0, 100.25 done"
        );
    }

    #[test]
    fn starting_a_running_sequence_replaces_it() {
        let mut both = sequence("b: Ball, to: Float", "wait 0.5s\n  b.x = to", ", 1.0");
        assert_eq!(both.frames(ball(0.0), 0.25, 1), "0.0");
        both.run(&format!("start s({START}, 2.0)"));
        assert_eq!(running(&mut both.0[0]), ["s"]);
        assert_eq!(both.frames(ball(0.0), 0.25, 2), "0.0, 2.0 done");
    }

    #[test]
    fn reports_errors_in_sequences() {
        let cases = [
            ("wait 1", "a duration must be a Float in seconds, got Int"),
            (
                "wait -1.0s",
                "a duration must be a finite number of seconds, at least 0s, got -1.0",
            ),
            ("b.z = 1.0", "`Ball` has no field `z`"),
        ];
        for (steps, message) in cases {
            let frames = sequence("b: Ball", steps, "").frames(ball(0.0), 0.25, 1);
            assert_eq!(frames, format!("[line 4] error: {message}"));
        }
        let frames = sequence("b: Int", "b.z = 1.0", "").frames(Value::Int(1), 0.25, 1);
        assert_eq!(
            frames,
            "[line 4] error: `b.z = ...` needs `b` to be a struct, got Int"
        );

        assert_eq!(
            Both::new("").run("s :: sequence (n: Int) {}\nstart s()\nstart t(1)\ns(1)"),
            "[line 2] error: `s` expects 1 argument, got 0\n\
             [line 3] error: undefined sequence `t`\n\
             [line 4] error: `s` is a sequence, not a function: it runs over time and cannot be called"
        );
    }

    #[test]
    fn a_reload_keeps_running_sequences_with_their_code_and_the_new_globals() {
        let source = |step: &str, times: &str, start: &str| {
            format!(
                "{BALL}step :: () -> Float {{ {step} }}\n\
                 s :: sequence (b: Ball) {{\n  over 1.0s {{\n    b.x = b.x + step() * {times}\n  }}\n}}\n{start}"
            )
        };
        let start = format!("start s({START})");
        let reload = |old: &mut Both, new: &mut Both| {
            for (new, old) in new.0.iter_mut().zip(&mut old.0) {
                new.adopt_sequences(old);
                assert!(running(old).is_empty());
            }
        };

        let mut old = Both::new(&source("1.0", "1.0", &start));
        assert_eq!(old.frames(ball(0.0), 0.25, 1), "1.0");
        // The running `s` keeps adding `step()` once, as its old code says,
        // but `step` is the new one.
        let mut new = Both::new(&source("10.0", "2.0", ""));
        reload(&mut old, &mut new);
        assert_eq!(new.frames(ball(1.0), 0.25, 3), "11.0, 21.0, 31.0 done");

        // One that the new code started already wins over the old one.
        let mut old = Both::new(&source("1.0", "1.0", &start));
        let mut new = Both::new(&source("10.0", "2.0", &start));
        reload(&mut old, &mut new);
        assert_eq!(new.frames(ball(0.0), 0.25, 1), "20.0");

        for engine in &mut new.0 {
            engine.stop_sequences();
        }
        assert_eq!(new.frames(ball(0.0), 0.25, 1), "0.0 done");
    }

    /// Runs `test` on a thread with room for the interpreter's 200 nested
    /// calls, which in a debug build need more than the 2 MiB a test thread
    /// gets. The VM keeps its calls in a `Vec` and needs no room.
    fn with_room(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn the_host_calls_functions() {
        with_room(|| {
            let mut both = Both::new(&format!(
                "{BALL}add :: (a: Int, b: Int) -> Int {{ a + b }}\nlet n = 1\n\
                 count :: (n: Int) -> Int {{ if n == 0 {{ 0 }} else {{ 1 + count(n - 1) }} }}"
            ));
            let int = Value::Int;
            assert_eq!(both.call("add", vec![int(2), int(3)]), Ok(int(5)));
            assert_eq!(both.call("abs", vec![int(-4)]), Ok(int(4)));
            assert_eq!(
                both.call("n", vec![]),
                Err("can only call functions, got Int".into())
            );
            assert_eq!(
                both.call("nope", vec![]),
                Err("undefined function `nope`".into())
            );
            // With no script below, the host's call is the first nested one.
            assert_eq!(both.call("count", vec![int(199)]), Ok(int(199)));
            assert_eq!(
                both.call("count", vec![int(200)]),
                Err("stack overflow: more than 200 nested calls".into())
            );

            let shapes = HashMap::from([("Ball".to_string(), vec!["x".into(), "y".into()])]);
            for engine in &both.0 {
                assert_eq!(engine.arity("add"), Some(2));
                assert_eq!(engine.arity("abs"), Some(1));
                assert_eq!(engine.arity("n"), None);
                assert_eq!(engine.arity("nope"), None);
                assert_eq!(engine.struct_shapes(), shapes);
            }
        });
    }
}
