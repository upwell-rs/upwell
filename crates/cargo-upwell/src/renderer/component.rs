use std::io::Read as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use semver::Version;
use thiserror::Error;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};

use super::{ComponentRenderer, RendererCommand};

const RENDERER_ABI_VERSION: Version = Version::new(0, 1, 0);
const EPOCH_TICK: Duration = Duration::from_millis(10);

wasmtime::component::bindgen!({
    path: "wit/renderer.wit",
    world: "renderer",
});

/// Hard resource limits applied to component loading and each render call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentLimits {
    /// Largest portable component accepted from disk.
    pub component_bytes: usize,
    /// Largest canonical request body transferred into a component.
    pub input_bytes: usize,
    /// Maximum stable resource IDs supplied to one render call.
    pub resources: usize,
    /// Maximum bytes in one resource ID or renderer request metadata field.
    pub metadata_bytes: usize,
    /// Largest response body accepted from a component.
    pub output_bytes: usize,
    /// Maximum instrumented Wasm fuel consumed by instantiation and rendering.
    pub fuel: u64,
    /// Wall-clock execution deadline enforced through Wasmtime epochs.
    pub deadline: Duration,
    /// Wall-clock deadline for isolated component validation and native compilation.
    pub compiler_deadline: Duration,
    /// Maximum address space for the isolated component compiler process.
    pub compiler_memory_bytes: usize,
    /// Maximum bytes per guest linear memory.
    pub memory_bytes: usize,
    /// Maximum number of guest linear memories.
    pub memories: usize,
    /// Maximum elements in each guest table.
    pub table_elements: usize,
    /// Maximum number of guest tables.
    pub tables: usize,
    /// Maximum number of component and core instances.
    pub instances: usize,
}

impl Default for ComponentLimits {
    fn default() -> Self {
        Self {
            component_bytes: 4 * 1024 * 1024,
            input_bytes: 8 * 1024 * 1024,
            resources: 100_000,
            metadata_bytes: 4 * 1024,
            output_bytes: 4 * 1024 * 1024,
            fuel: 10_000_000,
            deadline: Duration::from_secs(2),
            compiler_deadline: Duration::from_secs(10),
            compiler_memory_bytes: 8 * 1024 * 1024 * 1024,
            memory_bytes: 32 * 1024 * 1024,
            memories: 4,
            table_elements: 10_000,
            tables: 4,
            instances: 32,
        }
    }
}

/// Shared capability-free Wasmtime engine for presentation components.
pub struct ComponentRendererHost {
    engine: Engine,
    limits: ComponentLimits,
    stop_epoch: Arc<AtomicBool>,
    epoch_thread: Option<std::thread::JoinHandle<()>>,
}

/// Host-validated canonical request supplied to one presentation component.
#[derive(Clone, Copy, Debug)]
pub struct ComponentRenderRequest<'a> {
    pub command: RendererCommand,
    pub format: &'a str,
    pub media_type: &'a str,
    pub tooling_schema: &'a Version,
    pub resources: &'a [String],
    pub color: bool,
    pub payload: &'a [u8],
}

impl ComponentRendererHost {
    /// Creates a component host with fuel and epoch interruption enabled.
    pub fn new(limits: ComponentLimits) -> Result<Self, ComponentRenderError> {
        let engine = renderer_engine()?;
        let ticker = engine.clone();
        let stop_epoch = Arc::new(AtomicBool::new(false));
        let ticker_stop = Arc::clone(&stop_epoch);

        let epoch_thread = std::thread::Builder::new()
            .name(String::from("upwell-renderer-epoch"))
            .spawn(move || {
                while !ticker_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(EPOCH_TICK);
                    ticker.increment_epoch();
                }
            })
            .map_err(ComponentRenderError::EpochThread)?;

