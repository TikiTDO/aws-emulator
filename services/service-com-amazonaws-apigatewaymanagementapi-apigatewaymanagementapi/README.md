# service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi

Emulator implementation for the `com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi
cargo test  -p service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi
```

## Layout

```
service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   └── com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json      # the Smithy model — drives routing + wire shapes
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState — this service's in-memory state
    ├── scaffolded.rs     # Scaffolded<E> — the 501 stubs return, and `validation` for a 400
    └── ops/              # ONE FILE PER OPERATION
```

## Implementing an operation

Each file in `src/ops/` owns exactly one operation and declares which one
via an `@op` header comment. `build.rs` globs this directory and generates
both the module list and the operation set, so adding a file is all that
is needed — there is no central registry to update.

Every generated stub starts out **scaffolded, not implemented**: it is fully typed
and registered, and it answers **HTTP 501 `NotImplemented`** until you write it. It
deliberately does not return an empty success — a hollow `200` with a zeroed body is
indistinguishable from a real implementation to a client, and to any check that only
asserts the call succeeded.

Each operation is a struct that owns the shared engine (constructor-injected) and
implements `smithy_server_runtime::Operation`. The framework constructs one per
service via `OpName::new(engine)` and drives it through the centralized request walk:
deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.

To implement an operation, replace the `Err(..)` in `handle` with real behaviour and
delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line at the top of the file. That
marker is how the still-stubbed operations are counted:

```sh
grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' */src/ops/
```

```rust
//! @op CreateThing

impl<Eng: Send + Sync + 'static> smithy_server_runtime::Operation for CreateThing<Eng> {
    type Input = CreateThingInput;
    type Output = CreateThingOutput;
    type Error = CreateThingError;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Scaffolded: `Err(crate::scaffolded::Scaffolded::not_implemented("CreateThing"))`.
        // Modeled failures: `Err(CreateThingErrorModeled::Variant(..).into())`.
        /* ... */
    }
}
```

## State

A handler reaches this service's store through the engine held on `self`
(constructor-injected). The framework builds ONE shared engine and hands it to every
operation of this crate through its `new(engine)` constructor, so every handler in
this crate sees the same store:

```rust
async fn handle(
    &self,
    input: Self::Input,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<Self::Output, Self::Error> {
    let Some(state) = crate::state::service_state(&*self.engine) else {
        return Err(/* mounted on an engine that holds no state: a modelled server error */);
    };
    // A LOCKABLE handle, not the value — read and mutate through the guard:
    state.lock().your_field = /* ... */;
}
```

> **Never write `engine::InMemoryEngine::new()` inside a handler.** `engine` is the
> name of a CRATE as well as of the field, so that line compiles — and silently
> gives the request a private, empty store that is dropped when it returns. The write
> looks like it succeeded and the next call sees nothing. Reach the shared store
> through `self.engine`, never a fresh engine.

State lives in the SINGLE shared engine, keyed by type. Reach it through
`crate::state::service_state`, which `src/state.rs` generates.

Do not add a second engine crate — the router and every service must share
one `InMemoryEngine` type or the engine the router injects cannot be handed to this
service's operations.

## The connection registry, and how a connection comes to exist

This service's whole state is a registry of WebSocket connections, keyed by
`ConnectionId`:
`ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState::connections`, beside
the set of addresses that are a data-plane ENDPOINT of an api whose connections it holds
(`::endpoints`). All three modelled operations address one entry of the registry, and the
two refusals are split along the SHAPE of the id the request sent: **a missing entry for a
well-formed id is the single source of truth for `GoneException` (HTTP 410)** — never
minted, closed, or another api's — while an id that could never have been minted
(`state::is_well_formed_connection_id`: standard base64 of `state::CONNECTION_ID_BYTES`
bytes, the 16-character single-padded token API Gateway mints) names no connection
anywhere and is refused `400 BadRequestException` with `Invalid connectionId: <id>` from
the value alone, before the registry is consulted.

None of the three CREATES a connection: in real AWS a connection exists because a
client completed a WebSocket handshake against the API's own invoke endpoint, which
this management API never serves. The emulator therefore registers one through exactly
one seam, and that is the rule this crate follows:

```rust
// Emulator-internal: a test, or the WebSocket data plane of the owning API.
service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::state::register_connection(
    &*engine,
    "Lx9ZKcvPmjQCJhA=",
    // the modelled Identity shape: SourceIp / UserAgent
    state::Identity { source_ip: "192.0.2.10".into(), user_agent: "wscat/5".into() },
    std::time::SystemTime::now(), // becomes ConnectedAt, and the initial LastActiveAt
);
```

* A connection exists **only** between `register_connection` and the
  `DeleteConnection` that removes it. There is deliberately **no auto-registration on
  first reference**: that would make `GetConnection` answer `200` for an id that never
  existed, which the real service never does.
* Only an id of the shape the service mints can be registered, so the shape check above
  can never hide a connection the registry holds.
* A connection is addressed through the ENDPOINT it was established at (`::endpoint`, told
  to the registry by the data plane that opened the socket through
  `state::serve_connections_at`): a request that reached one api's endpoint addresses only
  the connections established there, so another api's live id is `GoneException` on it. An
  address that is no api's endpoint addresses the registry as a whole.
* `ConnectedAt` is fixed at registration and never moves; re-registering a present id
  changes nothing.
* `LastActiveAt` moves only when data actually flows to the connection — a successful
  `PostToConnection`. `GetConnection` reports and never writes.
* `state::connection(&*engine, id)` is the read side of the seam: it returns the stored
  record, including a BOUNDED window of the `Data` payloads `PostToConnection`
  delivered, which no modelled operation exposes. Retention is capped by
  `state::MAX_RETAINED_MESSAGES` and `state::MAX_RETAINED_BYTES` (emulator bounds, not
  AWS limits — AWS retains nothing a caller can read back), so a caller cannot grow
  shared state without limit; `delivered_count` / `delivered_bytes` stay exact across
  eviction.
