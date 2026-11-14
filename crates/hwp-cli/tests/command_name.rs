//! The command is `ahwp`. 2.0 removed the `hwp` compatibility name (#510 T8): a copy of the
//! binary named `hwp` behaves exactly like `ahwp`, with no deprecation notice even on a terminal.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn primary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ahwp"))
}

struct Alias(PathBuf);

impl Alias {
    fn new(label: &str) -> Self {
        // Cargo sets CARGO_TARGET_TMPDIR for integration tests. It lives under target/, so it is
        // on the same file system as the built binary and the hard link below can succeed.
        let directory = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("ahwp-bridge-{}-{label}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join(if cfg!(windows) { "hwp.exe" } else { "hwp" });
        // A hard link writes no data, so no write descriptor to the new executable ever exists.
        // A copy does: this test binary runs its tests on parallel threads, and a sibling
        // thread's fork inherits the copy's write descriptor until it execs, which makes
        // exec of the copy fail with ETXTBSY on Linux (#592). Copy only where linking is not
        // possible (a file system without hard links).
        if std::fs::hard_link(primary(), &executable).is_err() {
            std::fs::copy(primary(), &executable).unwrap();
        }
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
fn hwp_named_copy_output_matches_the_canonical_command() {
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
fn hwp_named_copy_prints_no_notice_on_a_terminal() {
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
             and rerun hwp_named_copy_prints_no_notice_on_a_terminal"
        );
        // The child has been reaped, so anything it wrote is already queued. A short quiet
        // interval completes the drain; the retained slave means EOF cannot be the signal.
        let wait = remaining.min(Duration::from_millis(300));
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
    assert!(
        stderr.is_empty(),
        "an hwp-named binary has no special behavior, even on a terminal: {stderr:?}"
    );
}
