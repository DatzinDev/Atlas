#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn configuration_errors_do_not_echo_input() {
    for args in [
        vec![],
        vec!["serve", "--unknown", "secret-token"],
        vec!["serve", "--listen", "postgres://user:password@host/db"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("startup_failed"));
        assert!(!error.contains("secret-token"));
        assert!(!error.contains("password"));
    }
}

#[test]
fn http_startup_and_shutdown() {
    for signal in ["-TERM", "-INT"] {
        let mut server = Server(
            Command::new(env!("CARGO_BIN_EXE_atlas"))
                .args(["serve", "--listen", "127.0.0.1:0"])
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let stderr = server.0.stderr.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let logs = thread::spawn(move || {
            let mut lines = BufReader::new(stderr).lines();
            let first = lines.next().unwrap().unwrap();
            sender.send(first).unwrap();
            lines.map(Result::unwrap).collect::<Vec<_>>().join("\n")
        });
        let listening = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        let address = listening
            .strip_prefix("{\"event\":\"listening\",\"address\":\"")
            .unwrap()
            .strip_suffix("\"}")
            .unwrap();
        for (path, status) in [("health", "200"), ("ready", "503"), ("datasets", "404")] {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(
                stream,
                "GET /api/v1/{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {status}")));
        }
        let occupied = Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--listen", address])
            .output()
            .unwrap();
        assert!(!occupied.status.success());
        assert!(
            String::from_utf8(occupied.stderr)
                .unwrap()
                .contains("cannot bind listener")
        );
        assert!(
            Command::new("kill")
                .args([signal, &server.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = server.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "shutdown timed out");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(logs.join().unwrap().contains("shutdown_complete"));
    }
}
