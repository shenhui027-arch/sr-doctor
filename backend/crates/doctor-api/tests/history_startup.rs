//! Capture the actual process stderr, including the startup error path.
use std::{
    net::TcpListener,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn task21_history_startup_stderr_contains_one_stable_code_without_private_details() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "doctor-task21-stderr-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("PRIVATE_PATH_SENTINEL.sqlite");
    std::fs::write(&path, b"PRIVATE_SQL_SENTINEL: not a SQLite database").unwrap();
    // Keep this port occupied so the process exits after HistoryRuntime::open,
    // without starting a long-lived API server or needing a timed kill.
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_doctor-api"))
        .env("DOCTOR_HISTORY", "on")
        .env("DOCTOR_HISTORY_PATH", &path)
        .env(
            "DOCTOR_PORT",
            listener.local_addr().unwrap().port().to_string(),
        )
        .env_remove("DOCTOR_DEPLOYMENT_MODE")
        .env_remove("DOCTOR_PUBLIC_PORT")
        .output()
        .unwrap();
    drop(listener);
    assert!(
        !output.status.success(),
        "the occupied port must terminate startup"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    let diagnostics = stderr
        .lines()
        .filter(|line| line.starts_with("history_store_unavailable"))
        .collect::<Vec<_>>();
    assert_eq!(
        diagnostics,
        ["history_store_unavailable code=history_store_open_failed"]
    );
    assert!(!stderr.contains("PRIVATE_"));
    assert!(!stderr.contains("not a database"));
    std::fs::remove_dir_all(dir).unwrap();
}
