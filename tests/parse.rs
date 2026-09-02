//! Drives a parse against a fake control plane.
//!
//! What is worth asserting here is not that the types line up — the compiler
//! does that — but that the name and the document travel, that the token goes
//! with them, and that the two shapes the client rebuilds by hand come back the
//! way they went out: probes keyed by path, and micros read as a duration.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use neurun::Parsers;
use neurun::parsers::parsers_server::{Parsers as ParsersService, ParsersServer};
use neurun::parsers::{ParseRequest, ParseResponse, Probe};
use tokio::net::TcpListener;
use tonic::{Request, Response, Status};

const TOKEN_HEADER: &str = "neurun-execution-token";

#[derive(Default)]
struct Recorded {
    parsed: Vec<ParseRequest>,
    tokens: Vec<String>,
}

struct FakeControlPlane {
    recorded: Arc<Mutex<Recorded>>,
}

impl FakeControlPlane {
    fn record(&self, request: &Request<ParseRequest>) {
        let token = request
            .metadata()
            .get(TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let mut recorded = self.recorded.lock().unwrap();
        recorded.tokens.push(token);
        recorded.parsed.push(request.get_ref().clone());
    }
}

#[tonic::async_trait]
impl ParsersService for FakeControlPlane {
    async fn parse(
        &self,
        request: Request<ParseRequest>,
    ) -> Result<Response<ParseResponse>, Status> {
        self.record(&request);
        Ok(Response::new(ParseResponse {
            output: r#"{"title":"Trainers","money":{"amount":129.99}}"#.into(),
            probes: vec![
                Probe {
                    path: "title".into(),
                    matches: 1,
                    selector: "h1.page-title".into(),
                    sample: "Trainers".into(),
                },
                Probe {
                    path: "money.amount".into(),
                    matches: 3,
                    selector: ".price".into(),
                    sample: "129.99".into(),
                },
            ],
            scopes: vec![Probe {
                path: "card".into(),
                matches: 3,
                selector: ".product-card".into(),
                sample: String::new(),
            }],
            elapsed_micros: 1_250,
        }))
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
            .add_service(ParsersServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
    });
    (address, recorded)
}

#[tokio::test]
async fn a_parse_carries_the_name_the_document_and_the_token() {
    let (address, recorded) = control_plane().await;
    let parsers = Parsers::new(address, "net_exe_secret").unwrap();

    parsers
        .parse("product-card", "<h1>Trainers</h1>")
        .await
        .unwrap();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.parsed[0].parser, "product-card");
    assert_eq!(recorded.parsed[0].html, "<h1>Trainers</h1>");
    assert_eq!(recorded.tokens[0], "net_exe_secret");
}

#[tokio::test]
async fn the_output_probes_and_cost_come_back_the_way_they_went_out() {
    let (address, _) = control_plane().await;
    let parsers = Parsers::new(address, "net_exe_secret").unwrap();

    let parsed = parsers
        .parse("product-card", "<html></html>")
        .await
        .unwrap();

    assert_eq!(parsed.output["title"], "Trainers");
    assert_eq!(parsed.output["money"]["amount"], 129.99);
    assert_eq!(parsed.probes["money.amount"].selector, ".price");
    assert_eq!(parsed.probes["title"].matches, 1);
    assert_eq!(parsed.scopes["card"].matches, 3);
    assert_eq!(parsed.elapsed, Duration::from_micros(1_250));
}

#[tokio::test]
async fn the_output_reads_into_a_shape_of_its_own() {
    #[derive(serde::Deserialize)]
    struct Product {
        title: String,
    }

    let (address, _) = control_plane().await;
    let parsers = Parsers::new(address, "net_exe_secret").unwrap();

    let parsed = parsers
        .parse("product-card", "<html></html>")
        .await
        .unwrap();
    let product: Product = parsed.parse().unwrap();

    assert_eq!(product.title, "Trainers");
}
