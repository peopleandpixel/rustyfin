//! The server binary as a process: how it stops on SIGTERM (`docker stop`, systemd) and how it
//! treats a `config.json` it can't parse.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// A fresh data directory for one test, removed when it ends.
struct DataDir(PathBuf);

impl DataDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rf-r3rel-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        DataDir(dir)
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Starts the server on `port` with `data` as its data directory; its output goes to `log`.
fn start(data: &Path, port: u16, log: &Path) -> Child {
    let out = std::fs::File::create(log).unwrap();
    let err = out.try_clone().unwrap();
    Command::new(env!("CARGO_BIN_EXE_rustyfin"))
        .env("RUSTYFIN_DATA", data)
        .env("RUSTYFIN_BIND", format!("127.0.0.1:{port}"))
        .env("RUST_LOG", "info")
        .env_remove("RUSTYFIN_DB")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .unwrap()
}

/// Waits up to `limit` for `child` to exit; kills it and fails when it doesn't.
fn wait_exit(child: &mut Child, limit: Duration, log: &Path) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > limit {
            child.kill().ok();
            child.wait().ok();
            panic!(
                "the server didn't exit within {limit:?}; log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Waits until the server accepts connections on `port`; the probe's connection is closed again
/// at once.
fn wait_listening(child: &mut Child, port: u16, log: &Path) {
    let start_wait = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "the server exited at start ({status}); log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
        if start_wait.elapsed() > Duration::from_secs(120) {
            child.kill().ok();
            child.wait().ok();
            panic!("the server didn't start listening on {port}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn signal(child: &Child, name: &str) {
    let sent = Command::new("kill")
        .args([&format!("-{name}"), &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
}

/// The body of `GET {uri}` over a connection of its own.
fn get(port: u16, uri: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {uri} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn r3rel_sigterm_stops_the_server_gracefully_and_quickly() {
    let data = DataDir::new("sigterm");
    let log = data.0.join("server.log");
    let port = free_port();
    let mut child = start(&data.0, port, &log);
    wait_listening(&mut child, port, &log);

    let sent = Instant::now();
    signal(&child, "TERM");
    // Docker and systemd send SIGKILL 10 s after SIGTERM; with nothing open it is far quicker.
    let status = wait_exit(&mut child, Duration::from_secs(9), &log);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(status.success(), "{status}; log:\n{text}");
    assert!(sent.elapsed() < Duration::from_secs(9));
    assert!(text.contains("SIGTERM received"), "{text}");
    assert!(text.contains("server stopped"), "{text}");
    // The database was closed whole: SQLite removes its WAL at the last close.
    assert!(data.0.join("rustyfin.db").exists());
    assert!(!data.0.join("rustyfin.db-wal").exists(), "{text}");
}

#[test]
fn r3rel_a_sighup_with_a_broken_config_keeps_the_settings_in_use() {
    let data = DataDir::new("sighup");
    let log = data.0.join("server.log");
    let config = data.0.join("config.json");
    std::fs::write(
        &config,
        r#"{"ServerName":"Kept Name","IsStartupWizardCompleted":true}"#,
    )
    .unwrap();
    let port = free_port();
    let mut child = start(&data.0, port, &log);
    wait_listening(&mut child, port, &log);
    assert!(get(port, "/System/Info/Public").contains("\"Kept Name\""));

    let bad = "{\"ServerName\": \"Other Name\",, }";
    std::fs::write(&config, bad).unwrap();
    signal(&child, "HUP");
    let waited = Instant::now();
    while !std::fs::read_to_string(&log)
        .unwrap()
        .contains("the configuration in use is kept")
    {
        assert!(
            waited.elapsed() < Duration::from_secs(10),
            "no reload error; log:\n{}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let info = get(port, "/System/Info/Public");
    assert!(info.contains("\"Kept Name\""), "{info}");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), bad);

    signal(&child, "TERM");
    let status = wait_exit(&mut child, Duration::from_secs(9), &log);
    assert!(status.success(), "{status}");
}

#[test]
fn r3rel_a_config_that_fails_to_parse_stops_the_start_and_is_kept() {
    let data = DataDir::new("bad-config");
    let log = data.0.join("server.log");
    let bad = "{\n  \"ServerName\": \"My Server\",\n  \"IsStartupWizardCompleted\": true,,\n}\n";
    std::fs::write(data.0.join("config.json"), bad).unwrap();

    let mut child = start(&data.0, free_port(), &log);
    let status = wait_exit(&mut child, Duration::from_secs(60), &log);
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(status.code(), Some(1), "{text}");
    assert!(text.contains("config.json"), "{text}");
    assert_eq!(
        std::fs::read_to_string(data.0.join("config.json")).unwrap(),
        bad,
        "the file is left as it was"
    );
}
