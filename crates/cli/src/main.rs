use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use ludo_core::ast::Stmt;
use ludo_core::checker::Checker;
use ludo_core::chunk::Function;
use ludo_core::compiler::{compile, CompileError};
use ludo_core::interpreter::{Interpreter, RuntimeError};
use ludo_core::parser::{parse, ParseError};
use ludo_core::scanner::scan_tokens;
use ludo_core::value::Value;
use ludo_core::vm::Vm;
use ludo_play::{Game, HOST};

const EX_USAGE: u8 = 64;
const EX_DATAERR: u8 = 65;
const EX_SOFTWARE: u8 = 70;

enum Failure {
    Static,
    Runtime,
    Incomplete(Vec<ParseError>),
}

/// How the REPL and `run` run a program, as chosen by a flag.
#[derive(Clone, Copy)]
enum Mode {
    Vm,
    /// `--interpreter`: on the tree-walking interpreter, to compare.
    Interpreter,
    /// `--trace`: on the VM, showing the bytecode of each input and tracing
    /// it as it runs.
    Trace,
}

/// What runs the statements: the VM, compiling each one to bytecode first,
/// or the interpreter, walking its tree.
enum Backend {
    Vm(Vm),
    Interpreter(Interpreter),
}

enum Fault {
    Compile(CompileError),
    Runtime(RuntimeError),
}

impl Backend {
    fn new(mode: Mode) -> Self {
        match mode {
            Mode::Vm => Backend::Vm(Vm::new()),
            Mode::Interpreter => Backend::Interpreter(Interpreter::new()),
            Mode::Trace => {
                let mut vm = Vm::new();
                vm.trace = true;
                Backend::Vm(vm)
            }
        }
    }

    fn execute(&mut self, stmt: &Stmt) -> Result<Option<Value>, Fault> {
        match self {
            Backend::Vm(vm) => {
                let script = compile(stmt).map_err(Fault::Compile)?;
                if vm.trace {
                    listing(stmt, &script);
                }
                vm.run(script).map_err(Fault::Runtime)
            }
            Backend::Interpreter(interpreter) => interpreter.execute(stmt).map_err(Fault::Runtime),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (flags, args): (Vec<&str>, Vec<&str>) = args
        .iter()
        .map(String::as_str)
        .partition(|arg| arg.starts_with("--"));
    let mode = match flags.as_slice() {
        [] => Some(Mode::Vm),
        ["--interpreter"] => Some(Mode::Interpreter),
        ["--trace"] => Some(Mode::Trace),
        _ => None,
    };

    match (args.as_slice(), mode) {
        ([], Some(mode)) => {
            run_repl(mode);
            ExitCode::SUCCESS
        }
        (["run", path], Some(mode @ (Mode::Vm | Mode::Interpreter))) => {
            read(path).map_or_else(|code| code, |source| run_file(&source, mode))
        }
        (["play", path], Some(Mode::Vm)) => {
            read(path).map_or_else(|code| code, |source| play(path, &source))
        }
        (["dis", path], Some(Mode::Vm)) => {
            read(path).map_or_else(|code| code, |source| disassemble(&source))
        }
        _ => {
            eprintln!("usage: ludo [--interpreter | --trace]    start the REPL");
            eprintln!("       ludo run [--interpreter] FILE     run a program");
            eprintln!("       ludo play FILE                    play a game file in a window");
            eprintln!("       ludo dis FILE                     compile to bytecode and trace it on the VM");
            eprintln!();
            eprintln!("The REPL and `run` compile to bytecode and run on the VM. With");
            eprintln!("--interpreter they run on the tree-walking interpreter instead, to");
            eprintln!("compare; with --trace the REPL shows the bytecode of each input and");
            eprintln!("traces it as it runs.");
            ExitCode::from(EX_USAGE)
        }
    }
}

fn read(path: &str) -> Result<String, ExitCode> {
    std::fs::read_to_string(path).map_err(|err| {
        eprintln!("could not read `{path}`: {err}");
        ExitCode::from(EX_USAGE)
    })
}

/// A backend and a checker with the host prelude loaded, so a game file sees
/// what `play` gives it.
fn prepared(mode: Mode) -> (Backend, Checker) {
    let mut backend = Backend::new(mode);
    let mut checker = Checker::new();
    if run(HOST, &mut backend, &mut checker, true).is_err() {
        unreachable!("the host prelude is valid");
    }
    (backend, checker)
}

fn run_file(source: &str, mode: Mode) -> ExitCode {
    let (mut backend, mut checker) = prepared(mode);
    match run(source, &mut backend, &mut checker, true) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Static | Failure::Incomplete(_)) => ExitCode::from(EX_DATAERR),
        Err(Failure::Runtime) => ExitCode::from(EX_SOFTWARE),
    }
}

