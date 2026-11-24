//! Runs the `ludo` binary on the examples, as a user would.

use std::process::Command;

#[test]
fn a_game_file_runs_with_the_host_prelude_on_both_backends() {
    for backend in [&[][..], &["--interpreter"]] {
        for (name, printed) in [
            ("pong.ludo", "This is a game file to play pong\n"),
            ("square.ludo", ""),
        ] {
            let path = format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"));
            let output = Command::new(env!("CARGO_BIN_EXE_ludo"))
                .arg("run")
                .args(backend)
                .arg(path)
                .output()
                .expect("ludo should start");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success() && stderr.is_empty(), "{stderr}");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                printed,
                "{backend:?}"
            );
        }
    }
}
