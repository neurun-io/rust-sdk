//! What a program declares itself to be.
//!
//! An app is one of two things, and it says which by what it declares. Declare
//! only an entrypoint and it is a runner: the worker starts it, hands it one
//! event, takes the result and the process exits. Declare an endpoint or a
//! schedule and it can also be run as a server: the same program, the same
//! environment, the same browser — started once and left up until somebody
//! terminates it.
//!
//! ```no_run
//! use neurun::{App, Method, Request, Response};
//!
//! async fn webhook(request: Request) -> Response {
//!     Response::json(200, &serde_json::json!({ "seen": request.path() }))
//! }
//!
//! async fn refresh() {
//!     // whatever the app does every hour
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), neurun::Error> {
//!     App::new()
//!         .endpoint(Method::Post, "/webhook", webhook)
//!         .cron("refresh", "0 * * * *", refresh)
//!         .run()
//!         .await
//! }
//! ```
//!
//! # How it is started
//!
//! The same binary answers three invocations, and which one it got is read off
//! the command line rather than configured:
//!
//! | | |
//! | --- | --- |
//! | `describe` | print the surface as JSON and exit — how the builder learns what this build serves |
//! | `serve` | bind and stay up, answering endpoints and keeping schedules |
//! | `<input> <result>` | run the entrypoint once against one event, as a runner |
//!
//! The third is the contract compiled builds have always had, so a program that
//! declares nothing new keeps working unchanged.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;

use crate::error::Error;

/// The HTTP methods an endpoint may answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }
}

/// One request, as the app sees it.
///
/// The path is the app's own — the prefix the control plane routes on is
/// stripped before this is built, so a handler mounted at `/webhook` sees
/// `/webhook` however it was reached.
#[derive(Debug, Clone)]
pub struct Request {
    method: Method,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    pub fn method(&self) -> Method {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The raw query string, without the leading `?`.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// A header by name, matched without regard to case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body, decoded as whatever the caller expects it to be.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, Error> {
        serde_json::from_slice(&self.body).map_err(|err| Error::Request(err.to_string()))
    }
}

/// What a handler answers with. Status, headers and body are the app's to
/// choose: the control plane routes the request and reads none of it.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// A JSON body, with the content type set to match.
    pub fn json<T: Serialize>(status: u16, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Self {
                status,
                headers: vec![("content-type".into(), "application/json".into())],
                body,
            },
            // A handler that cannot serialize its own answer is a fault in the
            // app, and it reads as one rather than as a broken connection.
            Err(err) => Self::new(500, err.to_string()),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type EndpointFn = Arc<dyn Fn(Request) -> BoxFuture<Response> + Send + Sync>;
type ScheduleFn = Arc<dyn Fn() -> BoxFuture<()> + Send + Sync>;
type EntrypointFn =
    Arc<dyn Fn(serde_json::Value) -> BoxFuture<Result<serde_json::Value, Error>> + Send + Sync>;

pub(crate) struct Endpoint {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) handler: EndpointFn,
}

pub(crate) struct Schedule {
    pub(crate) name: String,
    pub(crate) expression: String,
    pub(crate) overlap: Overlap,
    pub(crate) handler: ScheduleFn,
}

/// What a schedule does when its previous run has not finished.
///
/// The app decides, because the app is the only thing that knows whether two of
/// its own runs may overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overlap {
    /// Let the next run start anyway.
    Allow,
    /// Leave the tick unfired while the previous run is still going.
    #[default]
    Skip,
}

/// One program's declaration of itself.
#[derive(Default)]
pub struct App {
    pub(crate) entrypoint: Option<EntrypointFn>,
    pub(crate) endpoints: Vec<Endpoint>,
    pub(crate) schedules: Vec<Schedule>,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// The callable one execution invokes. A runner needs this and nothing
    /// else; a server may still declare it, so the same app can be invoked
    /// directly as well as served.
    pub fn entrypoint<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, Error>> + Send + 'static,
    {
        self.entrypoint = Some(Arc::new(move |event| Box::pin(handler(event))));
        self
    }

