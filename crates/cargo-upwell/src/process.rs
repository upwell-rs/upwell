use std::io::Write as _;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use command_group::CommandGroup as _;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);
const CAPTURE_DRAIN_IDLE_LIMIT: Duration = Duration::from_millis(100);

pub(crate) const CARGO_STDOUT_LIMIT: usize = 128 * 1024 * 1024;
pub(crate) const CARGO_STDERR_LIMIT: usize = 16 * 1024 * 1024;
pub(crate) const PROBE_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

/// Cooperative cancellation shared between tooling callers and child-process work.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Requests cancellation of current or future work using this token.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Portable child-process completion status exposed to terminal and IDE consumers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessStatus {
    /// Whether the child reported successful completion.
    pub success: bool,
    /// Numeric exit code when the platform exposes one.
    pub code: Option<i32>,
}

impl From<ExitStatus> for ProcessStatus {
    fn from(status: ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.code(),
        }
    }
}

pub(crate) struct ProcessOutput {
    pub(crate) status: ProcessStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) cancelled: bool,
}

#[derive(Debug)]
pub(crate) enum ProcessExecutionError {
    Spawn(std::io::Error),
    Wait(std::io::Error),
    Kill(std::io::Error),
    Capture(std::io::Error),
    CapturePanic,
}

pub(crate) fn execute(
    command: &mut Command,
    cancellation: &CancellationToken,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<ProcessOutput, ProcessExecutionError> {
    execute_with_stderr(command, cancellation, stdout_limit, stderr_limit, false)
}

pub(crate) fn execute_with_stderr(
    command: &mut Command,
    cancellation: &CancellationToken,
    stdout_limit: usize,
    stderr_limit: usize,
    mirror_stderr: bool,
) -> Result<ProcessOutput, ProcessExecutionError> {
    execute_with_mirror(
        command,
        cancellation,
        stdout_limit,
        stderr_limit,
        mirror_stderr.then_some(MirrorOutput::Stderr),
    )
}

#[cfg(all(test, unix))]
pub(crate) fn execute_with_mirror_writer(
    command: &mut Command,
    cancellation: &CancellationToken,
    stdout_limit: usize,
    stderr_limit: usize,
    writer: Box<dyn std::io::Write + Send>,
) -> Result<ProcessOutput, ProcessExecutionError> {
    execute_with_mirror(
        command,
        cancellation,
        stdout_limit,
        stderr_limit,
        Some(MirrorOutput::Writer(writer)),
    )
}

fn execute_with_mirror(
    command: &mut Command,
    cancellation: &CancellationToken,
    stdout_limit: usize,
    stderr_limit: usize,
    mirror_stderr: Option<MirrorOutput>,
) -> Result<ProcessOutput, ProcessExecutionError> {
    let capture_complete = Arc::new(AtomicBool::new(false));
    let mut cancelled = false;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if cancellation.is_cancelled() {
        return Ok(ProcessOutput {
            status: ProcessStatus {
                success: false,
                code: None,
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            cancelled: true,
        });
    }

    let mut child = command
        .group_spawn()
        .map_err(ProcessExecutionError::Spawn)?;
    let stdout_thread = capture(
        child
            .inner()
            .stdout
            .take()
            .expect("piped child stdout is available after spawn"),
        stdout_limit,
        Arc::clone(&capture_complete),
        None,
    );
    let stderr_thread = capture(
        child
            .inner()
            .stderr
            .take()
            .expect("piped child stderr is available after spawn"),
        stderr_limit,
        Arc::clone(&capture_complete),
        mirror_stderr,
    );

    let monitored = loop {
        if cancellation.is_cancelled() {
            cancelled = true;

            match child.kill() {
                Ok(()) => break child.wait().map_err(ProcessExecutionError::Wait),
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                    break child.wait().map_err(ProcessExecutionError::Wait);
                }
                Err(error) => {
                    terminate_group(&mut child);

                    break Err(ProcessExecutionError::Kill(error));
                }
            }
        }

        let completed = match child.try_wait() {
            Ok(completed) => completed,
            Err(error) => {
                terminate_group(&mut child);

                break Err(ProcessExecutionError::Wait(error));
            }
        };

        if let Some(completed) = completed {
            terminate_group(&mut child);

            break Ok(completed);
        }

        std::thread::sleep(PROCESS_POLL_INTERVAL);
    };

    capture_complete.store(true, Ordering::Release);

    let stdout = join_capture(stdout_thread);
    let stderr = join_capture(stderr_thread);
    let status = monitored?;
    let stdout = stdout?;
    let stderr = stderr?;

    Ok(ProcessOutput {
        status: status.into(),
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        cancelled,
    })
}

fn terminate_group(child: &mut command_group::GroupChild) {
    if let Err(error) = child.kill()
        && !matches!(
            error.kind(),
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
        )
    {
        return;
    }

    let _ = child.wait();
}

struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

enum MirrorOutput {
    Stderr,
    #[cfg(all(test, unix))]
    Writer(Box<dyn std::io::Write + Send>),
}

impl MirrorOutput {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Stderr => {
                let mut stderr = std::io::stderr().lock();

                stderr.write_all(bytes)?;
                stderr.flush()
            }
            #[cfg(all(test, unix))]
            Self::Writer(writer) => {
                writer.write_all(bytes)?;
                writer.flush()
            }
        }
    }
}

