//! The loop an app runs to get a browser that remembers yesterday.
//!
//! ```text
//! GET  /v1/browser-profiles/{id}/state   read the cookies and storage
//! OpenSession  →  127.0.0.1              carrying the identity and that state
//!     …drive the returned CDP or BiDi endpoint…
//! CloseSession →  what the browser captured on the way out
//! PUT  /v1/browser-profiles/{id}/state   store it
//! ```
//!
//! The last step is what saves. A session abandoned rather than closed leaves
//! the profile exactly as it was.

use std::future::Future;
use std::net::IpAddr;
use std::time::Duration;

use crate::api::{Api, DEFAULT_TIMEOUT};
use crate::error::{Error, Result};
use crate::profile::{BrowserKind, BrowserProfile, Identity, ProfileState, Protocol};
use crate::proto::browser_service_client::BrowserServiceClient;
use crate::proto::{CloseSessionRequest, OpenSessionRequest};
use crate::wire;

/// Where the browser server listens when nothing says otherwise.
pub const DEFAULT_BROWSER_ADDRESS: &str = "127.0.0.1:1268";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// An app's handle on its browser profiles.
///
/// Holds no session and no browser of its own: it reads profiles from the API
/// and opens sessions against the browser server running beside it.
#[derive(Debug, Clone)]
pub struct BrowserProfiles {
    api: Api,
    browser_endpoint: String,
    browser_address: String,
}

/// Assembles a [`BrowserProfiles`] from parts.
#[derive(Debug, Default, Clone)]
pub struct BrowserProfilesBuilder {
    base_url: Option<String>,
    api_key: Option<String>,
    browser_address: Option<String>,
    timeout: Option<Duration>,
}

impl BrowserProfilesBuilder {
    /// Where the Neurun API lives. Defaults to `NEURUN_URL`.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// The API key to act as. Defaults to `NEURUN_API_KEY`.
    ///
    /// Reading and writing profile state both take `browser_profiles:write`,
    /// because reading state is exporting live sessions.
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Where the browser server listens. Defaults to `NEURUN_BROWSER_ADDR`,
    /// then to [`DEFAULT_BROWSER_ADDRESS`].
    pub fn browser_address(mut self, address: impl Into<String>) -> Self {
        self.browser_address = Some(address.into());
        self
    }

    /// How long an API call may take. Sessions are not bounded by this — an
    /// open may have to download a browser first.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn build(self) -> Result<BrowserProfiles> {
        let base_url = self
            .base_url
            .or_else(|| environment("NEURUN_URL"))
            .unwrap_or_default();
        let api_key = self
            .api_key
            .or_else(|| environment("NEURUN_API_KEY"))
            .unwrap_or_default();
        let address = self
            .browser_address
            .or_else(|| environment("NEURUN_BROWSER_ADDR"))
            .unwrap_or_else(|| DEFAULT_BROWSER_ADDRESS.to_string());
        let browser_endpoint = loopback_endpoint(&address)?;
        Ok(BrowserProfiles {
            api: Api::new(&base_url, &api_key, self.timeout.unwrap_or(DEFAULT_TIMEOUT))?,
            browser_endpoint,
            browser_address: address.trim().to_string(),
        })
    }
}

impl BrowserProfiles {
    /// Reads `NEURUN_URL`, `NEURUN_API_KEY` and `NEURUN_BROWSER_ADDR`.
    pub fn from_env() -> Result<Self> {
        Self::builder().build()
    }

    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Result<Self> {
        Self::builder().base_url(base_url).api_key(api_key).build()
    }

    pub fn builder() -> BrowserProfilesBuilder {
        BrowserProfilesBuilder::default()
    }

    /// Where this handle expects the browser server.
    pub fn browser_address(&self) -> &str {
        &self.browser_address
    }

    /// The profile without its secrets: cookie names and sizes, and whether a
    /// proxy is set, but no cookie values and no proxy URL.
    pub async fn profile(&self, profile_id: &str) -> Result<BrowserProfile> {
        self.api.profile(profile_id).await
    }

    /// The cookie values and storage contents themselves.
    pub async fn state(&self, profile_id: &str) -> Result<ProfileState> {
        self.api.state(profile_id).await
    }

    /// Replaces the profile's stored state.
    ///
    /// Refuses an empty state, because `PUT .../state` replaces rather than
    /// merges and an empty one erases the profile — which is almost always an
    /// accident rather than an intention. To erase on purpose, call
    /// [`clear_state`](Self::clear_state).
    pub async fn save_state(
        &self,
        profile_id: &str,
        state: &ProfileState,
    ) -> Result<BrowserProfile> {
        if state.is_empty() {
            return Err(Error::EmptyState {
                profile_id: profile_id.to_string(),
            });
        }
        self.api.save_state(profile_id, state).await
    }

    /// Erases the profile's stored state: every cookie, every storage entry.
    pub async fn clear_state(&self, profile_id: &str) -> Result<BrowserProfile> {
        self.api
            .save_state(profile_id, &ProfileState::default())
            .await
    }

