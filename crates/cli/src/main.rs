use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use ludo_core::interpreter::Interpreter;
use ludo_core::parser::{parse, ParseError};
use ludo_core::scanner::scan_tokens;
use ludo_play::Game;

const EX_USAGE: u8 = 64;
const EX_DATAERR: u8 = 65;
const EX_SOFTWARE: u8 = 70;

enum Failure {
    Static,
    Runtime,
    Incomplete(Vec<ParseError>),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match args.as_slice() {
        [] => {
            run_repl();
            ExitCode::SUCCESS
        }
        ["run", path] => read(path).map_or_else(|code| code, |source| run_file(&source)),
        ["play", path] => read(path).map_or_else(|code| code, |source| play(path, &source)),
        _ => {
            eprintln!("usage: ludo              start the REPL");
            eprintln!("       ludo run FILE     run a program");
            eprintln!("       ludo play FILE    play a game file in a window");
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

fn run_file(source: &str) -> ExitCode {
    match run(source, &mut Interpreter::new(), true) {
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

fn run_repl() {
    let stdin = io::stdin();
    let mut interpreter = Interpreter::new();
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

        match run(&source, &mut interpreter, false) {
            Err(Failure::Incomplete(_)) if !blank => continue,
            Err(Failure::Incomplete(errors)) => report(&errors, false),
            _ => {}
        }
        source.clear();
    }
}

fn run(source: &str, interpreter: &mut Interpreter, file: bool) -> Result<(), Failure> {
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

    for stmt in &statements {
        match interpreter.execute(stmt) {
            Ok(Some(value)) if !file => println!("{value}"),
            Ok(_) => {}
            Err(error) => {
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