    /// One route this app answers. Declaring any is what makes it servable.
    pub fn endpoint<F, Fut>(mut self, method: Method, path: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.endpoints.push(Endpoint {
            method,
            path: path.into(),
            handler: Arc::new(move |request| Box::pin(handler(request))),
        });
        self
    }

    /// One function this app calls on a timer, while it is up.
    ///
    /// The schedule belongs to the process: it is kept by the running server
    /// rather than by the control plane, so it exists exactly as long as the
    /// server does and there is nothing to fire when it is gone.
    pub fn cron<F, Fut>(
        self,
        name: impl Into<String>,
        expression: impl Into<String>,
        handler: F,
    ) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.cron_with(name, expression, Overlap::default(), handler)
    }

    /// A schedule that says what to do when it is still running as it comes due.
    pub fn cron_with<F, Fut>(
        mut self,
        name: impl Into<String>,
        expression: impl Into<String>,
        overlap: Overlap,
        handler: F,
    ) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.schedules.push(Schedule {
            name: name.into(),
            expression: expression.into(),
            overlap,
            handler: Arc::new(move || Box::pin(handler())),
        });
        self
    }

    /// Run the app the way the command line asks it to be run.
    ///
    /// The whole entry contract: a program calls this from `main` and the
    /// invocation decides what happens, so one binary is a runner, a server and
    /// its own description without being rebuilt for each.
    pub async fn run(self) -> Result<(), Error> {
        crate::serve::run(self).await
    }

    /// Whether this app can be run as a server.
    pub fn serves(&self) -> bool {
        !self.endpoints.is_empty() || !self.schedules.is_empty()
    }

    /// What this program declares, as the builder reads it.
    pub(crate) fn surface(&self) -> Surface {
        Surface {
            endpoints: self
                .endpoints
                .iter()
                .map(|endpoint| DescribedEndpoint {
                    method: endpoint.method.as_str(),
                    path: endpoint.path.clone(),
                })
                .collect(),
            schedules: self
                .schedules
                .iter()
                .map(|schedule| DescribedSchedule {
                    name: schedule.name.clone(),
                    cron: schedule.expression.clone(),
                })
                .collect(),
        }
    }
}

/// The shape `describe` prints, which is what the build records.
#[derive(Debug, Serialize)]
pub(crate) struct Surface {
    pub(crate) endpoints: Vec<DescribedEndpoint>,
    pub(crate) schedules: Vec<DescribedSchedule>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DescribedEndpoint {
    pub(crate) method: &'static str,
    pub(crate) path: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct DescribedSchedule {
    pub(crate) name: String,
    pub(crate) cron: String,
}

pub(crate) fn build_request(
    method: Method,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
) -> Request {
    Request {
        method,
        path,
        query,
        headers,
        body,
    }
}

pub(crate) fn parse_method(raw: &str) -> Option<Method> {
    match raw.to_ascii_uppercase().as_str() {
        "GET" => Some(Method::Get),
        "POST" => Some(Method::Post),
        "PUT" => Some(Method::Put),
        "PATCH" => Some(Method::Patch),
        "DELETE" => Some(Method::Delete),
        "HEAD" => Some(Method::Head),
        "OPTIONS" => Some(Method::Options),
        _ => None,
    }
}

/// Where a served app binds. Loopback only: the control plane is the only thing
/// that reaches an app, exactly as it is the only thing that reaches a browser.
pub(crate) fn listen_address() -> Result<SocketAddr, Error> {
    let raw = std::env::var("NEURUN_LISTEN_ADDRESS")
        .map_err(|_| Error::Configuration("NEURUN_LISTEN_ADDRESS is not set".into()))?;
    raw.parse().map_err(|_| {
        Error::Configuration(format!("NEURUN_LISTEN_ADDRESS {raw:?} is not an address"))
    })
}
