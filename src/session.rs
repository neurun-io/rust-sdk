//! Browser sessions, as a handler sees them.
//!
//! ```text
//! OpenSession{browser, browser_profile_id?}  →  a session id
//! Execute{session_id, command}                  as many times as needed
//! CloseSession{session_id}                      including on failure
//! ```
//!
//! Neurun is the broker. This talks to the control plane on loopback and to
//! nothing else: it does not know that a browser service exists, where it
//! listens, or that one was spawned for this host, and it cannot be told. That
//! is what lets the dashboard list a session and watch its display — nothing is
//! happening on a port only this process knows about.
//!
//! There is no heartbeat. Driving a session renews its lease, because a browser
//! being commanded is a browser that is alive.

use std::net::IpAddr;
use std::time::Duration;

use tonic::metadata::MetadataValue;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tonic::{Request, Status};

use crate::error::{Error, Result};
use crate::proto::browser_client::BrowserClient;
use crate::proto::{CloseSessionRequest, ExecuteRequest, OpenSessionRequest};

/// The credential travels here on every call.
const TOKEN_HEADER: &str = "neurun-execution-token";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

type Client = BrowserClient<InterceptedService<Channel, Token>>;

/// A handler's door to the control plane's browser broker.
///
/// Reads `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`, which the worker
/// puts in the environment. That is the entire configuration: there is no app
/// id, because an app id in an environment variable is a claim rather than a
/// credential. The token is the one thing this process holds that Neurun
/// minted, and the organization, app and execution are looked up from it on the
/// other side.
#[derive(Debug, Clone)]
pub struct Browser {
    endpoint: String,
    address: String,
    token: String,
}

impl Browser {
    /// Reads `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`.
    pub fn from_env() -> Result<Self> {
        Self::new(
            environment("NEURUN_GRPC_ADDRESS").unwrap_or_default(),
            environment("NEURUN_EXECUTION_TOKEN").unwrap_or_default(),
        )
    }

    pub fn new(address: impl Into<String>, token: impl Into<String>) -> Result<Self> {
        let address = address.into();
        let address = address.trim().to_string();
        if address.is_empty() {
            return Err(Error::configuration(
                "a Neurun gRPC address is required: pass one, or set \
                 NEURUN_GRPC_ADDRESS. The worker sets it for a running handler.",
            ));
        }
        let token = token.into().trim().to_string();
        if token.is_empty() {
            return Err(Error::configuration(
                "a Neurun execution token is required: pass one, or set \
                 NEURUN_EXECUTION_TOKEN. The worker mints one per execution.",
            ));
        }
        Ok(Self {
            endpoint: loopback_endpoint(&address)?,
            address,
            token,
        })
    }

    /// Where this handle expects the control plane.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Opens a browser and returns the session the control plane created.
    pub async fn open(&self, browser: impl Into<String>) -> Result<Session> {
        self.open_with(browser, "").await
    }

    /// Opens a browser wearing a profile.
    ///
    /// An empty `profile_id` is a plain browser, which is the ordinary case.
    pub async fn open_with(
        &self,
        browser: impl Into<String>,
        profile_id: impl Into<String>,
    ) -> Result<Session> {
        let mut client = self.connect().await?;
        let session = client
            .open_session(OpenSessionRequest {
                browser: browser.into(),
                browser_profile_id: profile_id.into(),
            })
            .await?
            .into_inner();
        Ok(Session {
            client,
            info: SessionInfo {
                id: session.id,
                app_id: session.app_id,
                execution_id: session.execution_id,
                browser_profile_id: session.browser_profile_id,
                browser: session.browser,
                status: session.status,
                started_at: session.started_at,
            },
            is_open: true,
        })
    }

    async fn connect(&self) -> Result<Client> {
        let channel = tonic::transport::Endpoint::from_shared(self.endpoint.clone())
            .map_err(|error| {
                Error::configuration(format!(
                    "{} is not a usable Neurun gRPC address: {error}",
                    self.address
                ))
            })?
            .connect_timeout(CONNECT_TIMEOUT)
            .connect()
            .await
            .map_err(|source| Error::Connect {
                address: self.address.clone(),
                source,
            })?;
        let token = Token::new(&self.token)?;
        Ok(BrowserClient::with_interceptor(channel, token))
    }
}

/// A session as the control plane described it when it opened.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub app_id: String,
    pub execution_id: String,
    /// The profile this session wears, or empty for a plain browser.
    pub browser_profile_id: String,
    pub browser: String,
    pub status: String,
    /// Unix seconds.
    pub started_at: i64,
}

