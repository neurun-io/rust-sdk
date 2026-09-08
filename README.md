# Neurun Rust SDK

Opens a browser session and drives it, stores what it found, and remembers
where it stopped.

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

## Finding a profile, and labelling it

`profiles` searches the organization's profiles and returns every one when given
nothing to search for:

```rust
let browser = Browser::from_env()?;

for found in browser.profiles("").await? {
    println!("{} {} {:?}", found.id, found.name, found.meta);
}

let shoppers = browser.profiles("ada@").await?;
```

The term is matched, case-insensitively, against a profile's id, name, browser
and meta — keys and values both.

`meta` is the account's own labels: whatever is worth keeping about a profile
that Neurun has no opinion on. Writing merges, so a run that learns one fact
does not erase what another run knew:

```rust
browser
    .update_profile("bp_01J...", ProfileUpdate::meta([("last_run", "2026-08-25")]))
    .await?;
```

`ProfileUpdate::replacing_meta` swaps the whole map instead, which is how a key
is removed. Nothing redacts meta, so it is the wrong place for a credential.

**Meta is the only field a run may change.** `name` and `browser` describe the
persona the account chose, so asking for them comes back `PermissionDenied`
unless `ProfileUpdate::forced` says the caller meant it. Forcing silences the
refusal, not the warning — which is why the answer is a `Warned<Profile>`. It
derefs to the profile, so a caller that does not care reads straight through it:

```rust
let updated = browser
    .update_profile(
        "bp_01J...",
        ProfileUpdate { name: Some("renamed".into()), ..Default::default() }.forced(),
    )
    .await?;

if let Some(warning) = updated.warning() {
    eprintln!("{warning}");
}
println!("{}", updated.name);
```

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

Each command is its own call, shaped after the browser's own function. The set
grows one command at a time: a call this crate does not have is a call the
browser does not support yet, not one silently ignored.

| | |
| --- | --- |
| `navigate`, `wait_for_navigation` | drive the page |
| `node` | what an element is, where it is, what it says |
| `human_mouse_move`, `human_mouse_move_to` | the pointer |
| `human_click`, `human_click_at`, `human_click_with` | press it |
| `human_type`, `human_type_into`, `human_type_with` | the keyboard |
| `human_scroll_y`, `human_scroll_y_to` | the wheel |
| `scroll_into_view` | the same aim, jumped rather than turned |
| `eval_js` | what the page's own scripts would see |
| `cookies`, `set_cookies` | the jar |

The short form is the ordinary case and `_with` takes the full set — `navigate`
against `navigate_with`, `human_click` against `human_click_with`.

```rust
session.human_scroll_y_to("input[name=email]").await?;
session.human_type_into("input[name=email]", "someone@example.com").await?;
session.human_click("button[type=submit]").await?;
session.wait_for_navigation().await?;

let ids: Vec<String> = session
    .eval_js("[...document.querySelectorAll('.row')].map(r => r.id)")
    .await?;
```

`scroll_into_view` and `eval_js` are the two commands that are not human, and
both say so in their names. Reach for either where the driving is a means to
something else rather than something a page is meant to watch. `eval_js` takes
an expression, not a program, and deserializes what the page produced into
whatever type the call asks for.

### Elements are named by selector

Every call takes a CSS selector and looks it up again, so nothing goes stale
across a navigation and there is no handle to release. `node` reports the
browser's own node id so two matches can be told apart — not so one can be
addressed. Where a command takes both a selector and a point the selector wins:
an element knows where it is, and a caller holding a rectangle from before the
last scroll does not.

### Why the input is human

A pointer that teleports, a key held for exactly the same number of
milliseconds every time, a scroll that arrives in one jump — each is a thing no
hand does, and each is cheap for a page to notice. `human_mouse_move` walks a
Bezier curve drawn fresh for the move; `human_click` holds the button for a
length that is drawn rather than fixed; `human_type` draws a hold per key;
`human_scroll_y` eases to a stop.

