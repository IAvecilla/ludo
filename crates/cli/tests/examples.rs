//! Runs the `ludo` binary on the examples, as a user would.

use std::process::Command;

fn ludo(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ludo"))
        .args(args)
        .output()
        .expect("ludo should start");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn example(name: &str) -> String {
    format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn a_game_file_runs_with_the_host_prelude_on_both_backends() {
    for backend in [None, Some("--interpreter")] {
        let run = |name: &str| {
            let path = example(name);
            let mut args = vec!["run"];
            args.extend(backend);
            args.push(&path);
            ludo(&args)
        };
        assert_eq!(
            run("pong.ludo"),
            (
                true,
                "This is a game file to play pong\n".into(),
                String::new()
            ),
            "{backend:?}"
        );
        assert_eq!(
            run("square.ludo"),
            (true, String::new(), String::new()),
            "{backend:?}"
        );
    }
}