        Ok(Self {
            engine,
            limits,
            stop_epoch,
            epoch_thread: Some(epoch_thread),
        })
    }

    /// Loads, validates, instantiates, and invokes one explicitly configured component.
    pub fn render(
        &self,
        renderer: &ComponentRenderer,
        request: ComponentRenderRequest<'_>,
    ) -> Result<Vec<u8>, ComponentRenderError> {
        if !renderer.abi().matches(&RENDERER_ABI_VERSION) {
            return Err(ComponentRenderError::Abi {
                required: renderer.abi().clone(),
                host: RENDERER_ABI_VERSION.clone(),
            });
        }
        if !renderer.tooling_schema().matches(request.tooling_schema) {
            return Err(ComponentRenderError::ToolingSchema {
                required: renderer.tooling_schema().clone(),
                document: request.tooling_schema.clone(),
            });
        }
        let input_size = request_size(request, self.limits)?;
        if input_size > self.limits.input_bytes {
            return Err(ComponentRenderError::InputTooLarge {
                size: input_size,
                limit: self.limits.input_bytes,
            });
        }

        let bytes = read_bounded(renderer.path(), self.limits.component_bytes)?;
        let component = compile_component(&self.engine, &bytes, self.limits)?;
        let component_type = component.component_type();

        if let Some((name, _)) = component_type.imports(&self.engine).next() {
            return Err(ComponentRenderError::HostImport {
                name: name.to_owned(),
            });
        }

        let linker = Linker::<HostState>::new(&self.engine);
        let instance_pre = linker
            .instantiate_pre(&component)
            .map_err(ComponentRenderError::InvalidComponent)?;
        let renderer_pre =
            RendererPre::new(instance_pre).map_err(ComponentRenderError::InvalidComponent)?;
        let mut store = self.store()?;
        let bindings = renderer_pre
            .instantiate(&mut store)
            .map_err(classify_execution_error)?;
        let component_request = RenderRequest {
            abi_version: RENDERER_ABI_VERSION.to_string(),
            command: request.command.as_str().to_owned(),
            format: request.format.to_owned(),
            media_type: request.media_type.to_owned(),
            tooling_schema: request.tooling_schema.to_string(),
            resources: request.resources.to_vec(),
            color: request.color,
            payload: request.payload.to_vec(),
        };
        let output = bindings
            .call_render(&mut store, &component_request)
            .map_err(classify_execution_error)?
            .map_err(sanitize_rejection)?;

        let output_size = response_size(&output, self.limits)?;
        if output_size > self.limits.output_bytes {
            return Err(ComponentRenderError::OutputTooLarge {
                size: output_size,
                limit: self.limits.output_bytes,
            });
        }
        if output.format != request.format {
            return Err(ComponentRenderError::ResponseClaim {
                field: "format",
                expected: request.format.to_owned(),
                found: output.format,
            });
        }
        if output.media_type != request.media_type {
            return Err(ComponentRenderError::ResponseClaim {
                field: "media type",
                expected: request.media_type.to_owned(),
                found: output.media_type,
            });
        }
        let selected = request
            .resources
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        if output
            .resources
            .iter()
            .any(|resource| !selected.contains(resource.as_str()))
        {
            return Err(ComponentRenderError::UnselectedResource);
        }

        if renderer.utf8() {
            std::str::from_utf8(&output.body).map_err(ComponentRenderError::InvalidUtf8)?;
        }

        Ok(output.body)
    }

    fn store(&self) -> Result<Store<HostState>, ComponentRenderError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.limits.memory_bytes)
            .memories(self.limits.memories)
            .table_elements(self.limits.table_elements)
            .tables(self.limits.tables)
            .instances(self.limits.instances)
            .build();
        let mut store = Store::new(&self.engine, HostState { limits });

        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(self.limits.fuel)
            .map_err(ComponentRenderError::Engine)?;
        store.epoch_deadline_trap();
        store.set_epoch_deadline(deadline_ticks(self.limits.deadline));
        // Keep canonical-ABI lifting bounded independently of the stricter accepted body size.
        // The exact output contract is checked after lifting; guest memory limits cap this transfer.
        store.set_hostcall_fuel(self.limits.memory_bytes);

        Ok(store)
    }
}

