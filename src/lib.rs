//! Neurun Rust SDK — what a program needs from Neurun while it runs.
//!
//! One thing, really: a browser that is still signed in from yesterday.
//!
//! A [browser profile](profile::BrowserProfile) has two halves. Its **state**
//! is cookies and DOM storage, which is what makes it worth having. Its
//! **identity** is presentation — user agent inputs, screen metrics, timezone,
//! locale, GPU strings, proxy — and is optional: a profile without one launches
//! the browser as itself, and still carries its state.
//!
//! The control plane never opens a browser. `neurun-browser` is a separate gRPC
//! server that runs on loopback beside your program, so the loop is yours:
//!
//! ```no_run
//! use neurun::{BrowserProfiles, Error};
//!
//! # async fn drive(_endpoint: &str) -> Result<usize, Error> { Ok(0) }
//! # async fn example() -> Result<(), Error> {
//! let profiles = BrowserProfiles::from_env()?;
//!
//! let orders = profiles
//!     .run("bp_01J...", |session| async move {
//!         // session.endpoint_url() speaks CDP for Chrome, BiDi for Firefox.
//!         drive(session.endpoint_url()).await
//!     })
//!     .await?;
//! # let _ = orders;
//! # Ok(())
//! # }
//! ```
//!
//! [`run`](BrowserProfiles::run) reads the profile and its state, opens the
//! session carrying both, and afterwards closes it and stores what the browser
//! captured. Take the steps yourself with [`open`](BrowserProfiles::open) and
//! [`Session::close`] when a run needs to decide for itself what to keep.
//!
//! # Two ways a profile gets erased, and what this crate does about them
//!
//! `PUT .../state` **replaces** rather than merges — the browser hands back its
//! whole cookie jar, so a cookie missing from the body was deleted, and merging
//! would resurrect a login the site had already ended. The cost of that is that
//! writing an empty state erases the profile.
//!
//! Firefox is the way that happens by accident: it launches, but it carries no
//! profile, and closing a Firefox session hands back an empty state. So this
//! crate never writes back after Firefox — [`Close::unsaved`] says
//! [`Unsaved::Firefox`] and the capture is returned rather than stored.
//! [`Unsaved::EmptyCapture`] is the same refusal for a browser that handed back
//! nothing over a profile that held something. [`BrowserProfiles::clear_state`]
//! is how a profile gets erased on purpose.
//!
//! # Configuration
//!
//! [`BrowserProfiles::from_env`] reads `NEURUN_URL`, `NEURUN_API_KEY` and
//! `NEURUN_BROWSER_ADDR` (default `127.0.0.1:1268`). Reading profile state
//! takes the `browser_profiles:write` scope, not `:read` — reading state is
//! exporting live sessions.
//!
//! # What is deliberately not here
//!
//! Projects, apps, deployments, builds, users and API keys. Those are created
//! before a program runs and are the dashboard's and the API's business, not a
//! running program's. There is no entrypoint annotation either: the only
//! deployment runtime is Python, and that annotation lives in the Python SDK.

mod api;
pub mod error;
pub mod profile;
mod session;
mod wire;

/// The browser server's gRPC contract, generated from `proto/browser.proto`.
///
/// Generated rather than hand-written so that a field added upstream is a build
/// failure here rather than a value silently dropped.
pub mod proto {
    tonic::include_proto!("neurun.browser.v1");
}

pub use error::{Error, Result};
pub use profile::{
    Brand, BrowserKind, BrowserProfile, Cookie, Geo, Gpu, Identity, Os, Platform, ProfileState,
    Protocol, RedactedCookie, Screen, Storage,
};
pub use session::{
    BrowserProfiles, BrowserProfilesBuilder, Close, OpenOptions, Session, SessionInfo, Unsaved,
    DEFAULT_BROWSER_ADDRESS,
};
