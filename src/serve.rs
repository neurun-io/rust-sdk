//! Running a declared app: once, or until it is stopped.

use std::collections::HashMap;
use std::sync::Arc;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response as AxumResponse;
use axum::routing::any;
use chrono::Utc;
use croner::Cron;

use crate::app::{App, Overlap, build_request, listen_address, parse_method};
use crate::error::Error;

/// Run the app the way the command line asks it to be run.
///
/// This is the whole entry contract. A program calls it from `main` and the
/// invocation decides what happens, so the same binary is a runner, a server
/// and its own description without being rebuilt for each.
pub async fn run(app: App) -> Result<(), Error> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("describe") => describe(&app),
        Some("serve") => serve(app).await,
        _ => run_once(app, &arguments).await,
    }
}

/// Print what this build declares, for the builder to record.
fn describe(app: &App) -> Result<(), Error> {
    let encoded = serde_json::to_string(&app.surface())
        .map_err(|err| Error::Run(format!("describe this app: {err}")))?;
    println!("{encoded}");
    Ok(())
}

/// The runner contract: one event in, one result out, then exit.
async fn run_once(app: App, arguments: &[String]) -> Result<(), Error> {
    let (input_path, result_path) = match arguments {
        [input, result, ..] => (input.clone(), result.clone()),
        _ => {
            return Err(Error::Run(
                "expected `describe`, `serve`, or an input and a result path".into(),
            ));
        }
    };
    let entrypoint = app
        .entrypoint
        .clone()
        .ok_or_else(|| Error::Run("this app declares no entrypoint to invoke".into()))?;

    let raw = std::fs::read(&input_path)
        .map_err(|err| Error::Run(format!("read {input_path}: {err}")))?;
    let event: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|err| Error::Run(format!("read {input_path}: {err}")))?;

    let produced = entrypoint(event).await?;
    let encoded = serde_json::to_vec(&produced)
        .map_err(|err| Error::Run(format!("encode the result: {err}")))?;
    std::fs::write(&result_path, encoded)
        .map_err(|err| Error::Run(format!("write {result_path}: {err}")))
}

/// Stay up: answer endpoints, keep schedules, and stop when asked.
async fn serve(app: App) -> Result<(), Error> {
    if !app.serves() {
        return Err(Error::Run(
            "this app declares no endpoint or schedule to serve".into(),
        ));
    }
    let address = listen_address()?;
    let app = Arc::new(app);

    for schedule in schedules(&app)? {
        tokio::spawn(schedule.run());
    }

    // One catch-all rather than a route per endpoint: the app's own table is
    // what decides, so a request that matches nothing is refused here in the
    // same shape it would be anywhere else.
    let router = Router::new()
        .fallback(any(dispatch))
        .with_state(app.clone());

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|err| Error::Run(format!("bind {address}: {err}")))?;

    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|err| Error::Run(format!("serve: {err}")))
}

async fn dispatch(
    State(app): State<Arc<App>>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> AxumResponse {
    let Some(wanted) = parse_method(method.as_str()) else {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    };
    let path = uri.path().to_owned();
    let Some(endpoint) = app
        .endpoints
        .iter()
        .find(|endpoint| endpoint.method == wanted && endpoint.path == path)
    else {
        return empty(StatusCode::NOT_FOUND);
    };

    let mut carried = HashMap::with_capacity(headers.len());
    for (name, value) in headers.iter() {
        if let Ok(value) = value.to_str() {
            carried.insert(name.as_str().to_ascii_lowercase(), value.to_owned());
        }
    }
    let request = build_request(
        wanted,
        path,
        uri.query().unwrap_or_default().to_owned(),
        carried,
        body.to_vec(),
    );

    let answered = (endpoint.handler)(request).await;
    let mut builder = AxumResponse::builder()
        .status(StatusCode::from_u16(answered.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for (name, value) in answered.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(answered.body.into())
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

fn empty(status: StatusCode) -> AxumResponse {
    AxumResponse::builder()
        .status(status)
        .body(axum::body::Body::empty())
        .expect("an empty body is always a valid response")
}

/// A schedule, parsed and ready to keep time.
struct Keeper {
    name: String,
    cron: Cron,
    overlap: Overlap,
    handler: Arc<dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
    running: Arc<AtomicBool>,
}

fn schedules(app: &Arc<App>) -> Result<Vec<Keeper>, Error> {
    let mut keepers = Vec::with_capacity(app.schedules.len());
    for schedule in &app.schedules {
        // Parsed here rather than at each tick: an expression the app cannot
        // keep is a fault in the app, and it should refuse to start rather than
        // come up and silently never fire.
        let cron = Cron::from_str(&schedule.expression)
            .map_err(|err| Error::Run(format!("schedule {:?}: {err}", schedule.name)))?;
        keepers.push(Keeper {
            name: schedule.name.clone(),
            cron,
            overlap: schedule.overlap,
            handler: schedule.handler.clone(),
            running: Arc::new(AtomicBool::new(false)),
        });
    }
    Ok(keepers)
}

impl Keeper {
    /// Sleep until due, fire, repeat — for as long as the process lives.
    async fn run(self) {
        loop {
            let Some(wait) = self.until_next() else {
                eprintln!("neurun: schedule {:?} has no next occurrence", self.name);
                return;
            };
            tokio::time::sleep(wait).await;

            if self.overlap == Overlap::Skip
                && self
                    .running
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
            {
                continue;
            }
            let handler = self.handler.clone();
            let running = self.running.clone();
            let skipping = self.overlap == Overlap::Skip;
            tokio::spawn(async move {
                handler().await;
                if skipping {
                    running.store(false, Ordering::SeqCst);
                }
            });
        }
    }

    fn until_next(&self) -> Option<Duration> {
        let now = Utc::now();
        self.cron
            .find_next_occurrence(&now, false)
            .ok()
            .map(|next| (next - now).to_std().unwrap_or(Duration::from_secs(1)))
    }
}
