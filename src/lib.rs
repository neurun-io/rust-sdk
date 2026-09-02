//! Neurun Rust SDK — what a program needs from Neurun while it runs.
//!
//! Three things: a browser, somewhere to put what it found, and somewhere to
//! remember where it stopped.
//!
//! | | |
//! | --- | --- |
//! | [`Browser`] | a real browser, driven by session id |
//! | [`Documents`] | JSON records in a collection, read back by filter |
//! | [`Memory`] | a key, a value, and optionally an expiry |
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
//! session.close(false).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Configuration
//!
//! [`Browser::from_env`], [`Documents::from_env`] and [`Memory::from_env`] all
//! read `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`, which the worker
//! puts in a handler's environment. That is the entire configuration: there is
//! no app id and no organization, because a value in an environment variable is
//! a claim rather than a credential. The token travels in
//! `neurun-execution-token` metadata on every call, and an address that is not
//! loopback is refused — the token must not leave the host.
//!
//! The organization is resolved from that token on the other side, which is why
//! storage needs no tenancy argument: there is no collection name and no key
//! that reaches another client's data.
//!
//! # Storage
//!
//! ```no_run
//! use neurun::{Documents, Error, Memory};
//! use serde_json::json;
//!
//! # async fn example() -> Result<(), Error> {
//! let mut people = Documents::from_env()?.collection("people");
//! people.insert(&json!({"name": "ada", "age": 36})).await?;
//! let found = people.find(&json!({"age": {"$gte": 30}})).await?;
//!
//! let memory = Memory::from_env()?;
//! memory.set_with("cursor", &json!({"page": 4}), 3600).await?;
//! # Ok(())
//! # }
//! ```
//!
//! A collection is implicit: writing to a name creates it, and emptying it
//! removes it. The filter grammar is small and closed — `$eq`, `$ne`, `$in`,
//! `$gt`, `$gte`, `$lt`, `$lte`, `$exists` — and an operator outside it is
//! refused rather than ignored, because a filter that quietly drops a clause
//! matches more than was asked for.
//!
//! # Commands
//!
//! Each command is its own call, shaped after the browser's own function —
//! [`Session::navigate`] takes a URL, [`Session::wait_for_navigation`] a
//! [`WaitUntil`] and a timeout. The set grows one command at a time, and a
//! caller can read what it is allowed to do off this list.
//!
//! | | |
//! | --- | --- |
//! | [`navigate`](Session::navigate), [`wait_for_navigation`](Session::wait_for_navigation) | drive the page |
//! | [`node`](Session::node) | what an element is, where it is, what it says |
//! | [`human_mouse_move`](Session::human_mouse_move), [`human_click`](Session::human_click) | the pointer |
//! | [`human_type`](Session::human_type) | the keyboard |
//! | [`human_scroll_y`](Session::human_scroll_y), [`human_scroll_y_to`](Session::human_scroll_y_to) | the wheel |
//! | [`cookies`](Session::cookies), [`set_cookies`](Session::set_cookies) | the jar |
//!
//! # Why the input is human
//!
//! A pointer that teleports, a key held for exactly the same number of
//! milliseconds every time, a scroll that arrives in one jump — each is a thing
//! no hand does, and each is cheap for a page to notice. The `human_` commands
//! move along a curve drawn fresh every time, hold each key for a length drawn
//! per key, and ease a scroll to a stop.
//!
//! They are slower for exactly that reason, and that is the trade. A whole
//! gesture is one call rather than a stream of events, because the pacing has
//! to happen beside the browser: an event per round trip across this connection
//! would leave the network writing the rhythm.
//!
//! An element is named by CSS selector on every call and looked up again each
//! time, so nothing here goes stale across a navigation. Where a command takes
//! both a selector and a point, the selector wins — an element knows where it
//! is, and a caller holding a rectangle from before the last scroll does not.
//!
//! # What a profile remembers
//!
//! A profile is where a session's state lives between runs, and both
//! directions are opt-in: [`Browser::open_with`] takes a `load_storage` that
//! starts the browser from what the profile holds, and [`Session::close_with`]
//! a `save_storage` that writes what the browser holds back to it. Cookies,
//! for now.
//!
//! The capture replaces the profile's state rather than merging into it, which
//! is the only semantic that can end a login. Both flags need a profile.
//!
//! # No heartbeat
//!
//! Driving a session renews its lease, because a browser being commanded is a
//! browser that is alive. A session left idle past the lease leaves the list,
//! and one being used never does. Close on the way out anyway, including on
//! failure — a session left to expire shows the dashboard a browser that is not
//! there until the lease runs out.

mod app;
mod connection;
mod document;
pub mod error;
mod memory;
mod parser;
mod serve;
mod session;

/// The browser contract, generated from `proto/browser.proto`.
///
/// Generated rather than hand-written so that a field added upstream is a build
/// failure here rather than a value silently dropped.
pub mod proto {
    tonic::include_proto!("neurun.browser.v1");
}

/// The document contract, generated from `proto/document.proto`.
pub mod documents {
    tonic::include_proto!("neurun.document.v1");
}

/// The memory contract, generated from `proto/memory.proto`.
///
/// Named `memories` rather than `memory` because [`memory`](crate::Memory) is
/// the handle a caller uses, and a generated module is not what that word
/// should reach.
pub mod memories {
    tonic::include_proto!("neurun.memory.v1");
}

/// The parser contract, generated from `proto/parser.proto`.
///
/// Named `parsers` rather than `parser` because [`Parsers`](crate::Parsers) is
/// the handle a caller uses, and a generated module is not what that word
/// should reach.
pub mod parsers {
    tonic::include_proto!("neurun.parser.v1");
}

pub use app::{App, Method, Overlap, Request, Response};
pub use connection::Token;
pub use document::{Collection, CollectionInfo, Document, Documents};
pub use error::{Error, Result};
pub use memory::{Entry, Memory};
pub use parser::{ParseResult, Parsers, Probe};
pub use proto::{Attribute, Cookie, MetaEntry, MouseButton, Node, Profile, ScrollAlign, WaitUntil};
pub use session::{Browser, ProfileUpdate, Session, SessionInfo, Warned};