fn renderer_engine() -> Result<Engine, ComponentRenderError> {
    let mut config = Config::new();
    config
        .consume_fuel(true)
        .epoch_interruption(true)
        .max_wasm_stack(512 * 1024);

    Engine::new(&config).map_err(ComponentRenderError::Engine)
}

#[cfg(unix)]
fn configure_compiler_limits(
    _command: &mut std::process::Command,
    _memory_bytes: usize,
) -> Result<(), ComponentRenderError> {
    Ok(())
}

#[cfg(unix)]
fn constrain_compiler_process(
    _child: &std::process::Child,
    _memory_bytes: usize,
) -> Result<(), ComponentRenderError> {
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn constrain_compiler_process(
    child: &std::process::Child,
    memory_bytes: usize,
) -> Result<CompilerProcessGuard, ComponentRenderError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    // SAFETY: null security attributes and name request one private job object.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(ComponentRenderError::CompilerIo(
            std::io::Error::last_os_error(),
        ));
    }
    let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    information.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    information.ProcessMemoryLimit = memory_bytes;
    // SAFETY: job is valid and information points to a correctly sized initialized structure.
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const information).cast(),
            std::mem::size_of_val(&information) as u32,
        )
    };
    // SAFETY: child owns a valid live process handle until this call returns.
    let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) };
    if configured == 0 || assigned == 0 {
        // SAFETY: job was created above and has not yet been closed.
        unsafe { CloseHandle(job) };
        return Err(ComponentRenderError::CompilerIo(
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: flags request a snapshot of every thread, and the returned handle is closed on every
    // path below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        // SAFETY: job was created above, is still open, and this error path closes it exactly once.
        unsafe { CloseHandle(job) };
        return Err(ComponentRenderError::CompilerIo(
            std::io::Error::last_os_error(),
        ));
    }
    let mut entry = THREADENTRY32::default();
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut found = false;
    // SAFETY: snapshot and entry are valid for ToolHelp iteration.
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == child.id() {
            // SAFETY: this thread belongs to the suspended child process.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if !thread.is_null() {
                // SAFETY: thread is a valid suspended thread handle.
                found = unsafe { ResumeThread(thread) } != u32::MAX;
                // SAFETY: thread was opened above and is closed exactly once.
                unsafe { CloseHandle(thread) };
                break;
            }
        }
        // SAFETY: snapshot and entry remain valid for the next record.
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: snapshot is no longer needed.
    unsafe { CloseHandle(snapshot) };
    if !found {
        // SAFETY: job was created above, is still open, and this error path closes it exactly once.
        unsafe { CloseHandle(job) };
        return Err(ComponentRenderError::CompilerIo(
            std::io::Error::last_os_error(),
        ));
    }

    Ok(CompilerProcessGuard(job))
}

#[cfg(windows)]
struct CompilerProcessGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for CompilerProcessGuard {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the valid job handle.
        #[allow(unsafe_code)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0)
        };
    }
}

