use std::fmt::Display;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use ludo_core::ast::Stmt;
use ludo_core::checker::Checker;
use ludo_core::chunk::Chunk;
use ludo_core::compiler::compile;
use ludo_core::parser::{parse, ParseError};
use ludo_core::runtime::RuntimeError;
use ludo_core::scanner::scan_tokens;
use ludo_core::value::Value;
use ludo_core::vm::Vm;
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
    /// `--trace`: show the bytecode of each input of the REPL and trace it as
    /// it runs.
    trace: bool,
    /// `--frames=N`: play a game for N frames without a window, to time it.
    frames: Option<usize>,
}

impl Options {
    /// Reads the flags of a command, or of the REPL if there is none, and
    /// gives nothing if one of them does not apply to it.
    fn parse(command: Option<&str>, flags: &[&str]) -> Option<Options> {
        let mut options = Options::default();
        for flag in flags {
            match (*flag, command) {
                ("--trace", None) => options.trace = true,
                (_, Some("play")) => {
                    options.frames = Some(flag.strip_prefix("--frames=")?.parse().ok()?)
                }
                _ => return None,
            }
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
    let command = match args.as_slice() {
        [] => None,
        [command @ ("run" | "check" | "play" | "dis"), path] => Some((*command, *path)),
        _ => return usage(),
    };
    let Some(options) = Options::parse(command.map(|(command, _)| command), &flags) else {
        return usage();
    };
    let Some((command, path)) = command else {
        run_repl(&options);
        return ExitCode::SUCCESS;
    };
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("could not read `{path}`: {err}");
            return ExitCode::from(EX_USAGE);
        }
    };
    match command {
        "run" => exit(Session::new(&options, true).run(&source, |_| {})),
        "check" => exit(Session::new(&options, true).check(&source).map(drop)),
        "dis" => disassemble(&source),
        _ => play(path, &source, &options),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: ludo [--trace]               start the REPL");
    eprintln!("       ludo run FILE                run a program");
    eprintln!("       ludo check FILE              check a program without running it");
    eprintln!("       ludo play [--frames=N] FILE  play a game file in a window");
    eprintln!("       ludo dis FILE                compile to bytecode and trace it on the VM");
    eprintln!();
    eprintln!("Programs compile to bytecode and run on the VM. With --trace the REPL");
    eprintln!("shows the bytecode of each input and traces it as it runs.");
    eprintln!("--frames=N plays a game for N frames without a window, for the benchmarks.");
    ExitCode::from(EX_USAGE)
}

fn exit(result: Result<(), Failure>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Runtime) => ExitCode::from(EX_SOFTWARE),
        Err(_) => ExitCode::from(EX_DATAERR),
    }
}

/// A VM and a checker that last as long as a program, with the host
/// prelude loaded so a game file sees what `play` gives it.
struct Session {
    vm: Vm,
    checker: Checker,
    /// Whether it runs a file, whose errors say their line, and not the REPL.
    file: bool,
}

impl Session {
    fn new(options: &Options, file: bool) -> Self {
        let mut session = Session {
            vm: Vm::new(),
            checker: Checker::new(),
            file,
        };
        if session.run(HOST, |_| {}).is_err() {
            unreachable!("the host prelude is valid");
        }
        session.vm.trace = options.trace;
        session
    }

    fn error(&self, line: Option<usize>, kind: impl Display, message: &str) {
        let at = match line {
            Some(line) if self.file => format!("[line {line}] "),
            _ => String::new(),
        };
        eprintln!("{at}{kind} Error: {message}");
    }

    fn parse_errors(&self, errors: &[ParseError]) {
        for error in errors {
            self.error(Some(error.line), "Parsing", &error.message);
        }
    }

    /// Checks a program and, if it has no errors, runs it, giving `show` what
    /// each statement leaves.
    fn run(&mut self, source: &str, show: impl Fn(Option<Value>)) -> Result<(), Failure> {
        for stmt in &self.check(source)? {
            match self.execute(stmt) {
                Ok(value) => show(value),
                Err(error) => {
                    self.error(error.line, "Runtime", &error.message);
                    return Err(Failure::Runtime);
                }
            }
        }
        Ok(())
    }

