//! Neurun Rust SDK — what a program needs from Neurun while it runs.
//!
//! One thing, really: a browser.
//!
//! Neurun is the broker. A handler asks the control plane for a browser, gets a
//! session id, and drives that id — it never learns where the browser runs and
//! cannot be told. That is what lets the dashboard list a session and watch its
//! display: nothing is happening on a port only the tenant's code knows about.
//!
//! ```no_run
//! use neurun::{Browser, Error};
//!
//! # async fn example() -> Result<(), Error> {
//! let mut session = Browser::from_env()?.open("chrome").await?;
//! session.navigate("https://example.com").await?;
//! session.wait_for_navigation().await?;
//! session.close().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Configuration
//!
//! [`Browser::from_env`] reads `NEURUN_GRPC_ADDRESS` and
//! `NEURUN_EXECUTION_TOKEN`, which the worker puts in a handler's environment.
//! That is the entire configuration: there is no app id, because an app id in
//! an environment variable is a claim rather than a credential. The token
//! travels in `neurun-execution-token` metadata on every call, and an address
//! that is not loopback is refused — the token must not leave the host.
//!
//! # Commands
//!
//! Each command is its own call, shaped after the browser's own function —
//! [`Session::navigate`] takes a URL, [`Session::wait_for_navigation`] a
//! [`WaitUntil`] and a timeout. The set is small because the browser
//! implements a small set, and it grows one command at a time.
//!
//! # No heartbeat
//!
//! Driving a session renews its lease, because a browser being commanded is a
//! browser that is alive. A session left idle past the lease leaves the list,
//! and one being used never does. Close on the way out anyway, including on
//! failure — a session left to expire shows the dashboard a browser that is not
//! there until the lease runs out.

pub mod error;
mod session;

/// The control plane's gRPC contract, generated from `proto/browser.proto`.
///
/// Generated rather than hand-written so that a field added upstream is a build
/// failure here rather than a value silently dropped.
pub mod proto {
    tonic::include_proto!("neurun.browser.v1");
}

pub use error::{Error, Result};
pub use proto::WaitUntil;
pub use session::{Browser, Session, SessionInfo, Token};