#[cfg(windows)]
fn configure_compiler_limits(
    command: &mut std::process::Command,
    _memory_bytes: usize,
) -> Result<(), ComponentRenderError> {
    use std::os::windows::process::CommandExt as _;

    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);

    Ok(())
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn apply_worker_memory_limit() -> Result<(), ComponentRenderError> {
    #[cfg(test)]
    return Ok(());

    #[cfg(not(test))]
    {
        #[cfg(target_os = "macos")]
        let memory = clamped_limit(
            libc::RLIMIT_AS,
            macos_process_virtual_bytes()?.saturating_add(worker_memory_limit()? as u64)
                as libc::rlim_t,
        )?;
        #[cfg(not(target_os = "macos"))]
        let memory = clamped_limit(libc::RLIMIT_AS, worker_memory_limit()? as libc::rlim_t)?;
        #[cfg(not(target_os = "macos"))]
        let cpu_seconds = std::env::var("UPWELL_RENDERER_COMPILE_CPU_SECONDS")
            .map_err(|error| ComponentRenderError::CompilerFailed(error.to_string()))?
            .parse::<libc::rlim_t>()
            .map_err(|error| ComponentRenderError::CompilerFailed(error.to_string()))?;
        #[cfg(not(target_os = "macos"))]
        let cpu = clamped_limit(libc::RLIMIT_CPU, cpu_seconds)?;

        // macOS does not expose a reliable enforceable memory rlimit for JIT-capable processes;
        // process isolation and the parent deadline still make the worker killable there.
        // SAFETY: the worker applies the address-space limit to itself with a valid pointer.
        if unsafe { libc::setrlimit(libc::RLIMIT_AS, &memory) } != 0 {
            return Err(ComponentRenderError::CompilerIo(
                std::io::Error::last_os_error(),
            ));
        }
        #[cfg(not(target_os = "macos"))]
        // SAFETY: the worker applies the CPU limit to itself with a valid pointer.
        if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0 {
            return Err(ComponentRenderError::CompilerIo(
                std::io::Error::last_os_error(),
            ));
        }

        Ok(())
    }
}

#[cfg(all(unix, not(test)))]
#[cfg(target_os = "linux")]
type RlimitResource = libc::__rlimit_resource_t;

#[cfg(all(unix, not(test)))]
#[cfg(not(target_os = "linux"))]
type RlimitResource = libc::c_int;

#[cfg(all(unix, not(test)))]
#[allow(unsafe_code)]
fn clamped_limit(
    resource: RlimitResource,
    requested: libc::rlim_t,
) -> Result<libc::rlimit, ComponentRenderError> {
    let mut inherited = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    // SAFETY: inherited points to initialized writable storage for this process's limit.
    if unsafe { libc::getrlimit(resource, &mut inherited) } != 0 {
        return Err(ComponentRenderError::CompilerIo(
            std::io::Error::last_os_error(),
        ));
    }
    let hard = if inherited.rlim_max == libc::RLIM_INFINITY {
        requested
    } else {
        inherited.rlim_max.min(requested)
    };

    Ok(libc::rlimit {
        rlim_cur: inherited.rlim_cur.min(hard),
        rlim_max: hard,
    })
}

#[cfg(windows)]
fn apply_worker_memory_limit() -> Result<(), ComponentRenderError> {
    Ok(())
}

#[cfg(any(windows, all(unix, not(test))))]
fn worker_memory_limit() -> Result<usize, ComponentRenderError> {
    std::env::var("UPWELL_RENDERER_COMPILE_MEMORY")
        .map_err(|error| ComponentRenderError::CompilerFailed(error.to_string()))?
        .parse()
        .map_err(|error: std::num::ParseIntError| {
            ComponentRenderError::CompilerFailed(error.to_string())
        })
}

