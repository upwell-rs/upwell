use std::ffi::OsStr;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;
use upwell_tooling_schema::{
    ProbeEnvelope, ProbeOutcome, TOOLING_PROBE_ARGUMENT, TOOLING_PROBE_BINARY_NAME_ENV,
    TOOLING_PROBE_MANIFEST_PATH_ENV, TOOLING_PROBE_OUTPUT_ENV, TOOLING_PROBE_PACKAGE_NAME_ENV,
    TOOLING_PROBE_PACKAGE_VERSION_ENV, TOOLING_PROBE_TRANSITION_PREVIEW_ENV,
    TransitionPreviewRequest,
};

use crate::process::{PROBE_OUTPUT_LIMIT, ProcessExecutionError, execute};
use crate::{CancellationToken, ProcessStatus, SelectedTarget};

const MAX_PROBE_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
const RUN_DIRECTORY_ATTEMPTS: u16 = 128;
const RESERVED_PROBE_ENV_PREFIX: &str = "UPWELL_TOOLING_PROBE_";
static NEXT_RUN_DIRECTORY: AtomicU64 = AtomicU64::new(0);

/// Target-local process evidence retained independently from the probe envelope.
#[derive(Debug)]
pub struct ProbeEvidence {
    /// Portable target process completion status.
    pub status: ProcessStatus,
    /// Application stdout, never interpreted as probe JSON.
    pub stdout: Vec<u8>,
    /// Application stderr, retained for diagnostics without becoming schema data.
    pub stderr: Vec<u8>,
    /// Whether application stdout exceeded the retained bound.
    pub stdout_truncated: bool,
    /// Whether application stderr exceeded the retained bound.
    pub stderr_truncated: bool,
}

/// Validated target-local tooling probe result.
#[derive(Debug)]
pub struct ProbeResult {
    /// Versioned validated probe envelope.
    pub envelope: ProbeEnvelope,
    /// Independent target process evidence.
    pub evidence: ProbeEvidence,
    /// Cleanup failure retained without replacing a valid application result.
    pub cleanup_error: Option<std::io::Error>,
}

