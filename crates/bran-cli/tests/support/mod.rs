use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn output(command: &mut Command) -> io::Result<Output> {
    output_with_timeout(command, Duration::from_secs(30))
}

pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "bran-test-output-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir(&path)?;
    let scratch = Scratch(path);
    let stdout = scratch.0.join("stdout");
    let stderr = scratch.0.join("stderr");
    command
        .stdin(Stdio::null())
        .stdout(File::create(&stdout)?)
        .stderr(File::create(&stderr)?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let spawned = command.spawn();
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = spawned?;
    let deadline = Instant::now() + timeout;
    let error = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Output {
                    status,
                    stdout: fs::read(stdout)?,
                    stderr: fs::read(stderr)?,
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => break io::Error::new(io::ErrorKind::TimedOut, "test child timed out"),
            Err(error) => break error,
        }
    };
    #[cfg(unix)]
    // SAFETY: the unreaped child owns the process group configured above.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(error)
}
