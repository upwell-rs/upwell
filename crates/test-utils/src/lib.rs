//! Shared infrastructure for deterministic native tests.

#[cfg(feature = "app")]
mod environment;
mod path;
mod process;
#[cfg(feature = "app")]
mod server;
mod task;
mod temp;
mod timeout;

#[cfg(feature = "app")]
pub use environment::TestEnvironment;
pub use path::path_ends_with_components;
pub use process::{DEFAULT_PROCESS_TIMEOUT, run_command, run_command_with_timeout};
#[cfg(feature = "app")]
pub use server::TestServer;
pub use task::AbortOnDropTask;
#[cfg(feature = "app")]
pub(crate) use task::LoopbackTask;
pub use temp::{FIXTURE_MARKER, TempFixture};
pub use timeout::{DEFAULT_TEST_TIMEOUT, deadline, deadline_with_timeout};