fn capture(
    mut reader: impl CaptureReader,
    limit: usize,
    complete: Arc<AtomicBool>,
    mut mirror: Option<MirrorOutput>,
) -> JoinHandle<std::io::Result<CapturedOutput>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
        let mut buffer = [0_u8; 16 * 1024];
        let mut drain_idle_since = None;
        let mut truncated = false;

        reader.configure_capture()?;

        loop {
            let read = match reader.read(&mut buffer) {
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if complete.load(Ordering::Acquire) {
                        let idle_since =
                            drain_idle_since.get_or_insert_with(std::time::Instant::now);

                        if idle_since.elapsed() >= CAPTURE_DRAIN_IDLE_LIMIT {
                            break;
                        }
                    }

                    std::thread::sleep(PROCESS_POLL_INTERVAL);

                    continue;
                }
                Err(error) => return Err(error),
            };

            if read == 0 {
                break;
            }

            drain_idle_since = None;

            let remaining = limit.saturating_sub(bytes.len());
            let retained = remaining.min(read);

            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;

            if let Some(output) = &mut mirror
                && output.write_all(&buffer[..read]).is_err()
            {
                mirror = None;
            }
        }

        Ok(CapturedOutput { bytes, truncated })
    })
}

trait CaptureReader: std::io::Read + Send + 'static {
    fn configure_capture(&self) -> std::io::Result<()>;
}

#[cfg(unix)]
macro_rules! impl_capture_reader {
    ($type:ty) => {
        impl CaptureReader for $type {
            #[allow(unsafe_code)]
            fn configure_capture(&self) -> std::io::Result<()> {
                use std::os::fd::AsRawFd as _;

                let file_descriptor = self.as_raw_fd();
                // SAFETY: `file_descriptor` is the open descriptor of `self`, and `F_GETFL` takes
                // no further argument.
                let flags = unsafe { libc::fcntl(file_descriptor, libc::F_GETFL) };

                if flags == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                // SAFETY: `file_descriptor` is still the open descriptor of `self`, and `F_SETFL`
                // takes an int of flags.
                if unsafe { libc::fcntl(file_descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) }
                    == -1
                {
                    return Err(std::io::Error::last_os_error());
                }

                Ok(())
            }
        }
    };
}

#[cfg(not(unix))]
macro_rules! impl_capture_reader {
    ($type:ty) => {
        impl CaptureReader for $type {
            fn configure_capture(&self) -> std::io::Result<()> {
                Ok(())
            }
        }
    };
}

impl_capture_reader!(std::process::ChildStdout);
impl_capture_reader!(std::process::ChildStderr);

fn join_capture(
    thread: JoinHandle<std::io::Result<CapturedOutput>>,
) -> Result<CapturedOutput, ProcessExecutionError> {
    thread
        .join()
        .map_err(|_| ProcessExecutionError::CapturePanic)?
        .map_err(ProcessExecutionError::Capture)
}

#[cfg(test)]
mod tests;
