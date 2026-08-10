//! Drives the whole loop against a fake API and a fake browser server.
//!
//! What is worth asserting here is not that the types line up — the compiler
//! does that — but that the four steps happen in order and that the last one,
//! the write that saves, happens exactly when it should.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use neurun::proto::browser_service_server::{BrowserService, BrowserServiceServer};
use neurun::proto::{
    CloseSessionRequest, CloseSessionResponse, Cookie as ProtoCookie, OpenSessionRequest,
    OpenSessionResponse, ProfileState as ProtoState, Protocol, StorageOrigin,
};
use neurun::{BrowserProfiles, Protocol as SdkProtocol, Unsaved};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tonic::{Request, Response, Status};

// --- the fake browser server ------------------------------------------------

#[derive(Default)]
struct Recorded {
    opened: Vec<OpenSessionRequest>,
    closed: Vec<String>,
}

struct FakeBrowser {
    recorded: Arc<Mutex<Recorded>>,
    captured: ProtoState,
}

#[tonic::async_trait]
impl BrowserService for FakeBrowser {
    async fn open_session(
        &self,
        request: Request<OpenSessionRequest>,
    ) -> Result<Response<OpenSessionResponse>, Status> {
        self.recorded
            .lock()
            .unwrap()
            .opened
            .push(request.into_inner());
        Ok(Response::new(OpenSessionResponse {
            session_id: "bs_1".into(),
            protocol: Protocol::Cdp as i32,
            endpoint_url: "ws://127.0.0.1:9222/devtools/browser/abc".into(),
        }))
    }

    async fn close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        self.recorded
            .lock()
            .unwrap()
            .closed
            .push(request.into_inner().session_id);
        Ok(Response::new(CloseSessionResponse {
            state: Some(self.captured.clone()),
        }))
    }
}

async fn browser_server(captured: ProtoState) -> (String, Arc<Mutex<Recorded>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let service = FakeBrowser {
        recorded: Arc::clone(&recorded),
        captured,
    };
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(BrowserServiceServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
    });
    (address, recorded)
}

// --- the fake API -----------------------------------------------------------

#[derive(Default)]
struct Calls {
    seen: Vec<(String, String, String)>, // method, path, body
    authorization: Vec<String>,
}

/// Answers the three calls the SDK makes, and records what it was asked.
async fn api_server(profile: &'static str, state: &'static str) -> (String, Arc<Mutex<Calls>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(Mutex::new(Calls::default()));
    let recorded = Arc::clone(&calls);
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(_) => return,
            };
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buffer = [0_u8; 4096];
                // Read until the headers are complete, then until the declared
                // body has arrived. Enough HTTP for a test, and no more.
                let (head_end, length) = loop {
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                    if let Some(end) = find(&raw, b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..end]).to_string();
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while raw.len() < head_end + length {
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                }

                let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                let body = String::from_utf8_lossy(&raw[head_end..]).to_string();
                let mut lines = head.lines();
                let request_line = lines.next().unwrap_or_default().to_string();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let authorization = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization")
                            .then(|| value.trim().to_string())
                    })
                    .unwrap_or_default();

                {
                    let mut calls = recorded.lock().unwrap();
                    calls.seen.push((method.clone(), path.clone(), body));
                    calls.authorization.push(authorization);
                }

                let payload = if path.ends_with("/state") && method == "GET" {
                    state
                } else {
                    profile
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });
    (address, calls)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// --- fixtures ---------------------------------------------------------------

const CHROME_PROFILE: &str = r#"{
  "id": "bp_1",
  "name": "amazon",
  "browser": "chrome",
  "identity": null,
  "cookies": [{"name":"session","domain":".example.com","path":"/","secure":true,"http_only":true,"value_size":42}],
  "storage_origins": ["https://example.com"],
  "created_at": "2026-08-01T00:00:00Z",
  "updated_at": "2026-08-01T00:00:00Z"
}"#;

