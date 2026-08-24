//! Drives the session loop against a fake control plane.
//!
//! What is worth asserting here is not that the types line up — the compiler
//! does that — but that the four calls happen and that every one of them
//! carries the execution token.

use std::sync::{Arc, Mutex};

use neurun::proto::GetProfileRequest;
use neurun::proto::browser_server::{Browser as BrowserService, BrowserServer};
use neurun::proto::{
    Attribute, CloseSessionRequest, CloseSessionResponse, Cookie, GetCookiesRequest,
    GetCookiesResponse, GetNodeRequest, GetNodeResponse, HumanMouseClickRequest,
    HumanMouseClickResponse, HumanMouseMoveRequest, HumanMouseMoveResponse, HumanScrollYRequest,
    HumanScrollYResponse, HumanScrollYToRequest, HumanScrollYToResponse, HumanTypeRequest,
    HumanTypeResponse, ListProfilesRequest, ListProfilesResponse, MetaEntry, NavigateRequest,
    NavigateResponse, Node, OpenSessionRequest, Profile, ReportResultRequest, ReportResultResponse,
    Session as ProtoSession, SetCookiesRequest, SetCookiesResponse, UpdateProfileRequest,
    WaitForNavigationRequest, WaitForNavigationResponse,
};
use neurun::{Browser, ProfileUpdate};
use tokio::net::TcpListener;
use tonic::{Request, Response, Status};

#[derive(Default)]
struct Recorded {
    opened: Vec<OpenSessionRequest>,
    navigated: Vec<NavigateRequest>,
    waited: Vec<WaitForNavigationRequest>,
    located: Vec<GetNodeRequest>,
    moved: Vec<HumanMouseMoveRequest>,
    clicked: Vec<HumanMouseClickRequest>,
    typed: Vec<HumanTypeRequest>,
    scrolled: Vec<HumanScrollYRequest>,
    scrolled_to: Vec<HumanScrollYToRequest>,
    jarred: Vec<SetCookiesRequest>,
    closed: Vec<CloseSessionRequest>,
    listed: Vec<ListProfilesRequest>,
    updated: Vec<UpdateProfileRequest>,
    tokens: Vec<String>,
}

fn profile() -> Profile {
    Profile {
        id: "bp_1".into(),
        name: "shopper".into(),
        browser: "chrome".into(),
        meta: vec![MetaEntry {
            key: "login".into(),
            value: "ada@example.com".into(),
        }],
        created_at: 1_800_000_000,
        updated_at: 1_800_000_100,
    }
}

struct FakeControlPlane {
    recorded: Arc<Mutex<Recorded>>,
}

impl FakeControlPlane {
    /// Every call carries the token, so every call is checked for it.
    fn token<T>(&self, request: &Request<T>) -> Result<(), Status> {
        let token = request
            .metadata()
            .get("neurun-execution-token")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if token.is_empty() {
            return Err(Status::unauthenticated("no execution token"));
        }
        self.recorded.lock().unwrap().tokens.push(token);
        Ok(())
    }
}

#[tonic::async_trait]
impl BrowserService for FakeControlPlane {
    async fn open_session(
        &self,
        request: Request<OpenSessionRequest>,
    ) -> Result<Response<ProtoSession>, Status> {
        self.token(&request)?;
        let request = request.into_inner();
        let session = ProtoSession {
            id: "bsn_1".into(),
            app_id: "app_1".into(),
            execution_id: "exe_1".into(),
            browser_profile_id: request.browser_profile_id.clone(),
            browser: request.browser.clone(),
            status: "live".into(),
            started_at: 1_800_000_000,
        };
        self.recorded.lock().unwrap().opened.push(request);
        Ok(Response::new(session))
    }