/// Failure while invoking or consuming the private target-local probe contract.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProbeError {
    /// Cancellation was requested while the selected target was running.
    #[error("application tooling probe was cancelled")]
    Cancelled {
        /// Target output emitted before cancellation completed.
        evidence: Box<ProbeEvidence>,
    },
    /// A private probe response directory could not be created.
    #[error("failed to create a private tooling probe directory")]
    CreateDirectory(#[source] std::io::Error),
    /// Every bounded private run-directory name already existed.
    #[error("exhausted private tooling probe directory names")]
    DirectoryExhausted,
    /// The selected application executable could not be launched.
    #[error("failed to launch the selected application tooling probe")]
    Launch(#[source] std::io::Error),
    /// Target process monitoring failed.
    #[error("failed while waiting for the selected application tooling probe")]
    Process(#[source] std::io::Error),
    /// A target output reader thread failed unexpectedly.
    #[error("failed to capture application tooling probe output")]
    Capture,
    /// The selected target exited without publishing its private response file.
    #[error("application tooling probe published no response")]
    MissingResponse {
        /// Target process evidence.
        evidence: Box<ProbeEvidence>,
    },
    /// Response metadata could not be read.
    #[error("failed to inspect the application tooling probe response")]
    ResponseMetadata {
        /// Target process evidence.
        evidence: Box<ProbeEvidence>,
        /// File-system failure.
        #[source]
        source: std::io::Error,
    },
    /// The response exceeds the bounded process protocol size.
    #[error("application tooling probe response exceeds {limit} bytes")]
    ResponseTooLarge {
        /// Actual response size.
        size: u64,
        /// Accepted response size.
        limit: u64,
        /// Target process evidence.
        evidence: Box<ProbeEvidence>,
    },
    /// Response bytes could not be read.
    #[error("failed to read the application tooling probe response")]
    ReadResponse {
        /// Target process evidence.
        evidence: Box<ProbeEvidence>,
        /// File-system failure.
        #[source]
        source: std::io::Error,
    },
    /// Response bytes are not UTF-8 JSON text.
    #[error("application tooling probe response is not UTF-8")]
    ResponseEncoding {
        /// Target process evidence.
        evidence: Box<ProbeEvidence>,
        /// Encoding failure.
        #[source]
        source: std::string::FromUtf8Error,
    },
    /// Response JSON violates the versioned tooling probe schema.
    #[error("application tooling probe response is invalid")]
    Decode {
        /// Target process evidence.
        evidence: Box<ProbeEvidence>,
        /// Schema decode or validation failure.
        #[source]
        source: upwell_tooling_schema::ProbeDecodeError,
    },
    /// A valid envelope does not belong to the selected Cargo target.
    #[error("application tooling probe response identifies a different Cargo target")]
    TargetIdentityMismatch {
        /// Validated response envelope.
        envelope: Box<ProbeEnvelope>,
        /// Target process evidence.
        evidence: Box<ProbeEvidence>,
    },
    /// A valid envelope disagrees with the generated process exit contract.
    #[error("application tooling probe response disagrees with its process status")]
    StatusMismatch {
        /// Validated response envelope.
        envelope: Box<ProbeEnvelope>,
        /// Target process evidence.
        evidence: Box<ProbeEvidence>,
    },
}

/// Executes and validates one built target's generated private tooling probe, previewing
/// `transition_preview` against the prepared application when one is supplied.
pub fn execute_probe(
    executable: &Path,
    workspace_target_directory: &Path,
    selected: &SelectedTarget,
    current_dir: Option<&Path>,
    transition_preview: Option<&TransitionPreviewRequest>,
    cancellation: &CancellationToken,
) -> Result<ProbeResult, ProbeError> {
    let run_directory = ProbeDirectory::create(workspace_target_directory)?;
    let response = run_directory.path.join("response.json");
    let mut command = Command::new(executable);

    command.arg(TOOLING_PROBE_ARGUMENT);
    scrub_reserved_environment(&mut command);
    command
        .env(TOOLING_PROBE_OUTPUT_ENV, &response)
        .env(TOOLING_PROBE_PACKAGE_NAME_ENV, &selected.package_name)
        .env(TOOLING_PROBE_PACKAGE_VERSION_ENV, &selected.package_version)
        .env(TOOLING_PROBE_MANIFEST_PATH_ENV, &selected.manifest_path)
        .env(TOOLING_PROBE_BINARY_NAME_ENV, &selected.binary_name);

    if let Some(request) = transition_preview {
        command.env(TOOLING_PROBE_TRANSITION_PREVIEW_ENV, request.to_env_value());
    }

    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }

    let output = execute(
        &mut command,
        cancellation,
        PROBE_OUTPUT_LIMIT,
        PROBE_OUTPUT_LIMIT,
    )
    .map_err(map_process_error)?;

    let evidence = ProbeEvidence {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
    };

    if output.cancelled {
        return Err(ProbeError::Cancelled {
            evidence: Box::new(evidence),
        });
    }

    let bytes = read_response(&response, &evidence)?;
    let json = match String::from_utf8(bytes) {
        Ok(json) => json,
        Err(source) => {
            return Err(ProbeError::ResponseEncoding {
                evidence: Box::new(evidence),
                source,
            });
        }
    };
    let envelope = match ProbeEnvelope::from_json(json.trim_end()) {
        Ok(envelope) => envelope,
        Err(source) => {
            return Err(ProbeError::Decode {
                evidence: Box::new(evidence),
                source,
            });
        }
    };

    if !matches_selected_target(&envelope, selected) {
        return Err(ProbeError::TargetIdentityMismatch {
            envelope: Box::new(envelope),
            evidence: Box::new(evidence),
        });
    }

    let status_matches = match &envelope.outcome {
        ProbeOutcome::Success { .. } => evidence.status.success,
        ProbeOutcome::Failure { .. } => evidence.status.code == Some(1),
    };

    if !status_matches {
        return Err(ProbeError::StatusMismatch {
            envelope: Box::new(envelope),
            evidence: Box::new(evidence),
        });
    }

    let cleanup_error = run_directory.close().err();

    Ok(ProbeResult {
        envelope,
        evidence,
        cleanup_error,
    })
}

fn read_response(path: &Path, evidence: &ProbeEvidence) -> Result<Vec<u8>, ProbeError> {
    let mut file = match open_response(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProbeError::MissingResponse {
                evidence: Box::new(clone_evidence(evidence)),
            });
        }
        Err(source) => {
            return Err(ProbeError::ReadResponse {
                evidence: Box::new(clone_evidence(evidence)),
                source,
            });
        }
    };
    let metadata = file
        .metadata()
        .map_err(|source| ProbeError::ResponseMetadata {
            evidence: Box::new(clone_evidence(evidence)),
            source,
        })?;

    if !metadata.is_file() {
        return Err(ProbeError::ResponseMetadata {
            evidence: Box::new(clone_evidence(evidence)),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "probe response is not a regular file",
            ),
        });
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len().min(MAX_PROBE_RESPONSE_BYTES)).unwrap_or(0),
    );
    let mut bounded = (&mut file).take(MAX_PROBE_RESPONSE_BYTES + 1);

    bounded
        .read_to_end(&mut bytes)
        .map_err(|source| ProbeError::ReadResponse {
            evidence: Box::new(clone_evidence(evidence)),
            source,
        })?;

    if bytes.len() as u64 > MAX_PROBE_RESPONSE_BYTES {
        return Err(ProbeError::ResponseTooLarge {
            size: bytes.len() as u64,
            limit: MAX_PROBE_RESPONSE_BYTES,
            evidence: Box::new(clone_evidence(evidence)),
        });
    }

    Ok(bytes)
}

