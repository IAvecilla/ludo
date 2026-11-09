use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use ludo_core::ast::Stmt;
use ludo_core::checker::Checker;
use ludo_core::chunk::Function;
use ludo_core::compiler::compile;
use ludo_core::engine::{Backend, Engine, Failure as Fault};
use ludo_core::parser::{parse_with, ExprParser, ParseError};
use ludo_core::scanner::scan_tokens;
use ludo_core::value::Value;
use ludo_play::{Game, Keys, HOST};

const EX_USAGE: u8 = 64;
const EX_DATAERR: u8 = 65;
const EX_SOFTWARE: u8 = 70;

/// How a program stopped before its end.
enum Failure {
    Static,
    Runtime,
    Incomplete(Vec<ParseError>),
}

/// What the flags choose.
#[derive(Default)]
struct Options {
    /// `--interpreter`: run on the tree-walking interpreter instead of the
    /// VM, to compare.
    backend: Backend,
    /// `--trace`: show the bytecode of each input of the REPL and trace it as
    /// it runs.
    trace: bool,
    /// `--parser=ladder` or `--parser=pratt`: how to parse expressions, to
    /// compare the two.
    parser: ExprParser,
    /// `--frames=N`: play a game for N frames without a window, to time it.
    frames: Option<usize>,
}