fn play(path: &str, source: &str) -> ExitCode {
    match Game::load(source) {
        Ok(game) => {
            ludo_play::play(game, path.to_string());
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in errors {
                eprintln!("{error}");
            }
            ExitCode::from(EX_DATAERR)
        }
    }
}

fn disassemble(source: &str) -> ExitCode {
    let statements = match scan_tokens(source) {
        Ok(tokens) => match parse(tokens) {
            Ok(statements) => statements,
            Err(errors) => {
                report(&errors, true);
                return ExitCode::from(EX_DATAERR);
            }
        },
        Err(errors) => {
            for error in &errors {
                eprintln!("[line {}] Scanning Error: {}", error.line, error.message);
            }
            return ExitCode::from(EX_DATAERR);
        }
    };

    let (Backend::Vm(mut vm), mut checker) = prepared(Mode::Vm) else {
        unreachable!("`dis` runs on the VM")
    };
    if let Err(errors) = checker.check(&statements) {
        for error in &errors {
            eprintln!("{error}");
        }
        return ExitCode::from(EX_DATAERR);
    }
    vm.trace = true;
    for stmt in &statements {
        let script = match compile(stmt) {
            Ok(script) => script,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(EX_DATAERR);
            }
        };
        listing(stmt, &script);
        match vm.run(script) {
            Ok(Some(value)) => println!("=> {value}\n"),
            Ok(None) => println!(),
            Err(error) => {
                let line = error
                    .line
                    .map(|l| format!("[line {l}] "))
                    .unwrap_or_default();
                eprintln!("{line}Runtime Error: {}", error.message);
                return ExitCode::from(EX_SOFTWARE);
            }
        }
    }
    ExitCode::SUCCESS
}

/// Shows the bytecode of a statement and of the function or sequence it
/// declares.
fn listing(stmt: &Stmt, script: &Function) {
    println!("== {stmt} ==\n{}", script.chunk);
    for constant in &script.chunk.constants {
        if let Value::Compiled(function) = constant {
            println!("== {function} ==\n{}", function.chunk);
        }
    }
}

/// Each input is compiled and run on its own, on a backend that lives as
/// long as the REPL, so what one input binds the next ones see. On the VM
/// it is the REPL of clox, of Python and of Lua.
fn run_repl(mode: Mode) {
    let stdin = io::stdin();
    let (mut backend, mut checker) = prepared(mode);
    let mut source = String::new();

    loop {
        print!("{}", if source.is_empty() { "> " } else { "... " });
        let _ = io::stdout().flush();

        let mut bytes = Vec::new();
        match stdin.lock().read_until(b'\n', &mut bytes) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                eprintln!("read error: {err}");
                break;
            }
        }

        let line = String::from_utf8_lossy(&bytes);
        let blank = line.trim().is_empty();
        source.push_str(&line);

        match run(&source, &mut backend, &mut checker, false) {
            Err(Failure::Incomplete(_)) if !blank => continue,
            Err(Failure::Incomplete(errors)) => report(&errors, false),
            _ => {}
        }
        source.clear();
    }
}

fn run(
    source: &str,
    backend: &mut Backend,
    checker: &mut Checker,
    file: bool,
) -> Result<(), Failure> {
    let at = |line: usize| {
        if file {
            format!("[line {line}] ")
        } else {
            String::new()
        }
    };

    let tokens = match scan_tokens(source) {
        Ok(tokens) => tokens,
        Err(errors) => {
            for error in &errors {
                eprintln!("{}Scanning Error: {}", at(error.line), error.message);
            }
            return Err(Failure::Static);
        }
    };

    let statements = match parse(tokens) {
        Ok(statements) => statements,
        Err(errors) if !file && errors.last().is_some_and(|e| e.at_end) => {
            return Err(Failure::Incomplete(errors));
        }
        Err(errors) => {
            report(&errors, file);
            return Err(Failure::Static);
        }
    };

    if let Err(errors) = checker.check(&statements) {
        for error in &errors {
            eprintln!("{}{} Error: {}", at(error.line), error.kind, error.message);
        }
        return Err(Failure::Static);
    }

    for stmt in &statements {
        match backend.execute(stmt) {
            Ok(Some(value)) if !file => println!("{value}"),
            Ok(_) => {}
            Err(Fault::Compile(error)) => {
                eprintln!("{}Compile Error: {}", at(error.line), error.message);
                return Err(Failure::Static);
            }
            Err(Fault::Runtime(error)) => {
                let line = error.line.map(at).unwrap_or_default();
                eprintln!("{line}Runtime Error: {}", error.message);
                return Err(Failure::Runtime);
            }
        }
    }
    Ok(())
}

fn report(errors: &[ParseError], file: bool) {
    for error in errors {
        let at = if file {
            format!("[line {}] ", error.line)
        } else {
            String::new()
        };
        eprintln!("{at}Parsing Error: {}", error.message);
    }
}
