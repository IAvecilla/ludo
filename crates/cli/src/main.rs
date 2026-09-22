//! REPL.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use ludo_core::parser::parse_expr;
use ludo_core::scanner::scan_tokens;
const EX_USAGE: u8 = 64;
const EX_DATAERR: u8 = 65;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.len() {
        0 => {
            run_repl();
            ExitCode::SUCCESS
        }
        1 => run_file(&args[0]),
        _ => {
            eprintln!("usage: ludo [program.ludo]");
            ExitCode::from(EX_USAGE)
        }
    }
}

fn run_file(path: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("could not read `{path}`: {err}");
            return ExitCode::from(EX_USAGE);
        }
    };

    let mut ok = true;
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        println!("> {line}");
        if !run(line) {
            ok = false;
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EX_DATAERR)
    }
}

fn run_repl() {
    let stdin = io::stdin();

    loop {
        print!("> ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                eprintln!("read error: {err}");
                break;
            }
        }

        let line = line.trim();
        if !line.is_empty() {
            run(line);
        }
    }
}

fn run(source: &str) -> bool {
    let tokens = match scan_tokens(source) {
        Ok(tokens) => tokens,
        Err(errors) => {
            for error in &errors {
                eprintln!("Scanning Error: {}", error.message);
            }
            return false;
        }
    };

    match parse_expr(tokens) {
        Ok(expr) => {
            println!("{expr}");
            true
        }
        Err(error) => {
            eprintln!("Parsing Error: {}", error.message);
            false
        }
    }
}