#[allow(unsafe_code)]
fn compile_component(
    engine: &Engine,
    bytes: &[u8],
    limits: ComponentLimits,
) -> Result<Component, ComponentRenderError> {
    let directory = tempfile::Builder::new()
        .prefix("upwell-renderer-compile")
        .tempdir()
        .map_err(ComponentRenderError::CompilerIo)?;
    let input = directory.path().join("component.wasm");
    let output = directory.path().join("component.cwasm");

    std::fs::write(&input, bytes).map_err(ComponentRenderError::CompilerIo)?;
    let executable = compiler_executable()?;
    let mut command = std::process::Command::new(executable);

    command
        .env("UPWELL_RENDERER_COMPILE_INPUT", &input)
        .env("UPWELL_RENDERER_COMPILE_OUTPUT", &output)
        .env(
            "UPWELL_RENDERER_COMPILE_MEMORY",
            limits.compiler_memory_bytes.to_string(),
        )
        .env(
            "UPWELL_RENDERER_COMPILE_CPU_SECONDS",
            limits.compiler_deadline.as_secs().max(1).to_string(),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    configure_compiler_limits(&mut command, limits.compiler_memory_bytes)?;
    let mut child = command.spawn().map_err(ComponentRenderError::CompilerIo)?;
    #[cfg(unix)]
    constrain_compiler_process(&child, limits.compiler_memory_bytes)?;
    #[cfg(windows)]
    let _compiler_guard = match constrain_compiler_process(&child, limits.compiler_memory_bytes) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();

            return Err(error);
        }
    };
    let deadline = std::time::Instant::now() + limits.compiler_deadline;

    loop {
        if let Some(status) = child.try_wait().map_err(ComponentRenderError::CompilerIo)? {
            if !status.success() {
                let stderr = child
                    .wait_with_output()
                    .map_err(ComponentRenderError::CompilerIo)?
                    .stderr;

                return Err(ComponentRenderError::CompilerFailed(
                    String::from_utf8_lossy(&stderr).into_owned(),
                ));
            }
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().map_err(ComponentRenderError::CompilerIo)?;
            let _ = child.wait();

            return Err(ComponentRenderError::CompilerDeadline);
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // SAFETY: this exact binary's bounded worker produced the artifact in a private temporary
    // directory using the same engine configuration, and no untrusted process can modify it.
    unsafe { Component::deserialize_file(engine, output) }
        .map_err(ComponentRenderError::InvalidComponent)
}

fn compiler_executable() -> Result<std::path::PathBuf, ComponentRenderError> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_cargo-upwell-renderer-compiler") {
        return Ok(path.into());
    }
    let current = std::env::current_exe().map_err(ComponentRenderError::CompilerIo)?;
    let name = if cfg!(windows) {
        "cargo-upwell-renderer-compiler.exe"
    } else {
        "cargo-upwell-renderer-compiler"
    };

    let directory = current
        .parent()
        .ok_or(ComponentRenderError::CompilerMissing)?;

    [
        directory.join(name),
        directory
            .parent()
            .map_or_else(|| directory.join(name), |parent| parent.join(name)),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or(ComponentRenderError::CompilerMissing)
}

#[cfg(all(target_os = "macos", not(test)))]
#[allow(unsafe_code)]
fn macos_process_virtual_bytes() -> Result<u64, ComponentRenderError> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    // SAFETY: info is correctly sized writable storage and getpid identifies this worker.
    let written = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<libc::proc_taskinfo>() as i32,
        )
    };
    if written != std::mem::size_of::<libc::proc_taskinfo>() as i32 {
        return Err(ComponentRenderError::CompilerIo(
            std::io::Error::last_os_error(),
        ));
    }

    // SAFETY: proc_pidinfo reported writing a full proc_taskinfo, which is checked above.
    Ok(unsafe { info.assume_init() }.pti_virtual_size)
}

#[doc(hidden)]
pub fn run_component_compiler_worker() -> Option<Result<(), ComponentRenderError>> {
    let input = std::env::var_os("UPWELL_RENDERER_COMPILE_INPUT")?;
    let output = std::env::var_os("UPWELL_RENDERER_COMPILE_OUTPUT")?;

    Some((|| {
        apply_worker_memory_limit()?;
        let bytes = std::fs::read(input).map_err(ComponentRenderError::CompilerIo)?;
        let engine = renderer_engine()?;
        let artifact = engine
            .precompile_component(&bytes)
            .map_err(ComponentRenderError::InvalidComponent)?;

        std::fs::write(output, artifact).map_err(ComponentRenderError::CompilerIo)
    })())
}

impl Drop for ComponentRendererHost {
    fn drop(&mut self) {
        self.stop_epoch.store(true, Ordering::Relaxed);
        if let Some(thread) = self.epoch_thread.take() {
            let _ = thread.join();
        }
    }
}

struct HostState {
    limits: StoreLimits,
}

