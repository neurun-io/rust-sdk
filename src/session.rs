//! Browser sessions, as a handler sees them.
//!
//! ```text
//! open_with(browser, browser_profile_id?, load_storage?)  →  a session
//! navigate / node / human_click / human_type / …   as many times as needed
//! close_with(save_storage?)                        including on failure
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
use crate::proto::{
    CloseSessionRequest, Cookie, GetCookiesRequest, GetNodeRequest, HumanMouseClickRequest,
    HumanMouseMoveRequest, HumanScrollYRequest, HumanScrollYToRequest, HumanTypeRequest,
    MouseButton, NavigateRequest, Node, OpenSessionRequest, ScrollAlign, SetCookiesRequest,
    WaitForNavigationRequest, WaitUntil,
};

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
        self.open_with(browser, "", false).await
    }

    /// Opens a browser wearing a profile.
    ///
    /// An empty `profile_id` is a plain browser, which is the ordinary case.
    ///
    /// `load_storage` starts the browser from what that profile remembers —
    /// its cookies, for now — and needs a profile to read them from.
    pub async fn open_with(
        &self,
        browser: impl Into<String>,
        profile_id: impl Into<String>,
        load_storage: bool,
    ) -> Result<Session> {
        let browser_profile_id = profile_id.into();
        if load_storage && browser_profile_id.is_empty() {
            return Err(Error::configuration(
                "loading storage needs a profile: a profile is where a \
                 session's cookies are kept, and a browser without one keeps \
                 none.",
            ));
        }
        let mut client = self.connect().await?;
        let session = client
            .open_session(OpenSessionRequest {
                browser: browser.into(),
                browser_profile_id,
                load_storage,
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

/// One open browser. Drive it with [`navigate`](Session::navigate) and
/// [`wait_for_navigation`](Session::wait_for_navigation), and close it.
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

    /// Drives this session's browser to `url`, sending no Referer.
    ///
    /// Also renews the session's lease, which is why there is no heartbeat to
    /// forget.
    pub async fn navigate(&mut self, url: impl Into<String>) -> Result<()> {
        self.navigate_with(url, None).await
    }

    /// Drives this session's browser to `url`.
    ///
    /// `referer` absent sends no Referer at all, which is not the same as
    /// sending an empty one.
    pub async fn navigate_with(
        &mut self,
        url: impl Into<String>,
        referer: Option<String>,
    ) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .navigate(NavigateRequest {
                session_id: self.info.id.clone(),
                url: url.into(),
                referer,
            })
            .await?;
        Ok(())
    }

    /// Blocks until this session's page has navigated as far as [`WaitUntil::Load`].
    ///
    /// Also renews the session's lease, which is why there is no heartbeat to
    /// forget.
    pub async fn wait_for_navigation(&mut self) -> Result<()> {
        self.wait_for_navigation_with(WaitUntil::Unspecified, 0)
            .await
    }

    /// Blocks until this session's page has navigated as far as `wait_until`.
    ///
    /// `timeout_ms` left at zero leaves the browser's own default in place.
    pub async fn wait_for_navigation_with(
        &mut self,
        wait_until: WaitUntil,
        timeout_ms: u32,
    ) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .wait_for_navigation(WaitForNavigationRequest {
                session_id: self.info.id.clone(),
                wait_until: wait_until as i32,
                timeout_ms,
            })
            .await?;
        Ok(())
    }

    /// Describes the first element `selector` matches, looking once.
    pub async fn node(&mut self, selector: impl Into<String>) -> Result<Node> {
        self.node_with(selector, 0).await
    }

    /// Describes the first element `selector` matches, waiting up to
    /// `timeout_ms` for one to appear.
    ///
    /// Zero looks once, which is the difference between an element that is not
    /// there and one that is not there yet.
    pub async fn node_with(
        &mut self,
        selector: impl Into<String>,
        timeout_ms: u32,
    ) -> Result<Node> {
        self.open_or_closed()?;
        let found = self
            .client
            .get_node(GetNodeRequest {
                session_id: self.info.id.clone(),
                selector: selector.into(),
                timeout_ms,
            })
            .await?
            .into_inner();
        found.node.ok_or_else(|| {
            Error::Neurun(Status::internal(
                "the control plane answered with no element, which it does not \
                 do: a selector that matched nothing is an error, not an empty \
                 answer",
            ))
        })
    }

    /// Walks the pointer to a point in the viewport, the way a hand would.
    pub async fn human_mouse_move(&mut self, x: f64, y: f64) -> Result<()> {
        self.moving(None, Some(x), Some(y)).await
    }

    /// Walks the pointer to the centre of the first element `selector` matches.
    ///
    /// The element is scrolled into view if it is not already, because a point
    /// below the fold is one the pointer cannot reach.
    pub async fn human_mouse_move_to(&mut self, selector: impl Into<String>) -> Result<()> {
        self.moving(Some(selector.into()), None, None).await
    }

    async fn moving(
        &mut self,
        selector: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
    ) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .human_mouse_move(HumanMouseMoveRequest {
                session_id: self.info.id.clone(),
                x,
                y,
                selector: selector.unwrap_or_default(),
            })
            .await?;
        Ok(())
    }

    /// Clicks the centre of the first element `selector` matches, once, with
    /// the left button.
    pub async fn human_click(&mut self, selector: impl Into<String>) -> Result<()> {
        self.clicking(
            Some(selector.into()),
            None,
            None,
            MouseButton::Unspecified,
            1,
            0,
        )
        .await
    }

    /// Clicks a point in the viewport, once, with the left button.
    pub async fn human_click_at(&mut self, x: f64, y: f64) -> Result<()> {
        self.clicking(None, Some(x), Some(y), MouseButton::Unspecified, 1, 0)
            .await
    }

    /// Clicks the first element `selector` matches, with everything named.
    ///
    /// `count` above one is a double or triple click, with a pause between that
    /// is drawn rather than fixed. `delay_ms` left at zero leaves the hold to be
    /// drawn too, which is the point — a press that is always the same length is
    /// the tell a fixed one would hand over.
    pub async fn human_click_with(
        &mut self,
        selector: impl Into<String>,
        button: MouseButton,
        count: u32,
        delay_ms: u32,
    ) -> Result<()> {
        self.clicking(Some(selector.into()), None, None, button, count, delay_ms)
            .await
    }

    async fn clicking(
        &mut self,
        selector: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        button: MouseButton,
        count: u32,
        delay_ms: u32,
    ) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .human_mouse_click(HumanMouseClickRequest {
                session_id: self.info.id.clone(),
                x,
                y,
                selector: selector.unwrap_or_default(),
                button: button as i32,
                count,
                delay_ms,
            })
            .await?;
        Ok(())
    }

    /// Types `text` wherever focus already is, at an average typist's pace.
    pub async fn human_type(&mut self, text: impl Into<String>) -> Result<()> {
        self.typing(None, text.into(), 0, 0).await
    }

    /// Clicks the first element `selector` matches and types `text` into it.
    ///
    /// Clicked rather than focused: the click is what a page watches for, and
    /// one it would have refused is one the typing would not have reached
    /// either.
    pub async fn human_type_into(
        &mut self,
        selector: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<()> {
        self.typing(Some(selector.into()), text.into(), 0, 0).await
    }

    /// Types into an element at a pace drawn from `delay_min_ms..=delay_max_ms`,
    /// which is how long each key is held; the gap before the next follows from
    /// it.
    ///
    /// Both zero is an average typist — roughly 60 to 140 ms. A faster one is
    /// nearer 30 to 80, a careful one 100 to 250.
    pub async fn human_type_with(
        &mut self,
        selector: impl Into<String>,
        text: impl Into<String>,
        delay_min_ms: u32,
        delay_max_ms: u32,
    ) -> Result<()> {
        if delay_min_ms > delay_max_ms {
            return Err(Error::configuration(
                "a typing delay range needs its minimum below its maximum.",
            ));
        }
        self.typing(
            Some(selector.into()),
            text.into(),
            delay_min_ms,
            delay_max_ms,
        )
        .await
    }

    async fn typing(
        &mut self,
        selector: Option<String>,
        text: String,
        delay_min_ms: u32,
        delay_max_ms: u32,
    ) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .human_type(HumanTypeRequest {
                session_id: self.info.id.clone(),
                text,
                selector: selector.unwrap_or_default(),
                delay_min_ms,
                delay_max_ms,
            })
            .await?;
        Ok(())
    }

    /// Turns the wheel `delta_y` pixels down the page. Up is negative.
    ///
    /// Only the y axis: the browser's own scroll takes an x distance and drops
    /// it, so there is nothing here to pass one to.
    pub async fn human_scroll_y(&mut self, delta_y: i32) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .human_scroll_y(HumanScrollYRequest {
                session_id: self.info.id.clone(),
                delta_y,
            })
            .await?;
        Ok(())
    }

    /// Scrolls until the first element `selector` matches sits in the middle of
    /// the viewport.
    pub async fn human_scroll_y_to(&mut self, selector: impl Into<String>) -> Result<()> {
        self.human_scroll_y_to_with(selector, ScrollAlign::Center)
            .await
    }

    /// Scrolls until the element rests where `align` asks for it — its top at
    /// the top of the viewport, its middle in the middle, or its bottom at the
    /// bottom.
    pub async fn human_scroll_y_to_with(
        &mut self,
        selector: impl Into<String>,
        align: ScrollAlign,
    ) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .human_scroll_y_to(HumanScrollYToRequest {
                session_id: self.info.id.clone(),
                selector: selector.into(),
                align: align as i32,
            })
            .await?;
        Ok(())
    }

    /// Reads the browser's whole cookie jar.
    ///
    /// Browser-wide rather than per-origin, because that is what a profile
    /// keeps: a partial read written back would delete the rest.
    pub async fn cookies(&mut self) -> Result<Vec<Cookie>> {
        self.open_or_closed()?;
        let captured = self
            .client
            .get_cookies(GetCookiesRequest {
                session_id: self.info.id.clone(),
            })
            .await?
            .into_inner();
        Ok(captured.cookies)
    }

    /// Puts a jar into the browser, on top of what it already holds.
    pub async fn set_cookies(&mut self, cookies: Vec<Cookie>) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .set_cookies(SetCookiesRequest {
                session_id: self.info.id.clone(),
                cookies,
            })
            .await?;
        Ok(())
    }

    /// Refuses a command against a session that has already been closed.
    fn open_or_closed(&self) -> Result<()> {
        if self.is_open {
            return Ok(());
        }
        Err(Error::Closed {
            session_id: self.info.id.clone(),
        })
    }

    /// Stops the browser and drops the session, keeping what it collected.
    ///
    /// `save_storage` captures what the browser holds — its cookies, for
    /// now — into the profile this session wears. The capture replaces the
    /// profile's rather than merging into it, so a cookie the browser no
    /// longer has is a cookie the profile no longer has.
    pub async fn close(&mut self, save_storage: bool) -> Result<()> {
        self.open_or_closed()?;
        if save_storage && self.info.browser_profile_id.is_empty() {
            return Err(Error::configuration(format!(
                "session {} wears no profile, so there is nowhere to save what \
                 it collected",
                self.info.id
            )));
        }
        self.is_open = false;
        self.client
            .close_session(CloseSessionRequest {
                session_id: self.info.id.clone(),
                save_storage,
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