They are slower for that reason, and that is the trade. A whole gesture is one
call rather than a stream of events, because the pacing has to happen beside the
browser — an event per round trip would leave the network writing the rhythm.

`human_scroll_y` moves the y axis only. The browser's own scroll takes an x
distance and drops it, and a field nothing reads is worse than no field.

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

## Storage

An execution ends and its process goes. Two places outlive it:

```rust
use neurun::{Documents, Memory};
use serde_json::json;

let mut people = Documents::from_env()?.collection("people");
let stored = people.insert(&json!({"name": "ada", "age": 36})).await?;
people.update(&stored.id, &json!({"age": 37})).await?;
let found = people.find(&json!({"age": {"$gte": 30}})).await?;

let memory = Memory::from_env()?;
memory.set_with("cursor", &json!({"page": 4}), 3600).await?;
let entry = memory.get("cursor").await?;
```

A **document** is a JSON record you query back. A **collection** is implicit —
`collection()` reaches for a name and goes nowhere; writing to it is what
creates it, and emptying it is what removes it.

**Memory** is a key you already know the name of, in Redis, optionally with a
ttl. There is no filter, because there is nothing to search.

Bodies arrive as `serde_json::Value`, so reading one costs no annotation; call
`parse::<T>()` for a shape you have declared. Neither client takes an
organization — it is resolved from the execution token on the other side and
prefixed onto every key, so a collection name and a memory key are safe to
choose freely.

### The filter

Small and closed:

```text
{"name": "ada"}                  the field equals the value
{"name": {"$ne": "ada"}}         it does not
{"name": {"$in": ["ada", "g"]}}  it equals one of them
{"age": {"$gte": 30}}            ordered comparison, also $gt $lt $lte
{"email": {"$exists": true}}     the field is present
```

A dot walks into a nested object. Entries are ANDed; there is no `$or`. An
operator outside that list is **refused, not ignored** — a filter that silently
drops a clause matches more than you asked for, which is how a delete removes
the wrong records.

`update` merges shallowly and a JSON `null` removes a field; `replace` swaps the
whole body, so a field left out is gone.

## Parsing

Where to pick data out of a page, kept as a definition because the selectors for
a site outlive any one handler that scrapes it:

```rust
use neurun::Parsers;

let parsed = Parsers::from_env()?.parse("product-card", html).await?;

let title = &parsed.output["title"];
let matched = parsed.probes["title"].matches;
let cost = parsed.elapsed;
```

The plane never fetches the page. You send the HTML you already have, which is
what keeps a parse free of egress, robots and proxy policy: fetching is the
browser's job and parsing is this, and an app that wants both does both, in that
order.

A parser is addressed by **name**, unique inside the project the execution token
resolves to. So a name written into a handler reaches nothing outside its own
project, and it survives the definition behind it being rewritten.

Every parse comes back with **probes**: one per field, keyed by its path in the
output, carrying how many elements it matched, which of its selectors did the
matching, and the first value it read. It is what tells a selector that matched
the wrong element from one that matched nothing, which a scrape that silently
went empty needs as much as a builder does. `scopes` is the same, one per
parent. `elapsed` is what the parse itself took, measured on the other side, so
it is the cost of the work rather than of the call.

## Drift

`proto/browser.proto`, `proto/document.proto`, `proto/memory.proto` and
`proto/parser.proto` are copies of the contracts the control plane serves, and
the clients are generated from them at build time, so a field added upstream is
a build failure here rather than a value quietly dropped.

Four services, one listener, one credential. What `neurun-browser` speaks to
the control plane is a separate file, `browserservice.proto`, that this crate
never sees. The server sides are generated too, though this crate is a client —
it is what lets the tests stand a fake control plane up and drive the real loop
against it.

## Tests

```sh
cargo test
```

## What is not here

No entrypoint annotation: the only deployment runtime is Python, and that
annotation lives in the [Python SDK](../python-sdk). No project, app,
deployment, build, user or API-key calls either — those belong to whoever sets
things up, not to the program that runs afterwards. No external storage: a
bucket a client owns is not connected yet.