/// Component loading, compatibility, sandbox, or execution failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ComponentRenderError {
    #[error("failed to configure the WebAssembly renderer engine")]
    Engine(#[source] anyhow::Error),
    #[error("failed to start the WebAssembly renderer deadline clock")]
    EpochThread(#[source] std::io::Error),
    #[error("failed to read renderer component `{path}`")]
    Read {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("renderer component `{path}` is not a regular file")]
    ComponentNotRegular { path: std::path::PathBuf },
    #[error("renderer component `{path}` exceeds the {limit}-byte file limit")]
    ComponentTooLarge {
        path: std::path::PathBuf,
        limit: usize,
    },
    #[error("renderer component is invalid or does not export the Upwell renderer world")]
    InvalidComponent(#[source] anyhow::Error),
    #[error("renderer compiler process I/O failed")]
    CompilerIo(#[source] std::io::Error),
    #[error("renderer compilation exceeded its deadline")]
    CompilerDeadline,
    #[error("renderer compiler executable is not installed beside cargo-upwell")]
    CompilerMissing,
    #[error("renderer compiler process failed: {0}")]
    CompilerFailed(String),
    #[error("renderer component imports forbidden host capability `{name}`")]
    HostImport { name: String },
    #[error("renderer requires ABI `{required}`, but the host provides `{host}`")]
    Abi {
        required: semver::VersionReq,
        host: Version,
    },
    #[error("renderer requires tooling schema `{required}`, but the input uses `{document}`")]
    ToolingSchema {
        required: semver::VersionReq,
        document: Version,
    },
    #[error("renderer input is {size} bytes, exceeding the {limit}-byte limit")]
    InputTooLarge { size: usize, limit: usize },
    #[error("renderer request has {count} resources, exceeding the {limit}-resource limit")]
    TooManyResources { count: usize, limit: usize },
    #[error("renderer request {field} is {size} bytes, exceeding the {limit}-byte limit")]
    MetadataTooLarge {
        field: &'static str,
        size: usize,
        limit: usize,
    },
    #[error("renderer output is {size} bytes, exceeding the {limit}-byte limit")]
    OutputTooLarge { size: usize, limit: usize },
    #[error("renderer output is not valid UTF-8")]
    InvalidUtf8(#[source] std::str::Utf8Error),
    #[error("renderer rejected the request: {0}")]
    Rejected(String),
    #[error("renderer response changed the {field} claim from `{expected}` to `{found}`")]
    ResponseClaim {
        field: &'static str,
        expected: String,
        found: String,
    },
    #[error("renderer response claims a resource outside the host-selected request")]
    UnselectedResource,
    #[error("renderer exhausted its execution fuel")]
    OutOfFuel,
    #[error("renderer exceeded its execution deadline")]
    Deadline,
    #[error("renderer trapped during execution")]
    Trap(#[source] anyhow::Error),
}

fn read_bounded(path: &std::path::Path, limit: usize) -> Result<Vec<u8>, ComponentRenderError> {
    let path = path.to_path_buf();
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|source| ComponentRenderError::Read {
            path: path.clone(),
            source,
        })?;

    // Reject known special files before opening them: opening a FIFO or device can block before
    // Wasmtime's execution deadline exists. The platform-specific open then closes the race with
    // this check, and handle metadata verifies the object that was actually opened.
    if !is_regular_component(&metadata) {
        return Err(ComponentRenderError::ComponentNotRegular { path });
    }

    let mut file = open_component(&path).map_err(|source| ComponentRenderError::Read {
        path: path.clone(),
        source,
    })?;
    let metadata = file
        .metadata()
        .map_err(|source| ComponentRenderError::Read {
            path: path.clone(),
            source,
        })?;

    if !is_regular_component(&metadata) {
        return Err(ComponentRenderError::ComponentNotRegular { path });
    }
    if metadata.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(ComponentRenderError::ComponentTooLarge { path, limit });
    }

    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));

    (&mut file)
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| ComponentRenderError::Read {
            path: path.clone(),
            source,
        })?;
    if bytes.len() > limit {
        return Err(ComponentRenderError::ComponentTooLarge { path, limit });
    }

    Ok(bytes)
}

#[cfg(not(windows))]
fn is_regular_component(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}