    /// Opens a browser wearing this profile.
    pub async fn open(&self, profile_id: &str) -> Result<Session<'_>> {
        self.open_with(profile_id, OpenOptions::default()).await
    }

    /// Opens a browser wearing this profile, with the launch decided here.
    pub async fn open_with(&self, profile_id: &str, options: OpenOptions) -> Result<Session<'_>> {
        let profile = self.api.profile(profile_id).await?;
        let state = self.api.state(profile_id).await?;
        let identity = options.identity.as_ref().or(profile.identity.as_ref());

        let mut client = self.connect().await?;
        let response = client
            .open_session(OpenSessionRequest {
                browser: wire::browser_to_proto(profile.browser) as i32,
                identity: identity.map(wire::identity_to_proto),
                state: Some(wire::state_to_proto(&state)),
                executable_path: options.executable_path,
            })
            .await?
            .into_inner();

        Ok(Session {
            profiles: self,
            client,
            info: SessionInfo {
                session_id: response.session_id,
                protocol: wire::protocol_from_proto(response.protocol),
                endpoint_url: response.endpoint_url,
                profile,
            },
            opened_with_state: state,
        })
    }

    /// Opens a session, hands it to `drive`, and closes it afterwards.
    ///
    /// A returned `Ok` closes and saves; an `Err` closes without saving, since
    /// a run that failed part-way is not a state worth keeping.
    pub async fn run<T, E, F, Fut>(
        &self,
        profile_id: &str,
        drive: F,
    ) -> std::result::Result<T, E>
    where
        F: FnOnce(SessionInfo) -> Fut,
        Fut: Future<Output = std::result::Result<T, E>>,
        E: From<Error>,
    {
        let session = self.open(profile_id).await?;
        let info = session.info().clone();
        match drive(info).await {
            Ok(value) => {
                session.close().await?;
                Ok(value)
            }
            Err(error) => {
                // The run already failed; a failure to shut the browser down
                // cleanly must not replace the reason the caller cares about.
                let _ = session.discard().await;
                Err(error)
            }
        }
    }

    async fn connect(&self) -> Result<BrowserServiceClient<tonic::transport::Channel>> {
        let channel = tonic::transport::Endpoint::from_shared(self.browser_endpoint.clone())
            .map_err(|error| {
                Error::configuration(format!(
                    "{} is not a usable browser server address: {error}",
                    self.browser_address
                ))
            })?
            .connect_timeout(CONNECT_TIMEOUT)
            .connect()
            .await
            .map_err(|source| Error::BrowserConnect {
                address: self.browser_address.clone(),
                source,
            })?;
        Ok(BrowserServiceClient::new(channel))
    }
}

/// How to launch, when the profile alone does not say.
#[derive(Debug, Default, Clone)]
pub struct OpenOptions {
    /// Points at an installed browser. Absent downloads one, which can make an
    /// open take minutes.
    pub executable_path: Option<String>,
    /// Used instead of the profile's stored identity.
    ///
    /// The API never returns a proxy URL, so this is how a session runs behind
    /// the proxy a profile was created with: read the profile, put the URL back
    /// into a copy of its identity, and pass it here.
    pub identity: Option<Identity>,
}

/// What an open session is, without the machinery that owns it.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    session_id: String,
    protocol: Protocol,
    endpoint_url: String,
    profile: BrowserProfile,
}

impl SessionInfo {
    /// The browser server's handle on this session.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// What [`endpoint_url`](Self::endpoint_url) speaks.
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Drive the browser from here. It stays open until the session closes.
    pub fn endpoint_url(&self) -> &str {
        &self.endpoint_url
    }

    /// The profile this session is wearing.
    pub fn profile(&self) -> &BrowserProfile {
        &self.profile
    }
}

/// An open browser session. Close it to save what it captured.
///
/// Dropping one without [`close`](Self::close) or [`discard`](Self::discard)
/// abandons it: the profile keeps the state it had, and the browser server
/// keeps the session until it exits.
#[derive(Debug)]
pub struct Session<'a> {
    profiles: &'a BrowserProfiles,
    client: BrowserServiceClient<tonic::transport::Channel>,
    info: SessionInfo,
    opened_with_state: ProfileState,
}

/// Why a close captured state but did not store it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsaved {
    /// The session was Firefox, which carries no profile.
    ///
    /// `rustenium-identity` drives Chrome over CDP only and rustenium exposes
    /// no BiDi storage API, so closing a Firefox session hands back an empty
    /// state. Writing that over a profile that holds cookies would erase it,
    /// and the server cannot tell that apart from a browser that genuinely
    /// holds none — so this SDK does not write back after Firefox at all.
    Firefox,
    /// The browser handed back nothing, over a profile that held something.
    ///
    /// The same erasure in a different coat. The captured state is in
    /// [`Close::state`] and can be stored deliberately.
    EmptyCapture,
}