    async fn navigate(
        &self,
        request: Request<NavigateRequest>,
    ) -> Result<Response<NavigateResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .navigated
            .push(request.into_inner());
        Ok(Response::new(NavigateResponse {}))
    }

    async fn wait_for_navigation(
        &self,
        request: Request<WaitForNavigationRequest>,
    ) -> Result<Response<WaitForNavigationResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .waited
            .push(request.into_inner());
        Ok(Response::new(WaitForNavigationResponse {}))
    }

    async fn get_node(
        &self,
        request: Request<GetNodeRequest>,
    ) -> Result<Response<GetNodeResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .located
            .push(request.into_inner());
        Ok(Response::new(GetNodeResponse {
            node: Some(Node {
                node_id: 42,
                local_name: "input".into(),
                node_type: 1,
                attributes: vec![Attribute {
                    name: "name".into(),
                    value: "email".into(),
                }],
                text: String::new(),
                html: "<input name=\"email\">".into(),
                x: 100.0,
                y: 900.0,
                width: 200.0,
                height: 40.0,
            }),
        }))
    }

    async fn human_mouse_move(
        &self,
        request: Request<HumanMouseMoveRequest>,
    ) -> Result<Response<HumanMouseMoveResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .moved
            .push(request.into_inner());
        Ok(Response::new(HumanMouseMoveResponse {}))
    }

    async fn human_mouse_click(
        &self,
        request: Request<HumanMouseClickRequest>,
    ) -> Result<Response<HumanMouseClickResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .clicked
            .push(request.into_inner());
        Ok(Response::new(HumanMouseClickResponse {}))
    }

    async fn human_type(
        &self,
        request: Request<HumanTypeRequest>,
    ) -> Result<Response<HumanTypeResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .typed
            .push(request.into_inner());
        Ok(Response::new(HumanTypeResponse {}))
    }

    async fn human_scroll_y(
        &self,
        request: Request<HumanScrollYRequest>,
    ) -> Result<Response<HumanScrollYResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .scrolled
            .push(request.into_inner());
        Ok(Response::new(HumanScrollYResponse {}))
    }

    async fn human_scroll_y_to(
        &self,
        request: Request<HumanScrollYToRequest>,
    ) -> Result<Response<HumanScrollYToResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .scrolled_to
            .push(request.into_inner());
        Ok(Response::new(HumanScrollYToResponse {}))
    }

    async fn get_cookies(
        &self,
        request: Request<GetCookiesRequest>,
    ) -> Result<Response<GetCookiesResponse>, Status> {
        self.token(&request)?;
        Ok(Response::new(GetCookiesResponse {
            cookies: vec![Cookie {
                name: "session".into(),
                value: "abc".into(),
                domain: "example.com".into(),
                path: "/".into(),
                expires: None,
                secure: true,
                http_only: true,
                same_site: "Lax".into(),
            }],
        }))
    }

    async fn set_cookies(
        &self,
        request: Request<SetCookiesRequest>,
    ) -> Result<Response<SetCookiesResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .jarred
            .push(request.into_inner());
        Ok(Response::new(SetCookiesResponse {}))
    }

    async fn report_result(
        &self,
        request: Request<ReportResultRequest>,
    ) -> Result<Response<ReportResultResponse>, Status> {
        self.token(&request)?;
        Ok(Response::new(ReportResultResponse {}))
    }

    async fn close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .closed
            .push(request.into_inner());
        Ok(Response::new(CloseSessionResponse {}))
    }

    async fn list_profiles(
        &self,
        request: Request<ListProfilesRequest>,
    ) -> Result<Response<ListProfilesResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .listed
            .push(request.into_inner());
        Ok(Response::new(ListProfilesResponse {
            profiles: vec![profile()],
        }))
    }

    async fn get_profile(
        &self,
        request: Request<GetProfileRequest>,
    ) -> Result<Response<Profile>, Status> {
        self.token(&request)?;
        Ok(Response::new(profile()))
    }

    // The door's own rule, mirrored: meta is a run's to change and the rest is
    // not, so anything else is refused unless force says the caller meant it —
    // and forcing still warns.
    async fn update_profile(
        &self,
        request: Request<UpdateProfileRequest>,
    ) -> Result<Response<Profile>, Status> {
        self.token(&request)?;
        let asked = request.into_inner();
        let reserved = asked.name.is_some() || asked.browser.is_some();
        if reserved && !asked.force {
            return Err(Status::permission_denied(
                "a run may change a browser profile's meta and nothing else",
            ));
        }
        let mut answer = Response::new(profile());
        if reserved {
            answer
                .metadata_mut()
                .insert("neurun-warning", "forced a change to name".parse().unwrap());
        }
        self.recorded.lock().unwrap().updated.push(asked);
        Ok(answer)
    }
}

async fn control_plane() -> (String, Arc<Mutex<Recorded>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let service = FakeControlPlane {
        recorded: Arc::clone(&recorded),
    };
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(BrowserServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
    });
    (address, recorded)
}

