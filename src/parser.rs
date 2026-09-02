//! Parsers, as a handler sees it.
//!
//! ```no_run
//! use neurun::{Error, Parsers};
//!
//! # async fn example(html: String) -> Result<(), Error> {
//! let parsers = Parsers::from_env()?;
//! let parsed = parsers.parse("product-card", html).await?;
//! let title = &parsed.output["title"];
//! let cost = parsed.elapsed;
//! # Ok(())
//! # }
//! ```
//!
//! The plane never fetches the page. The HTML is the one the handler already
//! has, which is what keeps a parse free of egress, robots and proxy policy:
//! fetching is [`Browser`](crate::Browser)'s job, and parsing is this.
//!
//! Neither the organization nor the project is sent. Both are resolved from the
//! execution token on the other side, which is what makes a parser name safe to
//! write into a handler: there is no name that reaches another client's parsers.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

use crate::connection::{Connection, Token};
use crate::error::{Error, Result};
use crate::parsers::ParseRequest;
use crate::parsers::parsers_client::ParsersClient;

type Client = ParsersClient<InterceptedService<Channel, Token>>;

/// What one row saw.
///
/// Enough to tell a selector that matched the wrong element from one that
/// matched nothing — which a scrape that silently went empty needs as much as a
/// builder does. `selector` is the one of the row's selectors the page answered
/// to, so a parser quietly running on its second choice says so.
///
/// There is no error: a stored parser was validated on write, so a selector
/// that does not compile never reaches a parse.
#[derive(Debug, Clone)]
pub struct Probe {
    /// A field's path in the output — `money.amount` — or a parent's name.
    pub path: String,
    /// Totalled across every element the parent matched, not per element.
    pub matches: u32,
    pub selector: String,
    pub sample: String,
}

/// The output, what each row saw, and what the parse cost.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The parsed document.
    ///
    /// It arrives in the order the definition declares, but a [`Value`] holds
    /// its keys sorted, so that order survives only as far as here. A handler
    /// that wants the declared order reads it into a shape of its own with
    /// [`parse`](ParseResult::parse), where the field order is the struct's.
    pub output: Value,
    /// What each field saw, keyed by its path in the output.
    pub probes: BTreeMap<String, Probe>,
    /// What each parent matched, keyed by name, including a parent no field
    /// points at yet.
    pub scopes: BTreeMap<String, Probe>,
    /// What the parse itself took, measured around it on the other side. It is
    /// the cost of the work rather than of the call: time the call yourself if
    /// the round trip is what you are asking about.
    pub elapsed: Duration,
}

impl ParseResult {
    /// Reads the output as a declared shape.
    pub fn parse<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.output.clone())
            .map_err(|error| Error::Request(format!("the output could not be read: {error}")))
    }
}

/// A handler's door to the parsers its own project owns.
///
/// Reads `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`, which the worker
/// puts in the environment — the same two [`Browser`](crate::Browser) reads,
/// because it is the same listener and the same credential.
#[derive(Debug, Clone)]
pub struct Parsers {
    connection: Connection,
}

impl Parsers {
    /// Reads `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            connection: Connection::from_env()?,
        })
    }

    pub fn new(address: impl Into<String>, token: impl Into<String>) -> Result<Self> {
        Ok(Self {
            connection: Connection::new(address, token)?,
        })
    }

    /// Where this handle expects the control plane.
    pub fn address(&self) -> &str {
        self.connection.address()
    }

    /// Runs a stored parser over a document.
    ///
    /// `parser` is the name it was created with, which is what survives the
    /// definition behind it being rewritten. `html` is at most 4 MB: past that
    /// it is a download, not a page.
    pub async fn parse(
        &self,
        parser: impl Into<String>,
        html: impl Into<String>,
    ) -> Result<ParseResult> {
        let mut client = self.connect().await?;
        let answer = client
            .parse(ParseRequest {
                parser: parser.into(),
                html: html.into(),
            })
            .await?
            .into_inner();
        let output = serde_json::from_str(&answer.output)
            .map_err(|error| Error::Request(format!("the output could not be read: {error}")))?;
        Ok(ParseResult {
            output,
            probes: probes(answer.probes),
            scopes: probes(answer.scopes),
            elapsed: Duration::from_micros(u64::try_from(answer.elapsed_micros).unwrap_or(0)),
        })
    }

    async fn connect(&self) -> Result<Client> {
        let (channel, token) = self.connection.open().await?;
        Ok(ParsersClient::with_interceptor(channel, token))
    }
}

/// Keys the rows by the path each carries, which is what they are read by.
fn probes(rows: Vec<crate::parsers::Probe>) -> BTreeMap<String, Probe> {
    rows.into_iter()
        .map(|row| {
            (
                row.path.clone(),
                Probe {
                    path: row.path,
                    matches: row.matches,
                    selector: row.selector,
                    sample: row.sample,
                },
            )
        })
        .collect()
}
