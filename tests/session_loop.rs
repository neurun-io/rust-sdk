//! Drives the session loop against a fake control plane.
//!
//! What is worth asserting here is not that the types line up — the compiler
//! does that — but that the four calls happen and that every one of them
//! carries the execution token.

use std::sync::{Arc, Mutex};

use neurun::Browser;
use neurun::proto::browser_server::{Browser as BrowserService, BrowserServer};
use neurun::proto::{
    CloseSessionRequest, CloseSessionResponse, NavigateRequest, NavigateResponse,
    OpenSessionRequest, Session as ProtoSession, WaitForNavigationRequest,
    WaitForNavigationResponse,
};
use tokio::net::TcpListener;
use tonic::{Request, Response, Status};

#[derive(Default)]
struct Recorded {
    opened: Vec<OpenSessionRequest>,
    navigated: Vec<NavigateRequest>,
    waited: Vec<WaitForNavigationRequest>,
    closed: Vec<String>,
    tokens: Vec<String>,
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

    async fn close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        self.token(&request)?;
        self.recorded
            .lock()
            .unwrap()
            .closed
            .push(request.into_inner().session_id);
        Ok(Response::new(CloseSessionResponse {}))
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

    let mut session = browser.open_with("chrome", "bp_1").await.unwrap();
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
    session.close().await.unwrap();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.opened[0].browser, "chrome");
    assert_eq!(recorded.opened[0].browser_profile_id, "bp_1");
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
    assert_eq!(recorded.closed, vec!["bsn_1".to_string()]);
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
    session.close().await.unwrap();

    assert!(
        recorded.lock().unwrap().opened[0]
            .browser_profile_id
            .is_empty(),
        "a plain browser is the ordinary case"
    );
}

#[tokio::test]
async fn a_closed_session_is_neither_driven_nor_closed_again() {
    let (address, _) = control_plane().await;
    let browser = Browser::new(address, "net_exe_secret").unwrap();

    let mut session = browser.open("chrome").await.unwrap();
    session.close().await.unwrap();

    assert!(!session.is_open());
    assert!(matches!(
        session.navigate("https://example.com").await,
        Err(neurun::Error::Closed { .. })
    ));
    assert!(matches!(
        session.close().await,
        Err(neurun::Error::Closed { .. })
    ));
}