/// One open browser. Drive it with [`execute`](Session::execute), and close it.
///
/// Dropping one without [`close`](Session::close) leaves it to expire, which is
/// correct but slow: the dashboard shows a browser that is not there until the
/// lease runs out.
#[derive(Debug)]
pub struct Session {
    client: Client,
    info: SessionInfo,
    is_open: bool,
}

impl Session {
    /// What the control plane said this session is.
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn id(&self) -> &str {
        &self.info.id
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Relays one command to this session's browser and returns its reply.
    ///
    /// The payload is opaque here as it is at the control plane: it is a
    /// serialized browser-service command, and what encodes it is an agreement
    /// between the caller and that service.
    ///
    /// Executing also renews the session's lease, which is why there is no
    /// heartbeat to forget.
    pub async fn execute(&mut self, command: impl Into<Vec<u8>>) -> Result<Vec<u8>> {
        if !self.is_open {
            return Err(Error::Closed {
                session_id: self.info.id.clone(),
            });
        }
        let response = self
            .client
            .execute(ExecuteRequest {
                session_id: self.info.id.clone(),
                command: command.into(),
            })
            .await?
            .into_inner();
        Ok(response.result)
    }

    /// Stops the browser and drops the session.
    pub async fn close(&mut self) -> Result<()> {
        if !self.is_open {
            return Err(Error::Closed {
                session_id: self.info.id.clone(),
            });
        }
        self.is_open = false;
        self.client
            .close_session(CloseSessionRequest {
                session_id: self.info.id.clone(),
            })
            .await?;
        Ok(())
    }
}

/// Puts the execution token on every request.
#[derive(Debug, Clone)]
pub struct Token(MetadataValue<tonic::metadata::Ascii>);

impl Token {
    fn new(token: &str) -> Result<Self> {
        token
            .parse()
            .map(Self)
            .map_err(|_| Error::configuration("the execution token is not a usable header value"))
    }
}

impl tonic::service::Interceptor for Token {
    fn call(&mut self, mut request: Request<()>) -> std::result::Result<Request<()>, Status> {
        request.metadata_mut().insert(TOKEN_HEADER, self.0.clone());
        Ok(request)
    }
}

fn environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Resolves the listener address, and refuses one that is not loopback.
///
/// The listener binds loopback only, and that is not defence in depth but the
/// defence: nothing outside the host can reach it. An address pointing
/// elsewhere would send an execution token off the machine.
fn loopback_endpoint(address: &str) -> Result<String> {
    let address = address.trim();
    let authority = address
        .strip_prefix("http://")
        .or_else(|| address.strip_prefix("https://"))
        .unwrap_or(address)
        .trim_end_matches('/');
    if !is_loopback(host_of(authority)) {
        return Err(Error::configuration(format!(
            "the Neurun gRPC address {address} is not loopback. The listener \
             runs beside the handler and an execution token must not leave the \
             host."
        )));
    }
    Ok(format!("http://{authority}"))
}

fn host_of(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match authority.rsplit_once(':') {
        Some((host, _)) => host,
        None => authority,
    }
}

fn is_loopback(host: &str) -> bool {
    match host.parse::<IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_addresses_are_accepted_in_the_forms_people_write_them() {
        assert_eq!(
            loopback_endpoint("127.0.0.1:7000").unwrap(),
            "http://127.0.0.1:7000"
        );
        assert_eq!(
            loopback_endpoint("http://localhost:7000/").unwrap(),
            "http://localhost:7000"
        );
        assert_eq!(
            loopback_endpoint("[::1]:7000").unwrap(),
            "http://[::1]:7000"
        );
    }

    #[test]
    fn a_routable_listener_is_refused() {
        for address in [
            "10.0.0.4:7000",
            "worker.internal:7000",
            "https://neurun.example.com",
            "[2001:db8::1]:7000",
        ] {
            assert!(
                matches!(loopback_endpoint(address), Err(Error::Configuration(_))),
                "expected {address} to be refused"
            );
        }
    }

    #[test]
    fn an_address_and_a_token_are_both_required() {
        assert!(matches!(
            Browser::new("", "net_exe_secret"),
            Err(Error::Configuration(_))
        ));
        assert!(matches!(
            Browser::new("127.0.0.1:7000", "  "),
            Err(Error::Configuration(_))
        ));
        assert!(Browser::new("127.0.0.1:7000", "net_exe_secret").is_ok());
    }
}