impl Options {
    fn parse(flags: &[&str]) -> Option<Options> {
        let mut options = Options::default();
        for flag in flags {
            match *flag {
                "--interpreter" => options.backend = Backend::Interpreter,
                "--trace" => options.trace = true,
                "--parser=pratt" => options.parser = ExprParser::Pratt,
                "--parser=ladder" => options.parser = ExprParser::Ladder,
                _ => options.frames = Some(flag.strip_prefix("--frames=")?.parse().ok()?),
            }
        }
        // The trace shows bytecode, which only the VM runs.
        if options.trace && options.backend == Backend::Interpreter {
            return None;
        }
        Some(options)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (flags, args): (Vec<&str>, Vec<&str>) = args
        .iter()
        .map(String::as_str)
        .partition(|arg| arg.starts_with("--"));
    let options = Options::parse(&flags);

    let default_parser = |o: &Options| o.parser == ExprParser::default();
    match (args.as_slice(), options) {
        ([], Some(o)) if o.frames.is_none() => {
            run_repl(&o);
            ExitCode::SUCCESS
        }
        (["run", path], Some(o)) if !o.trace && o.frames.is_none() => {
            read(path).map_or_else(|code| code, |source| run_file(&source, &o))
        }
        (["check", path], Some(o))
            if o.backend == Backend::Vm && !o.trace && o.frames.is_none() =>
        {
            read(path).map_or_else(|code| code, |source| check_file(&source, &o))
        }
        (["play", path], Some(o)) if !o.trace && default_parser(&o) => {
            read(path).map_or_else(|code| code, |source| play(path, &source, &o))
        }
        (["dis", path], Some(o)) if o.backend == Backend::Vm && !o.trace && o.frames.is_none() => {
            read(path).map_or_else(|code| code, |source| disassemble(&source, &o))
        }
        _ => {
            eprintln!("usage: ludo [--interpreter | --trace] [--parser=P]     start the REPL");
            eprintln!("       ludo run [--interpreter] [--parser=P] FILE     run a program");
            eprintln!("       ludo check [--parser=P] FILE                   check a program without running it");
            eprintln!("       ludo play [--interpreter] [--frames=N] FILE    play a game file in a window");
            eprintln!("       ludo dis [--parser=P] FILE                     compile to bytecode and trace it on the VM");
            eprintln!();
            eprintln!("Programs compile to bytecode and run on the VM. With --interpreter they");
            eprintln!("run on the tree-walking interpreter instead, to compare; with --trace the");
            eprintln!("REPL shows the bytecode of each input and traces it as it runs.");
            eprintln!("--parser=ladder parses expressions with the precedence ladder instead of");
            eprintln!("Pratt, and --frames=N plays a game for N frames without a window; both are");
            eprintln!("there for the benchmarks.");
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

/// A backend and a checker that last as long as a program, with the host
/// prelude loaded so a game file sees what `play` gives it.
struct Session {
    engine: Engine,
    checker: Checker,
    parser: ExprParser,
}

impl Session {
    fn new(options: &Options) -> Self {
        let mut session = Session {
            engine: Engine::new(options.backend),
            checker: Checker::new(),
            parser: options.parser,
        };
        if session.run(HOST, true).is_err() {
            unreachable!("the host prelude is valid");
        }
        if let (Engine::Vm(vm), true) = (&mut session.engine, options.trace) {
            vm.trace = true;
        }
        session
    }

    /// Scans, parses and checks a program, and runs it if it has no errors.
    /// In a file every error says its line; in the REPL an input that ends
    /// too soon is incomplete, to go on with the next line.
    fn run(&mut self, source: &str, file: bool) -> Result<(), Failure> {
        let at = |line: usize| match file {
            true => format!("[line {line}] "),
            false => String::new(),
        };
        let statements = self.check(source, file)?;
        for stmt in &statements {
            match self.execute(stmt) {
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

    /// Scans, parses and checks a program, giving its statements.
    fn check(&mut self, source: &str, file: bool) -> Result<Vec<Stmt>, Failure> {
        let at = |line: usize| match file {
            true => format!("[line {line}] "),
            false => String::new(),
        };
        let tokens = scan_tokens(source).map_err(|errors| {
            for error in &errors {
                eprintln!("{}Scanning Error: {}", at(error.line), error.message);
            }
            Failure::Static
        })?;
        let statements = match parse_with(tokens, self.parser) {
            Ok(statements) => statements,
            Err(errors) if !file && errors.last().is_some_and(|e| e.at_end) => {
                return Err(Failure::Incomplete(errors));
            }
            Err(errors) => {
                report(&errors, file);
                return Err(Failure::Static);
            }
        };
        if let Err(errors) = self.checker.check(&statements) {
            for error in &errors {
                eprintln!("{}{} Error: {}", at(error.line), error.kind, error.message);
            }
            return Err(Failure::Static);
        }
        Ok(statements)
    }

    /// Runs a statement. With the trace on, it first shows its bytecode.
    fn execute(&mut self, stmt: &Stmt) -> Result<Option<Value>, Fault> {
        if let Engine::Vm(vm) = &mut self.engine {
            if vm.trace {
                let script = compile(stmt).map_err(Fault::Compile)?;
                listing(stmt, &script);
                return vm.run(script).map_err(Fault::Runtime);
            }
        }
        self.engine.execute(stmt)
    }
}

fn run_file(source: &str, options: &Options) -> ExitCode {
    match Session::new(options).run(source, true) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Static | Failure::Incomplete(_)) => ExitCode::from(EX_DATAERR),
        Err(Failure::Runtime) => ExitCode::from(EX_SOFTWARE),
    }
}

fn check_file(source: &str, options: &Options) -> ExitCode {
    match Session::new(options).check(source, true) {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(EX_DATAERR),
    }
}

fn play(path: &str, source: &str, options: &Options) -> ExitCode {
    let game = match Game::load_on(source, options.backend) {
        Ok(game) => game,
        Err(errors) => {
            for error in errors {
                eprintln!("{error}");
            }
            return ExitCode::from(EX_DATAERR);
        }
    };
    match options.frames {
        Some(frames) => headless(game, frames),
        None => {
            ludo_play::play(game, path.to_string());
            ExitCode::SUCCESS
        }
    }
}

/// Plays a game for a number of frames without a window and with no key
/// down, as fast as it can: each frame runs `update`, the sequences and
/// `draw`. It is what the benchmarks time.
fn headless(mut game: Game, frames: usize) -> ExitCode {
    let keys = Keys::default();
    for _ in 0..frames {
        let frame = game
            .step(&keys, 1.0 / 60.0)
            .and_then(|()| game.draw().map(drop));
        if let Err(error) = frame {
            eprintln!("{error}");
            return ExitCode::from(EX_SOFTWARE);
        }
    }
    ExitCode::SUCCESS
}

fn disassemble(source: &str, options: &Options) -> ExitCode {
    let mut session = Session::new(&Options {
        trace: true,
        parser: options.parser,
        ..Options::default()
    });
    let Ok(statements) = session.check(source, true) else {
        return ExitCode::from(EX_DATAERR);
    };
    for stmt in &statements {
        match session.execute(stmt) {
            Ok(Some(value)) => println!("=> {value}\n"),
            Ok(None) => println!(),
            Err(Fault::Compile(error)) => {
                eprintln!("{error}");
                return ExitCode::from(EX_DATAERR);
            }
            Err(Fault::Runtime(error)) => {
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
fn run_repl(options: &Options) {
    let stdin = io::stdin();
    let mut session = Session::new(options);
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

        match session.run(&source, false) {
            Err(Failure::Incomplete(_)) if !blank => continue,
            Err(Failure::Incomplete(errors)) => report(&errors, false),
            _ => {}
        }
        source.clear();
    }
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
