use std::{
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

use tempfile::TempDir;

struct DaemonProcess {
    child: Child,
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn cli_and_daemon_exchange_nota_over_rkyv_socket() {
    let temp = TempDir::new().expect("tempdir");
    let socket_path = temp.path().join("spirit-next.sock");
    let socket_text = socket_path.to_string_lossy().into_owned();

    let daemon = Command::new(env!("CARGO_BIN_EXE_spirit-next-daemon"))
        .arg(format!("[{socket_text}]"))
        .spawn()
        .expect("spawn daemon");
    let _daemon = DaemonProcess { child: daemon };
    wait_for_socket(&socket_path);

    let record = Command::new(env!("CARGO_BIN_EXE_spirit-next"))
        .env("SPIRIT_NEXT_SOCKET", &socket_path)
        .arg("(Record ([schema] Constraint [schema creates the interface] Maximum))")
        .output()
        .expect("run record cli");
    assert!(
        record.status.success(),
        "record stderr: {}",
        String::from_utf8_lossy(&record.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&record.stdout).trim(),
        "(RecordAccepted (1 (1 39)))"
    );

    let observe = Command::new(env!("CARGO_BIN_EXE_spirit-next"))
        .env("SPIRIT_NEXT_SOCKET", &socket_path)
        .arg("(Observe ([schema] Constraint))")
        .output()
        .expect("run observe cli");
    assert!(
        observe.status.success(),
        "observe stderr: {}",
        String::from_utf8_lossy(&observe.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&observe.stdout).trim(),
        "(RecordsObserved (([schema] Constraint [schema creates the interface] Maximum) (1 39)))"
    );

    let rejected = Command::new(env!("CARGO_BIN_EXE_spirit-next"))
        .env("SPIRIT_NEXT_SOCKET", &socket_path)
        .arg("(Record ([] Constraint [schema rejects before SEMA] Maximum))")
        .output()
        .expect("run rejected record cli");
    assert!(
        rejected.status.success(),
        "rejected stderr: {}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&rejected.stdout).trim(),
        "(Rejected (EmptyTopic (1 39)))"
    );
}

fn wait_for_socket(path: &std::path::Path) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("socket did not appear at {}", path.display());
}