const FIREFOX_PROFILE: &str = r#"{
  "id": "bp_2",
  "name": "reader",
  "browser": "firefox",
  "identity": null,
  "cookies": [{"name":"session","domain":".example.com","path":"/","secure":true,"http_only":true,"value_size":42}],
  "storage_origins": [],
  "created_at": "2026-08-01T00:00:00Z",
  "updated_at": "2026-08-01T00:00:00Z"
}"#;

const STORED_STATE: &str = r#"{
  "cookies": [{"name":"session","value":"yesterday","domain":".example.com","path":"/","secure":true,"http_only":true,"same_site":"Lax"}],
  "local_storage": {"https://example.com": {"theme": "dark"}},
  "session_storage": {}
}"#;

fn captured_state() -> ProtoState {
    ProtoState {
        cookies: vec![ProtoCookie {
            name: "session".into(),
            value: "today".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            expires: Some(1_900_000_000.0),
            secure: true,
            http_only: true,
            same_site: "Lax".into(),
        }],
        local_storage: vec![StorageOrigin {
            origin: "https://example.com".into(),
            entries: HashMap::from([("theme".to_string(), "light".to_string())]),
        }],
        session_storage: Vec::new(),
    }
}

// --- the tests --------------------------------------------------------------

#[tokio::test]
async fn a_closed_chrome_session_carries_state_in_and_stores_what_came_back() {
    let (api, calls) = api_server(CHROME_PROFILE, STORED_STATE).await;
    let (browser, recorded) = browser_server(captured_state()).await;

    let profiles = BrowserProfiles::builder()
        .base_url(api)
        .api_key("neu_test_abc.secret")
        .browser_address(browser)
        .build()
        .unwrap();

    let session = profiles.open("bp_1").await.unwrap();
    assert_eq!(session.protocol(), SdkProtocol::Cdp);
    assert_eq!(
        session.endpoint_url(),
        "ws://127.0.0.1:9222/devtools/browser/abc"
    );

    let close = session.close().await.unwrap();
    assert!(close.saved(), "a closed Chrome session stores its capture");
    assert!(close.unsaved.is_none());
    assert_eq!(close.state.cookies[0].value, "today");

    // The session opened wearing the state the API held.
    {
        let recorded = recorded.lock().unwrap();
        let opened = &recorded.opened[0];
        let carried = opened.state.as_ref().unwrap();
        assert_eq!(carried.cookies[0].value, "yesterday");
        assert_eq!(carried.local_storage[0].entries["theme"], "dark");
        assert!(
            opened.identity.is_none(),
            "a profile without an identity launches the browser as itself"
        );
        assert_eq!(recorded.closed, vec!["bs_1".to_string()]);
    }

    // And the four steps happened, in order, as one authenticated caller.
    let calls = calls.lock().unwrap();
    let steps: Vec<(String, String)> = calls
        .seen
        .iter()
        .map(|(method, path, _)| (method.clone(), path.clone()))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("GET".into(), "/v1/browser-profiles/bp_1".to_string()),
            ("GET".into(), "/v1/browser-profiles/bp_1/state".to_string()),
            ("PUT".into(), "/v1/browser-profiles/bp_1/state".to_string()),
        ]
    );
    assert!(
        calls
            .authorization
            .iter()
            .all(|header| header == "Bearer neu_test_abc.secret"),
        "every call carries the key: {:?}",
        calls.authorization
    );

    // The write is the whole state, and it is the captured one.
    let written: serde_json::Value = serde_json::from_str(&calls.seen[2].2).unwrap();
    assert_eq!(written["cookies"][0]["value"], "today");
    assert_eq!(written["local_storage"]["https://example.com"]["theme"], "light");
    assert_eq!(written["session_storage"], serde_json::json!({}));
}