/// What closing a session did.
#[derive(Debug, Clone)]
pub struct Close {
    /// What the browser captured on the way out.
    pub state: ProfileState,
    /// The profile as it now stands, when the state was stored.
    pub profile: Option<BrowserProfile>,
    /// Why it was not stored, when it was not.
    pub unsaved: Option<Unsaved>,
}

impl Close {
    /// Whether the captured state reached the API.
    pub fn saved(&self) -> bool {
        self.profile.is_some()
    }
}

impl Session<'_> {
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    /// The browser server's handle on this session.
    pub fn session_id(&self) -> &str {
        self.info.session_id()
    }

    /// What [`endpoint_url`](Self::endpoint_url) speaks.
    pub fn protocol(&self) -> Protocol {
        self.info.protocol()
    }

    /// Drive the browser from here.
    pub fn endpoint_url(&self) -> &str {
        self.info.endpoint_url()
    }

    /// The profile this session is wearing.
    pub fn profile(&self) -> &BrowserProfile {
        self.info.profile()
    }

    /// Closes the browser and stores what it captured.
    ///
    /// Returns the capture either way, so a state this refuses to store on its
    /// own is still the caller's to store deliberately.
    pub async fn close(mut self) -> Result<Close> {
        let state = self.close_session().await?;
        if let Some(reason) = self.refusal(&state) {
            return Ok(Close {
                state,
                profile: None,
                unsaved: Some(reason),
            });
        }
        let profile = self
            .profiles
            .api
            .save_state(self.info.profile.id.as_str(), &state)
            .await?;
        Ok(Close {
            state,
            profile: Some(profile),
            unsaved: None,
        })
    }

    /// Closes the browser and stores nothing. The profile stands as it was.
    pub async fn discard(mut self) -> Result<ProfileState> {
        self.close_session().await
    }

    async fn close_session(&mut self) -> Result<ProfileState> {
        let response = self
            .client
            .close_session(CloseSessionRequest {
                session_id: self.info.session_id.clone(),
            })
            .await?
            .into_inner();
        Ok(response.state.map(wire::state_from_proto).unwrap_or_default())
    }

    fn refusal(&self, captured: &ProfileState) -> Option<Unsaved> {
        if self.info.profile.browser == BrowserKind::Firefox {
            return Some(Unsaved::Firefox);
        }
        if captured.is_empty() && !self.opened_with_state.is_empty() {
            return Some(Unsaved::EmptyCapture);
        }
        None
    }
}

fn environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Resolves the browser server address, and refuses one that is not loopback.
///
/// The browser server carries no authentication because only the machine
/// running the app can reach it. Dialling a routable address would send that
/// profile's cookies — and a proxy URL with credentials in it — to an
/// unauthenticated port across the network. The authenticated boundary is the
/// Neurun API, not this one.
fn loopback_endpoint(address: &str) -> Result<String> {
    let address = address.trim();
    let authority = address
        .strip_prefix("http://")
        .or_else(|| address.strip_prefix("https://"))
        .unwrap_or(address)
        .trim_end_matches('/');
    if authority.is_empty() {
        return Err(Error::configuration(
            "a browser server address is required, for example 127.0.0.1:1268",
        ));
    }
    if !is_loopback(host_of(authority)) {
        return Err(Error::configuration(format!(
            "the browser server address {address} is not loopback. It listens \
             without authentication and is meant to run beside the app, so an \
             app must not send cookies or a proxy URL to it over a network."
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
            loopback_endpoint("127.0.0.1:1268").unwrap(),
            "http://127.0.0.1:1268"
        );
        assert_eq!(
            loopback_endpoint("http://localhost:1268/").unwrap(),
            "http://localhost:1268"
        );
        assert_eq!(
            loopback_endpoint("[::1]:1268").unwrap(),
            "http://[::1]:1268"
        );
        assert_eq!(
            loopback_endpoint("127.9.9.9:1268").unwrap(),
            "http://127.9.9.9:1268"
        );
    }

    #[test]
    fn a_routable_browser_server_is_refused() {
        for address in [
            "10.0.0.4:1268",
            "browser.internal:1268",
            "https://browser.example.com",
            "[2001:db8::1]:1268",
        ] {
            assert!(
                matches!(loopback_endpoint(address), Err(Error::Configuration(_))),
                "expected {address} to be refused"
            );
        }
    }

    #[test]
    fn an_empty_state_is_the_one_that_would_erase_a_profile() {
        assert!(ProfileState::default().is_empty());

        let mut state = ProfileState::default();
        state
            .local_storage
            .insert("https://example.com".into(), Default::default());
        assert!(
            state.is_empty(),
            "an origin with no entries stores nothing, so it erases like nothing"
        );

        state
            .local_storage
            .get_mut("https://example.com")
            .unwrap()
            .insert("theme".into(), "dark".into());
        assert!(!state.is_empty());
    }
}
