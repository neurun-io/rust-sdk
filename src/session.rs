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

use std::ops::Deref;

use tonic::Status;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

use crate::connection::{Connection, Token};
use crate::error::{Error, Result};
use crate::proto::browser_client::BrowserClient;
use crate::proto::{
    CloseSessionRequest, Cookie, EvalJsRequest, GetCookiesRequest, GetNodeRequest,
    GetProfileRequest, HumanMouseClickRequest, HumanMouseMoveRequest, HumanScrollYRequest,
    HumanScrollYToRequest, HumanTypeRequest, ListProfilesRequest, MetaEntry, MouseButton,
    NavigateRequest, Node, OpenSessionRequest, Profile, ScrollAlign, ScrollIntoViewRequest,
    SetCookiesRequest, UpdateProfileRequest, WaitForNavigationRequest, WaitUntil,
};

/// Where an answer says what it let through that it would rather have refused.
/// gRPC carries it, so no message has a warning field and nothing has to be
/// parsed out of a success.
pub const WARNING_HEADER: &str = "neurun-warning";

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
    connection: Connection,
}

impl Browser {
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

    /// Finds the organization's browser profiles, newest first.
    ///
    /// `query` is matched, case-insensitively, against a profile's id, name,
    /// browser and meta — keys and values both. Empty matches every profile, so
    /// this is the whole list as well as the search.
    pub async fn profiles(&self, query: impl Into<String>) -> Result<Vec<Profile>> {
        self.profiles_with(query, 0).await
    }

    /// The same, with a page size. Zero takes the server's own.
    pub async fn profiles_with(
        &self,
        query: impl Into<String>,
        limit: u32,
    ) -> Result<Vec<Profile>> {
        let mut client = self.connect().await?;
        let found = client
            .list_profiles(ListProfilesRequest {
                query: query.into(),
                limit,
            })
            .await?
            .into_inner();
        Ok(found.profiles)
    }

    /// Reads one profile by id.
    pub async fn profile(&self, profile_id: impl Into<String>) -> Result<Profile> {
        let mut client = self.connect().await?;
        Ok(client
            .get_profile(GetProfileRequest {
                browser_profile_id: profile_id.into(),
            })
            .await?
            .into_inner())
    }

    /// Changes a profile.
    ///
    /// Meta is the only field a run may change. The rest describes the persona
    /// the account chose, so asking for it comes back
    /// [`PermissionDenied`](tonic::Code::PermissionDenied) unless
    /// [`ProfileUpdate::force`] says the caller meant it — and even then the
    /// answer carries a warning, which is why this returns a [`Warned`].
    pub async fn update_profile(
        &self,
        profile_id: impl Into<String>,
        update: ProfileUpdate,
    ) -> Result<Warned<Profile>> {
        let mut client = self.connect().await?;
        let answer = client
            .update_profile(UpdateProfileRequest {
                browser_profile_id: profile_id.into(),
                name: update.name,
                browser: update.browser,
                meta: update.meta,
                replace_meta: update.replace_meta,
                force: update.force,
            })
            .await?;
        let warning = answer
            .metadata()
            .get(WARNING_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(Warned {
            value: answer.into_inner(),
            warning,
        })
    }

    async fn connect(&self) -> Result<Client> {
        let (channel, token) = self.connection.open().await?;
        Ok(BrowserClient::with_interceptor(channel, token))
    }
}

/// A change to a browser profile, field by field.
///
/// Meta is the only field a run may change; the rest need [`force`](Self::force).
#[derive(Debug, Clone, Default)]
pub struct ProfileUpdate {
    /// The labels to write. Empty writes nothing, unless `replace_meta` says to
    /// empty the map.
    pub meta: Vec<MetaEntry>,
    /// Swap the whole map, which is how a key is removed. Without it the keys
    /// given are written and the rest are left alone — two runs labelling the
    /// same profile know different things, and neither should erase the
    /// other's.
    pub replace_meta: bool,
    /// A rename, which is not a run's to make.
    pub name: Option<String>,
    /// Redraws the whole identity for that browser, because the fields of one
    /// are bound together and there is no swapping this one alone.
    pub browser: Option<String>,
    /// Apply the fields a run may not change instead of being refused them.
    pub force: bool,
}

impl ProfileUpdate {
    /// Writes these labels over whatever the profile already holds.
    pub fn meta<K, V>(entries: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            meta: entries
                .into_iter()
                .map(|(key, value)| MetaEntry {
                    key: key.into(),
                    value: value.into(),
                })
                .collect(),
            ..Self::default()
        }
    }

    /// Makes these labels the whole map, dropping any key not named.
    pub fn replacing_meta<K, V>(entries: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            replace_meta: true,
            ..Self::meta(entries)
        }
    }

    /// Applies the fields a run may not otherwise change.
    pub fn forced(mut self) -> Self {
        self.force = true;
        self
    }
}

/// An answer, and what the control plane warned about while giving it.
///
/// Derefs to the answer, so a caller that does not care reads straight through
/// it. One that does asks for [`warning`](Self::warning) — forcing a change
/// silences the refusal, not the warning.
#[derive(Debug, Clone)]
pub struct Warned<T> {
    value: T,
    warning: Option<String>,
}

impl<T> Warned<T> {
    /// What the answer warned about, if anything.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T> Deref for Warned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
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

    /// Puts the first element `selector` matches in the middle of the viewport,
    /// at once.
    pub async fn scroll_into_view(&mut self, selector: impl Into<String>) -> Result<()> {
        self.scroll_into_view_with(selector, ScrollAlign::Center)
            .await
    }

    /// Puts the element where `align` asks for it, at once.
    ///
    /// The same aim as [`human_scroll_y_to_with`](Self::human_scroll_y_to_with)
    /// taken a different way: the jump is instant and nothing about it looks
    /// like a hand. Reach for it where the scrolling is a means to a later
    /// command rather than something a page is meant to watch.
    pub async fn scroll_into_view_with(
        &mut self,
        selector: impl Into<String>,
        align: ScrollAlign,
    ) -> Result<()> {
        self.open_or_closed()?;
        self.client
            .scroll_into_view(ScrollIntoViewRequest {
                session_id: self.info.id.clone(),
                selector: selector.into(),
                align: align as i32,
            })
            .await?;
        Ok(())
    }

    /// Runs a JavaScript expression in the page and hands back its value.
    pub async fn eval_js<T: serde::de::DeserializeOwned>(
        &mut self,
        expression: impl Into<String>,
    ) -> Result<T> {
        self.eval_js_with(expression, false).await
    }

    /// Runs the expression, waiting for a promise it evaluates to when
    /// `await_promise` says to.
    ///
    /// An expression, not a program: it is evaluated for the value it produces,
    /// so a statement is a syntax error rather than a value of nothing. Without
    /// `await_promise` a promise is the value, and a promise is a handle
    /// nothing outside the page can read. What the page threw comes back as the
    /// error, because a script that failed and one that returned nothing must
    /// not read alike.
    pub async fn eval_js_with<T: serde::de::DeserializeOwned>(
        &mut self,
        expression: impl Into<String>,
        await_promise: bool,
    ) -> Result<T> {
        self.open_or_closed()?;
        let evaluated = self
            .client
            .eval_js(EvalJsRequest {
                session_id: self.info.id.clone(),
                expression: expression.into(),
                await_promise,
            })
            .await?
            .into_inner();
        serde_json::from_slice(&evaluated.result).map_err(|err| {
            Error::Neurun(Status::internal(format!(
                "the page's value is not the shape this call asked for: {err}"
            )))
        })
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

#[cfg(test)]
mod tests {
    use super::*;

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
