# Neurun Rust SDK

Opens a browser session and drives it.

That is the whole surface. Projects, apps, deployments and keys are made before
a program runs; this is what a program needs while it is running.

## Install

```toml
[dependencies]
neurun = "0.1"
```

Building it compiles `proto/browser.proto`, so `protoc` must be on `PATH`.

## Neurun is the broker

```
your program ──gRPC──▶ control plane ──gRPC──▶ neurun-browser
                             ▲
            dashboard ──WS───┘
```

The SDK talks to Neurun and to nothing else. It does not know that a browser
service exists, where it listens, or that one was spawned for this host — and it
cannot be told. It asks for a session, gets an id, and drives that id.

That is the whole reason the dashboard can list a session and stream its
display: nothing is happening on a port only your code knows about.

```rust
use neurun::Browser;

let mut session = Browser::from_env()?
    .open_with("chrome", "bp_01J...", true)
    .await?;
session.navigate("https://example.com").await?;
session.wait_for_navigation().await?;
session.close_with(true).await?;
```

`open` takes a browser and no profile; `open_with` wears one. An empty profile
id is a plain browser, which is the ordinary case.

## What a profile remembers

A profile is where a session's state lives between runs, and both directions
are opt-in: `open_with`'s `load_storage` starts the browser from what the
profile holds, and `close_with`'s `save_storage` writes what the browser holds
back to it. Cookies, for now — the profile also keeps DOM storage, and these
two flags will carry it when the browser does.

The capture **replaces** the profile's state rather than merging into it, which
is the only semantic that can end a login: a cookie the site invalidated has to
be able to disappear, and merging would resurrect it. So save a run you would
be happy to keep, not one that failed halfway through a sign-in.

Both flags need a profile. Asking for either without one is an
`Error::Configuration`, refused before the call goes out.

## Commands

Each command is its own call, shaped after the browser's own function —
`navigate` takes a URL and, with `navigate_with`, an optional referer;
`wait_for_navigation` takes a `WaitUntil` and a timeout through
`wait_for_navigation_with`. The set is small because the browser implements a
small set, and it grows one command at a time: a call this crate does not have
is a call the browser does not support yet, not one silently ignored.

## No heartbeat

**Driving a session renews its lease**, because a browser being commanded is a
browser that is alive. A session left idle past the lease leaves the list, and
one being used never does.

Close on the way out anyway, including on failure. A session left to expire is
correct but slow: the dashboard shows a browser that is not there until the
lease runs out.

## Configuration

| Variable | Meaning |
| --- | --- |
| `NEURUN_GRPC_ADDRESS` | `127.0.0.1:<port>`, the control plane inside the worker. |
| `NEURUN_EXECUTION_TOKEN` | Proves the caller is this execution. |

That is the entire environment. There is no app id, because **an app id in an
environment variable is a claim, not a credential** — the process holding it is
your own code and could change it. The token is the one thing it holds that
Neurun minted; the organization, the app and the execution all live on the
other side of the lookup.

The token travels in `neurun-execution-token` metadata on every call. An
address that is not loopback is refused: the listener runs beside the handler,
and a token must not leave the host.

## Drift

`proto/browser.proto` is a copy of the contract the control plane serves, and
the client is generated from it at build time, so a field added upstream is a
build failure here rather than a value quietly dropped.

This is the SDK's whole contract: what `neurun-browser` speaks to the control
plane is a separate file, `browserservice.proto`, that this crate never sees.
The server side of `browser.proto` is generated too, though this crate is a
client — it is what lets the tests stand a fake control plane up and drive the
real loop against it.

## Tests

```sh
cargo test
```

## What is not here

No entrypoint annotation: the only deployment runtime is Python, and that
annotation lives in the [Python SDK](../python-sdk). No project, app,
deployment, build, user or API-key calls either — those belong to whoever sets
things up, not to the program that runs afterwards.
