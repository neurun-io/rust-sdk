# Neurun Rust SDK

Opens a browser wearing a stored profile, and stores what it remembers.

That is the whole surface. Projects, apps, deployments and keys are made before
a program runs; this is what a program needs while it is running.

## Install

```toml
[dependencies]
neurun = "0.1"
```

Building it compiles `proto/browser.proto`, so `protoc` must be on `PATH`.

## The loop

A browser profile has two halves. Its **state** — cookies, localStorage,
sessionStorage — is what makes it worth having: a run that signed in yesterday
is still signed in today. Its **identity** is presentation, and is optional; a
profile without one launches the browser as itself and still carries its state.

The control plane never opens a browser. `neurun-browser` is a separate gRPC
server on loopback beside your program, so the loop is yours to run:

```rust
use neurun::BrowserProfiles;

let profiles = BrowserProfiles::from_env()?;

let orders = profiles
    .run("bp_01J...", |session| async move {
        // CDP for Chrome, BiDi for Firefox.
        scrape(session.endpoint_url()).await
    })
    .await?;
```

`run` reads the profile and its state, opens a session carrying both, and
afterwards closes it and stores what the browser captured. A closure that
returns `Err` closes without storing — a run that failed part-way is not a
state worth keeping.

Take the steps yourself when a run should decide what to keep:

```rust
let session = profiles.open("bp_01J...").await?;
println!("{} speaks {:?}", session.endpoint_url(), session.protocol());

let close = session.close().await?;
if !close.saved() {
    eprintln!("kept nothing: {:?}", close.unsaved);
}
```

`close` returns the capture either way, so a state the SDK will not store on
its own is still yours to store with `save_state`. Dropping a session without
`close` or `discard` abandons it, and the profile keeps the state it had.

## Two ways a profile gets erased

`PUT .../state` **replaces** rather than merges. It has to: the browser hands
back its whole cookie jar, so a cookie missing from the body was deleted, and
merging would resurrect a login the site had already ended. The cost is that
writing an empty state erases the profile.

Firefox is how that happens by accident. It launches, but it carries no
profile — `rustenium-identity` drives Chrome over CDP only, and rustenium
exposes no BiDi storage API — so closing a Firefox session hands back an empty
state, which the server cannot tell apart from a browser that genuinely holds
no cookies. **This SDK never writes back after Firefox**: `close` reports
`Unsaved::Firefox` and returns the capture instead. `Unsaved::EmptyCapture` is
the same refusal for a browser that handed back nothing over a profile that
held something.

`clear_state` is how a profile gets erased on purpose.

## Configuration

| Variable | Meaning |
| --- | --- |
| `NEURUN_URL` | Where the Neurun API lives. |
| `NEURUN_API_KEY` | The key to act as. |
| `NEURUN_BROWSER_ADDR` | Where the browser server listens. Default `127.0.0.1:1268`. |

Both reading and writing profile state take the `browser_profiles:write`
scope, not `:read` — reading state is exporting live sessions.

A browser server address that is not loopback is refused. That server carries
no authentication because only the machine running the program can reach it;
dialling it across a network would send a profile's cookies, and a proxy URL
with credentials in it, to an unauthenticated port. The authenticated boundary
is the Neurun API, not that one.

## The proxy an identity was created with

The API never returns a proxy URL — an identity comes back reporting
`proxy_set` and nothing else — so a session opens without one unless the
program supplies it:

```rust
let mut identity = profiles.profile("bp_01J...").await?.identity.unwrap();
identity.proxy = Some(std::env::var("SCRAPER_PROXY")?);

let session = profiles
    .open_with("bp_01J...", neurun::OpenOptions { identity: Some(identity), ..Default::default() })
    .await?;
```

## Drift

`proto/browser.proto` is a copy of the contract `neurun-browser` serves, and
the client is generated from it at build time. The mapping in `src/wire.rs`
builds the identity with an exhaustive struct literal, so a field added
upstream fails this crate's build rather than being quietly dropped.

## Tests

```sh
cargo test
```

The suite includes the whole loop driven against a fake API and a fake browser
server, which is where the rules above are actually pinned down.

## What is not here

No entrypoint annotation: the only deployment runtime is Python, and that
annotation lives in the [Python SDK](../python-sdk). No project, app,
deployment, build, user or API-key calls either — those belong to whoever sets
things up, not to the program that runs afterwards.
