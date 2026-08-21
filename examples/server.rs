use neurun::{App, Method, Request, Response};

async fn entry(event: serde_json::Value) -> Result<serde_json::Value, neurun::Error> {
    Ok(serde_json::json!({ "echoed": event }))
}

async fn webhook(request: Request) -> Response {
    Response::json(200, &serde_json::json!({ "path": request.path() }))
}

async fn refresh() {
    eprintln!("refresh fired");
}

#[tokio::main]
async fn main() -> Result<(), neurun::Error> {
    App::new()
        .entrypoint(entry)
        .endpoint(Method::Post, "/webhook", webhook)
        .cron("refresh", "* * * * *", refresh)
        .run()
        .await
}
