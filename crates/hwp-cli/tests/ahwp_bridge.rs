//! Phase-1 command rename: canonical output and a terminal-only legacy notice.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn primary() -> PathBuf {
    // The fallback lets these assertions run against the pre-rename binary too.
    option_env!("CARGO_BIN_EXE_ahwp")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_hwp")))
}

struct Alias(PathBuf);

impl Alias {
    fn new(label: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("ahwp-bridge-{}-{label}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join(if cfg!(windows) { "hwp.exe" } else { "hwp" });
        std::fs::copy(primary(), &executable).unwrap();
        Self(executable)
    }
}

impl Drop for Alias {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}

fn run(executable: &Path, arguments: &[&str]) -> Output {
    Command::new(executable)
        .env("HWP_LANG", "en")
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn canonical_version_and_help_name_ahwp() {
    let version = run(&primary(), &["--version"]);
    assert!(version.status.success());
    assert!(
        version.stdout.starts_with(b"ahwp "),
        "canonical version must name ahwp: {}",
        String::from_utf8_lossy(&version.stdout)
    );
    let help = run(&primary(), &["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage: ahwp"));
}

#[test]
fn legacy_non_terminal_output_matches_the_canonical_command() {
    let alias = Alias::new("non-terminal");
    for arguments in [
        vec!["--version"],
        vec!["--help"],
        vec!["invalid-subcommand"],
    ] {
        let canonical = run(&primary(), &arguments);
        let legacy = run(&alias.0, &arguments);
        assert_eq!(canonical.status.code(), legacy.status.code());
        assert_eq!(canonical.stdout, legacy.stdout, "stdout: {arguments:?}");
        assert_eq!(canonical.stderr, legacy.stderr, "stderr: {arguments:?}");
    }
}

#[cfg(unix)]
#[test]
fn legacy_terminal_prints_one_deprecation_line() {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let alias = Alias::new("terminal");
    let mut master = -1;
    let mut slave = -1;
    // Both descriptors are immediately owned by Files, including on a later assertion failure.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
    let mut reader = unsafe { std::fs::File::from_raw_fd(master) };
    let terminal = unsafe { std::fs::File::from_raw_fd(slave) };
    // Darwin can discard unread output when the last slave closes. Keep the parent copy
    // alive until the master is drained, even after the child has exited.
    let parent_terminal = terminal.try_clone().unwrap();
    let output = Command::new(&alias.0)
        .env("HWP_LANG", "en")
        .arg("--version")
        .stderr(Stdio::from(terminal))
        .output()
        .unwrap();
    assert!(output.status.success());
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "PTY output did not quiesce within five seconds; inspect descriptor ownership \
             and rerun legacy_terminal_prints_one_deprecation_line"
        );
        // The child has been reaped. After receiving output, a quiet interval completes the
        // drain; the retained slave means EOF cannot be used as the completion signal.
        let wait = if bytes.is_empty() {
            remaining
        } else {
            remaining.min(Duration::from_millis(100))
        };
        let mut descriptor = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready =
            unsafe { libc::poll(&mut descriptor, 1, wait.as_millis().max(1) as libc::c_int) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            panic!("poll terminal: {error}; inspect PTY descriptors and rerun this test");
        }
        if ready == 0 || descriptor.revents & libc::POLLIN == 0 {
            break;
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => bytes.extend_from_slice(&buffer[..length]),
            // Linux returns EIO after the last slave closes; macOS returns EOF.
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("read terminal: {error}"),
        }
    }
    drop(parent_terminal);
    let stderr = String::from_utf8(bytes).unwrap();
    let lines = stderr.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one notice on terminal stderr: {stderr:?}");
    assert!(lines[0].contains("deprecated"), "{stderr:?}");
    assert!(lines[0].contains("ahwp"), "{stderr:?}");
    assert!(lines[0].contains("2.0"), "{stderr:?}");
}