#[tokio::test]
async fn a_closed_firefox_session_is_never_written_back() {
    // Firefox carries no profile, so it closes with nothing — and nothing,
    // written over a profile that holds cookies, erases it.
    let (api, calls) = api_server(FIREFOX_PROFILE, STORED_STATE).await;
    let (browser, _) = browser_server(ProtoState::default()).await;

    let profiles = BrowserProfiles::builder()
        .base_url(api)
        .api_key("neu_test_abc.secret")
        .browser_address(browser)
        .build()
        .unwrap();

    let close = profiles.open("bp_2").await.unwrap().close().await.unwrap();

    assert!(!close.saved());
    assert_eq!(close.unsaved, Some(Unsaved::Firefox));
    assert!(
        calls
            .lock()
            .unwrap()
            .seen
            .iter()
            .all(|(method, _, _)| method != "PUT"),
        "the profile was left as it was"
    );
}

#[tokio::test]
async fn an_empty_capture_over_a_stocked_profile_is_refused() {
    let (api, calls) = api_server(CHROME_PROFILE, STORED_STATE).await;
    let (browser, _) = browser_server(ProtoState::default()).await;

    let profiles = BrowserProfiles::builder()
        .base_url(api)
        .api_key("neu_test_abc.secret")
        .browser_address(browser)
        .build()
        .unwrap();

    let close = profiles.open("bp_1").await.unwrap().close().await.unwrap();

    assert_eq!(close.unsaved, Some(Unsaved::EmptyCapture));
    assert!(
        calls
            .lock()
            .unwrap()
            .seen
            .iter()
            .all(|(method, _, _)| method != "PUT")
    );
}

#[tokio::test]
async fn a_discarded_session_stores_nothing() {
    let (api, calls) = api_server(CHROME_PROFILE, STORED_STATE).await;
    let (browser, recorded) = browser_server(captured_state()).await;

    let profiles = BrowserProfiles::builder()
        .base_url(api)
        .api_key("neu_test_abc.secret")
        .browser_address(browser)
        .build()
        .unwrap();

    let captured = profiles
        .open("bp_1")
        .await
        .unwrap()
        .discard()
        .await
        .unwrap();

    assert_eq!(captured.cookies[0].value, "today");
    assert_eq!(recorded.lock().unwrap().closed.len(), 1, "the browser still shut down");
    assert!(
        calls
            .lock()
            .unwrap()
            .seen
            .iter()
            .all(|(method, _, _)| method != "PUT")
    );
}

#[tokio::test]
async fn a_run_that_fails_leaves_the_profile_alone() {
    let (api, calls) = api_server(CHROME_PROFILE, STORED_STATE).await;
    let (browser, recorded) = browser_server(captured_state()).await;

    let profiles = BrowserProfiles::builder()
        .base_url(api)
        .api_key("neu_test_abc.secret")
        .browser_address(browser)
        .build()
        .unwrap();

    let outcome: Result<(), neurun::Error> = profiles
        .run("bp_1", |session| async move {
            assert!(session.endpoint_url().starts_with("ws://"));
            Err(neurun::Error::Configuration("the run failed".into()))
        })
        .await;

    assert!(outcome.is_err());
    assert_eq!(
        recorded.lock().unwrap().closed.len(),
        1,
        "a failed run still closes the browser"
    );
    assert!(
        calls
            .lock()
            .unwrap()
            .seen
            .iter()
            .all(|(method, _, _)| method != "PUT"),
        "and stores nothing"
    );
}

#[tokio::test]
async fn a_run_that_succeeds_stores_the_capture() {
    let (api, calls) = api_server(CHROME_PROFILE, STORED_STATE).await;
    let (browser, _) = browser_server(captured_state()).await;

    let profiles = BrowserProfiles::builder()
        .base_url(api)
        .api_key("neu_test_abc.secret")
        .browser_address(browser)
        .build()
        .unwrap();

    let title: String = profiles
        .run("bp_1", |session| async move {
            Ok::<_, neurun::Error>(session.profile().name.clone())
        })
        .await
        .unwrap();

    assert_eq!(title, "amazon");
    assert!(
        calls
            .lock()
            .unwrap()
            .seen
            .iter()
            .any(|(method, _, _)| method == "PUT")
    );
}