#[cfg(windows)]
fn is_regular_component(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(unix)]
fn open_component(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(windows)]
fn open_component(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_component(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn deadline_ticks(deadline: Duration) -> u64 {
    let deadline = deadline.as_nanos();
    let tick = EPOCH_TICK.as_nanos();
    let ticks = deadline.saturating_add(tick.saturating_sub(1)) / tick;

    u64::try_from(ticks.max(1)).unwrap_or(u64::MAX)
}

fn request_size(
    request: ComponentRenderRequest<'_>,
    limits: ComponentLimits,
) -> Result<usize, ComponentRenderError> {
    if request.resources.len() > limits.resources {
        return Err(ComponentRenderError::TooManyResources {
            count: request.resources.len(),
            limit: limits.resources,
        });
    }

    let abi_version = RENDERER_ABI_VERSION.to_string();
    let tooling_schema = request.tooling_schema.to_string();
    let fields = [
        ("ABI version", abi_version.as_str()),
        ("command", request.command.as_str()),
        ("format", request.format),
        ("media type", request.media_type),
        ("tooling schema", tooling_schema.as_str()),
    ];
    let mut size = framed_size(request.payload.len());

    for (field, value) in fields {
        if value.len() > limits.metadata_bytes {
            return Err(ComponentRenderError::MetadataTooLarge {
                field,
                size: value.len(),
                limit: limits.metadata_bytes,
            });
        }
        size = size.saturating_add(framed_size(value.len()));
    }
    size = size.saturating_add(std::mem::size_of::<u32>());
    for resource in request.resources {
        if resource.len() > limits.metadata_bytes {
            return Err(ComponentRenderError::MetadataTooLarge {
                field: "resource ID",
                size: resource.len(),
                limit: limits.metadata_bytes,
            });
        }
        size = size.saturating_add(framed_size(resource.len()));
    }

    Ok(size)
}

fn response_size(
    response: &RenderResponse,
    limits: ComponentLimits,
) -> Result<usize, ComponentRenderError> {
    if response.resources.len() > limits.resources {
        return Err(ComponentRenderError::TooManyResources {
            count: response.resources.len(),
            limit: limits.resources,
        });
    }
    let mut size = framed_size(response.body.len());
    for (field, value) in [
        ("format", &response.format),
        ("media type", &response.media_type),
    ] {
        if value.len() > limits.metadata_bytes {
            return Err(ComponentRenderError::MetadataTooLarge {
                field,
                size: value.len(),
                limit: limits.metadata_bytes,
            });
        }
        size = size.saturating_add(framed_size(value.len()));
    }
    size = size.saturating_add(std::mem::size_of::<u32>());
    for resource in &response.resources {
        if resource.len() > limits.metadata_bytes {
            return Err(ComponentRenderError::MetadataTooLarge {
                field: "resource ID",
                size: resource.len(),
                limit: limits.metadata_bytes,
            });
        }
        size = size.saturating_add(framed_size(resource.len()));
    }

    Ok(size)
}

const fn framed_size(content: usize) -> usize {
    std::mem::size_of::<u32>().saturating_add(content)
}

fn sanitize_rejection(rejection: String) -> ComponentRenderError {
    const LIMIT: usize = 1024;
    let mut sanitized = String::with_capacity(rejection.len().min(LIMIT));

    for character in rejection.chars() {
        let fragment = match character {
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            character if character.is_control() => format!("\\u{{{:x}}}", character as u32),
            character => character.to_string(),
        };
        if sanitized.len().saturating_add(fragment.len()) > LIMIT {
            sanitized.push_str("...");
            break;
        }
        sanitized.push_str(&fragment);
    }

    ComponentRenderError::Rejected(sanitized)
}

fn classify_execution_error(error: anyhow::Error) -> ComponentRenderError {
    match error.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => ComponentRenderError::OutOfFuel,
        Some(Trap::Interrupt) => ComponentRenderError::Deadline,
        _ => ComponentRenderError::Trap(error),
    }
}

#[cfg(test)]
mod tests;