#[tokio::test]
async fn a_session_opens_navigates_and_closes() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let mut session = browser.open_with("chrome", "bp_1", true).await.unwrap();
    assert_eq!(session.id(), "bsn_1");
    assert_eq!(session.info().status, "live");
    assert_eq!(session.info().browser_profile_id, "bp_1");
    assert_eq!(session.info().started_at, 1_800_000_000);

    session
        .navigate_with("https://example.com", Some("https://ref.example".into()))
        .await
        .unwrap();
    session
        .wait_for_navigation_with(neurun::WaitUntil::NetworkIdle, 5_000)
        .await
        .unwrap();
    session.close(true).await.unwrap();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.opened[0].browser, "chrome");
    assert_eq!(recorded.opened[0].browser_profile_id, "bp_1");
    assert!(recorded.opened[0].load_storage);
    assert_eq!(recorded.navigated[0].session_id, "bsn_1");
    assert_eq!(recorded.navigated[0].url, "https://example.com");
    assert_eq!(
        recorded.navigated[0].referer.as_deref(),
        Some("https://ref.example")
    );
    assert_eq!(recorded.waited[0].session_id, "bsn_1");
    assert_eq!(
        recorded.waited[0].wait_until,
        neurun::WaitUntil::NetworkIdle as i32
    );
    assert_eq!(recorded.waited[0].timeout_ms, 5_000);
    assert_eq!(recorded.closed[0].session_id, "bsn_1");
    assert!(recorded.closed[0].save_storage);
    assert_eq!(
        recorded.tokens,
        vec!["net_exe_secret".to_string(); 4],
        "every call carries the token"
    );
}

#[tokio::test]
async fn a_session_without_a_profile_wears_none() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let mut session = browser.open("safari").await.unwrap();
    session.close(false).await.unwrap();

    assert!(
        recorded.lock().unwrap().opened[0]
            .browser_profile_id
            .is_empty(),
        "a plain browser is the ordinary case"
    );
}

#[tokio::test]
async fn storage_without_a_profile_is_refused_at_both_ends() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    assert!(matches!(
        browser.open_with("chrome", "", true).await,
        Err(neurun::Error::Configuration(_))
    ));
    assert!(
        recorded.lock().unwrap().opened.is_empty(),
        "a session with nowhere to load from is not opened at all"
    );

    let mut session = browser.open("chrome").await.unwrap();
    assert!(matches!(
        session.close(true).await,
        Err(neurun::Error::Configuration(_))
    ));
    assert!(
        session.is_open(),
        "a refused save leaves the session open to close properly"
    );
    session.close(false).await.unwrap();
}

#[tokio::test]
async fn a_closed_session_is_neither_driven_nor_closed_again() {
    let (address, _) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let mut session = browser.open("chrome").await.unwrap();
    session.close(false).await.unwrap();

    assert!(!session.is_open());
    assert!(matches!(
        session.navigate("https://example.com").await,
        Err(neurun::Error::Closed { .. })
    ));
    assert!(matches!(
        session.human_click("button").await,
        Err(neurun::Error::Closed { .. })
    ));
    assert!(matches!(
        session.human_scroll_y(-400).await,
        Err(neurun::Error::Closed { .. })
    ));
    assert!(matches!(
        session.node("input").await,
        Err(neurun::Error::Closed { .. })
    ));
    assert!(matches!(
        session.close(false).await,
        Err(neurun::Error::Closed { .. })
    ));
}

#[tokio::test]
async fn a_form_is_filled_in_the_way_a_person_would() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let mut session = browser.open("chrome").await.unwrap();
    let field = session.node("input[name=email]").await.unwrap();
    session
        .human_scroll_y_to("input[name=email]")
        .await
        .unwrap();
    session
        .human_type_into("input[name=email]", "someone@example.com")
        .await
        .unwrap();
    session.human_click("button[type=submit]").await.unwrap();
    session.close(false).await.unwrap();

    assert_eq!(field.node_id, 42);
    assert_eq!(field.local_name, "input");
    assert_eq!(field.attributes[0].value, "email");
    assert_eq!(field.height, 40.0);

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.located[0].selector, "input[name=email]");
    assert_eq!(
        recorded.located[0].timeout_ms, 0,
        "the short form looks once"
    );
    assert_eq!(
        recorded.scrolled_to[0].align,
        neurun::ScrollAlign::Center as i32,
        "an element scrolled to without saying where lands in the middle"
    );
    assert_eq!(recorded.typed[0].selector, "input[name=email]");
    assert_eq!(recorded.typed[0].text, "someone@example.com");
    assert_eq!(
        (
            recorded.typed[0].delay_min_ms,
            recorded.typed[0].delay_max_ms
        ),
        (0, 0),
        "an unnamed pace is the browser's own, not one invented here"
    );
    assert_eq!(recorded.clicked[0].selector, "button[type=submit]");
    assert_eq!(recorded.clicked[0].count, 1);
    assert_eq!(
        recorded.clicked[0].delay_ms, 0,
        "an unnamed hold is drawn per click rather than fixed here"
    );
    assert!(
        recorded.clicked[0].x.is_none() && recorded.clicked[0].y.is_none(),
        "a selector travels instead of a point, not beside one"
    );
}