    /// Scans, parses and checks a program, giving its statements. In the REPL
    /// an input that ends too soon is incomplete, to go on with the next line.
    fn check(&mut self, source: &str) -> Result<Vec<Stmt>, Failure> {
        let tokens = scan_tokens(source).map_err(|errors| {
            for error in &errors {
                self.error(Some(error.line), "Scanning", &error.message);
            }
            Failure::Static
        })?;
        let statements = match parse(tokens) {
            Ok(statements) => statements,
            Err(errors) if !self.file && errors.last().is_some_and(|e| e.at_end) => {
                return Err(Failure::Incomplete(errors));
            }
            Err(errors) => {
                self.parse_errors(&errors);
                return Err(Failure::Static);
            }
        };
        if let Err(errors) = self.checker.check(&statements) {
            for error in &errors {
                self.error(Some(error.line), error.kind, &error.message);
            }
            return Err(Failure::Static);
        }
        Ok(statements)
    }

    /// Runs a statement. With the trace on, it first shows its bytecode and
    /// that of the function or sequence it declares.
    fn execute(&mut self, stmt: &Stmt) -> Result<Option<Value>, RuntimeError> {
        if !self.vm.trace {
            return self.vm.interpret(stmt);
        }
        let script = compile(stmt);
        println!("== {stmt} ==\n{}", script.chunk);
        functions(&script.chunk);
        self.vm.run(script)
    }
}

/// Shows the bytecode of every function in a chunk: the function or sequence
/// a statement declares, and those written inside it, with the locals
/// each of those keeps.
fn functions(chunk: &Chunk) {
    for constant in &chunk.constants {
        if let Value::Function(function) = constant {
            match function.captured.as_slice() {
                [] => println!("== {function} ==\n{}", function.chunk),
                kept => println!(
                    "== {function} keeping {} ==\n{}",
                    kept.join(", "),
                    function.chunk
                ),
            }
            functions(&function.chunk);
        }
    }
}

fn disassemble(source: &str) -> ExitCode {
    let options = Options {
        trace: true,
        ..Options::default()
    };
    exit(
        Session::new(&options, true).run(source, |value| match value {
            Some(value) => println!("=> {value}\n"),
            None => println!(),
        }),
    )
}

fn play(path: &str, source: &str, options: &Options) -> ExitCode {
    let mut game = match Game::load(source) {
        Ok(game) => game,
        Err(errors) => {
            for error in errors {
                eprintln!("{error}");
            }
            return ExitCode::from(EX_DATAERR);
        }
    };
    let Some(frames) = options.frames else {
        ludo_play::play(game, path.to_string());
        return ExitCode::SUCCESS;
    };
    // Without a window and with no key down, as fast as it can: each frame
    // runs `update`, the sequences and `draw`. It is what the benchmarks time.
    for _ in 0..frames {
        let frame = game
            .step(&Keys::default(), 1.0 / 60.0)
            .and_then(|()| game.draw().map(drop));
        if let Err(error) = frame {
            eprintln!("{error}");
            return ExitCode::from(EX_SOFTWARE);
        }
    }
    ExitCode::SUCCESS
}

/// Each input is compiled and run on its own, on a VM that lives as long as
/// the REPL, so what one input binds the next ones see, as in the REPL of
/// clox, of Python and of Lua.
fn run_repl(options: &Options) {
    let stdin = io::stdin();
    let mut session = Session::new(options, false);
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

        let show = |value: Option<Value>| {
            if let Some(value) = value {
                println!("{value}");
            }
        };
        match session.run(&source, show) {
            Err(Failure::Incomplete(_)) if !blank => continue,
            Err(Failure::Incomplete(errors)) => session.parse_errors(&errors),
            _ => {}
        }
        source.clear();
    }
}