fn matches_selected_target(envelope: &ProbeEnvelope, selected: &SelectedTarget) -> bool {
    let package = envelope.identity.package.as_ref();
    let binary = envelope.identity.binary.as_ref();

    package.is_some_and(|package| {
        package.name == selected.package_name
            && package.version.as_deref() == Some(selected.package_version.as_str())
            && package.manifest_path.as_deref() == selected.manifest_path.to_str()
    }) && binary.is_some_and(|binary| binary.name == selected.binary_name)
}

fn clone_evidence(evidence: &ProbeEvidence) -> ProbeEvidence {
    ProbeEvidence {
        status: evidence.status,
        stdout: evidence.stdout.clone(),
        stderr: evidence.stderr.clone(),
        stdout_truncated: evidence.stdout_truncated,
        stderr_truncated: evidence.stderr_truncated,
    }
}

fn scrub_reserved_environment(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if starts_with_reserved_prefix(&name) {
            command.env_remove(name);
        }
    }
}

#[cfg(windows)]
fn starts_with_reserved_prefix(name: &OsStr) -> bool {
    name.to_string_lossy()
        .to_ascii_uppercase()
        .starts_with(RESERVED_PROBE_ENV_PREFIX)
}

#[cfg(not(windows))]
fn starts_with_reserved_prefix(name: &OsStr) -> bool {
    name.as_encoded_bytes()
        .starts_with(RESERVED_PROBE_ENV_PREFIX.as_bytes())
}

fn map_process_error(error: ProcessExecutionError) -> ProbeError {
    match error {
        ProcessExecutionError::Spawn(source) => ProbeError::Launch(source),
        ProcessExecutionError::Wait(source)
        | ProcessExecutionError::Kill(source)
        | ProcessExecutionError::Capture(source) => ProbeError::Process(source),
        ProcessExecutionError::CapturePanic => ProbeError::Capture,
    }
}

struct ProbeDirectory {
    path: PathBuf,
    closed: bool,
}

impl ProbeDirectory {
    fn create(workspace_target_directory: &Path) -> Result<Self, ProbeError> {
        let root = workspace_target_directory.join("upwell/probes");

        create_directory_all(&root).map_err(ProbeError::CreateDirectory)?;

        for _ in 0..RUN_DIRECTORY_ATTEMPTS {
            let ordinal = NEXT_RUN_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("{}-{ordinal}", std::process::id()));

            match create_private_directory(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        closed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(ProbeError::CreateDirectory(error)),
            }
        }

        Err(ProbeError::DirectoryExhausted)
    }

    fn close(mut self) -> std::io::Result<()> {
        std::fs::remove_dir_all(&self.path)?;
        self.closed = true;

        Ok(())
    }
}

impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        if !self.closed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn create_directory_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(unix)]
fn open_response(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(windows)]
fn open_response(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_response(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = std::fs::DirBuilder::new();

    builder.mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(test)]
mod tests;