#[tokio::test]
async fn a_point_and_a_selector_are_different_ways_to_aim() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let mut session = browser.open("chrome").await.unwrap();
    session.human_mouse_move(120.0, 480.0).await.unwrap();
    session.human_mouse_move_to("a.more").await.unwrap();
    session.human_click_at(120.0, 480.0).await.unwrap();
    session
        .human_click_with("a.more", neurun::MouseButton::Right, 2, 90)
        .await
        .unwrap();
    session.human_scroll_y(-400).await.unwrap();
    session.close(false).await.unwrap();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.moved[0].x, Some(120.0));
    assert_eq!(recorded.moved[0].y, Some(480.0));
    assert!(recorded.moved[0].selector.is_empty());
    assert_eq!(recorded.moved[1].selector, "a.more");
    assert!(recorded.moved[1].x.is_none());

    assert_eq!(recorded.clicked[0].x, Some(120.0));
    assert_eq!(
        recorded.clicked[1].button,
        neurun::MouseButton::Right as i32
    );
    assert_eq!(recorded.clicked[1].count, 2);
    assert_eq!(recorded.clicked[1].delay_ms, 90);

    assert_eq!(
        recorded.scrolled[0].delta_y, -400,
        "scrolling up is a negative distance, which is why the field is signed"
    );
}

#[tokio::test]
async fn a_typing_range_the_wrong_way_round_is_refused_before_it_is_sent() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let mut session = browser.open("chrome").await.unwrap();
    assert!(matches!(
        session.human_type_with("input", "hello", 200, 50).await,
        Err(neurun::Error::Configuration(_))
    ));
    assert!(
        recorded.lock().unwrap().typed.is_empty(),
        "a range that cannot be drawn from never leaves the process"
    );
    session.close(false).await.unwrap();
}

#[tokio::test]
async fn a_jar_goes_out_and_comes_back() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let mut session = browser.open("chrome").await.unwrap();
    let jar = session.cookies().await.unwrap();
    session.set_cookies(jar.clone()).await.unwrap();
    session.close(false).await.unwrap();

    assert_eq!(jar[0].name, "session");
    assert!(jar[0].expires.is_none(), "a session cookie has no date");
    assert_eq!(recorded.lock().unwrap().jarred[0].cookies[0].value, "abc");
}

#[tokio::test]
async fn an_empty_query_asks_for_every_profile() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let found = browser.profiles("").await.unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "bp_1");
    assert_eq!(found[0].meta[0].key, "login");
    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.listed[0].query, "");
    assert_eq!(recorded.listed[0].limit, 0);
}

#[tokio::test]
async fn a_query_and_a_page_size_travel() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    browser.profiles_with("ada", 10).await.unwrap();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.listed[0].query, "ada");
    assert_eq!(recorded.listed[0].limit, 10);
}

#[tokio::test]
async fn one_profile_is_read_by_id() {
    let (address, _) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let found = browser.profile("bp_1").await.unwrap();

    assert_eq!(found.name, "shopper");
}

#[tokio::test]
async fn meta_is_written_and_merges_unless_told_otherwise() {
    let (address, recorded) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let answer = browser
        .update_profile("bp_1", ProfileUpdate::meta([("ticket", "OPS-4")]))
        .await
        .unwrap();

    assert_eq!(answer.id, "bp_1");
    assert_eq!(answer.warning(), None);
    let recorded = recorded.lock().unwrap();
    let asked = &recorded.updated[0];
    assert_eq!(asked.browser_profile_id, "bp_1");
    assert_eq!(asked.meta[0].value, "OPS-4");
    assert!(!asked.replace_meta);
    assert!(asked.name.is_none());
}

#[tokio::test]
async fn a_rename_is_refused_until_it_is_forced_and_warned_about_even_then() {
    let (address, _) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let refused = browser
        .update_profile(
            "bp_1",
            ProfileUpdate {
                name: Some("renamed".into()),
                ..ProfileUpdate::default()
            },
        )
        .await;
    assert!(matches!(
        refused,
        Err(neurun::Error::Neurun(ref status))
            if status.code() == tonic::Code::PermissionDenied
    ));

    let forced = browser
        .update_profile(
            "bp_1",
            ProfileUpdate {
                name: Some("renamed".into()),
                ..ProfileUpdate::default()
            }
            .forced(),
        )
        .await
        .unwrap();

    assert_eq!(forced.warning(), Some("forced a change to name"));
    assert_eq!(forced.id, "bp_1");
}
