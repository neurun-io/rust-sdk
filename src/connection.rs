//! The connection every client in this crate makes, and the rules it obeys.
//!
//! There is one listener. The browser, the document store and memory are three
//! services on it, so the endpoint, the credential and the loopback check
//! belong here rather than to whichever of them was written first.
//!
//! Two environment variables are the entire configuration —
//! `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`, both set by the worker.
//! There is no app id and no organization, because a value in a handler's
//! environment is a claim rather than a credential: the token is the one thing
//! this process holds that Neurun minted, and everything it identifies is
//! looked up on the other side.

use std::net::IpAddr;
use std::time::Duration;

use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Request, Status};

use crate::error::{Error, Result};

/// The credential travels here on every call, to every service.
pub(crate) const TOKEN_HEADER: &str = "neurun-execution-token";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What a service client reads from the environment, and checks once.
#[derive(Debug, Clone)]
pub(crate) struct Connection {
    endpoint: String,
    address: String,
    token: String,
}

impl Connection {
    /// Reads `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`.
    pub(crate) fn from_env() -> Result<Self> {
        Self::new(
            environment("NEURUN_GRPC_ADDRESS").unwrap_or_default(),
            environment("NEURUN_EXECUTION_TOKEN").unwrap_or_default(),
        )
    }

    pub(crate) fn new(address: impl Into<String>, token: impl Into<String>) -> Result<Self> {
        let address = address.into().trim().to_string();
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
    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    /// Dials the listener and returns the channel with the credential attached.
    pub(crate) async fn open(&self) -> Result<(Channel, Token)> {
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
        Ok((channel, Token::new(&self.token)?))
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

pub(crate) fn environment(name: &str) -> Option<String> {
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
            Connection::new("", "net_exe_secret"),
            Err(Error::Configuration(_))
        ));
        assert!(matches!(
            Connection::new("127.0.0.1:7000", "  "),
            Err(Error::Configuration(_))
        ));
        assert!(Connection::new("127.0.0.1:7000", "net_exe_secret").is_ok());
    }
}
