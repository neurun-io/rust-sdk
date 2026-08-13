# Neurun Rust SDK

Opens a browser session and drives it.

That is the whole surface. Projects, apps, deployments and keys are made before
a program runs; this is what a program needs while it is running.

## Install

```toml
[dependencies]
neurun = "0.1"
```

Building it compiles `proto/control.proto`, so `protoc` must be on `PATH`.

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

let mut session = Browser::from_env()?.open_with("chrome", "bp_01J...").await?;
let reply = session.execute(command).await?;
session.close().await?;
```

`open` takes a browser and no profile; `open_with` wears one. An empty profile
id is a plain browser, which is the ordinary case.

## Commands are opaque

`execute` takes and returns bytes. The payload is a serialized browser-service
command, and encoding one is an agreement between you and that service: the
control plane brokers sessions, not browser semantics, so it never parses a
command, and a command it has never heard of is not one it can corrupt.

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

`proto/control.proto` is a copy of the contract the control plane serves, and
the client is generated from it at build time, so a field added upstream is a
build failure here rather than a value quietly dropped.

The contract carries two services. This crate speaks `Browser`; `BrowserService`
is between the control plane and `neurun-browser`, and is generated but unused
here except by the tests, which stand a fake control plane up and drive the real
loop against it.

## Tests

```sh
cargo test
```

## What is not here

No entrypoint annotation: the only deployment runtime is Python, and that
annotation lives in the [Python SDK](../python-sdk). No project, app,
deployment, build, user or API-key calls either — those belong to whoever sets
things up, not to the program that runs afterwards.
