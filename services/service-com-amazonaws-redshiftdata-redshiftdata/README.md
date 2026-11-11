# service-com-amazonaws-redshiftdata-redshiftdata

Emulator implementation for the `com-amazonaws-redshiftdata-redshiftdata` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-redshiftdata-redshiftdata
cargo test  -p service-com-amazonaws-redshiftdata-redshiftdata
```

## The SQL engine, and its memory-safety posture (SR-MEM-001)

The Redshift Data API is an HTTP control plane in front of a SQL engine, so this crate
runs submitted statements on a REAL engine in-process: `rusqlite` (SQLite), declared in
`Cargo.toml` with the `bundled` feature. This is a deliberate, documented choice and the
rationale is recorded here so it is visible with the code rather than only in a review:

* **Why an engine crate at all.** The alternative is a hand-written SQL subset, which
  this project forbids for execution planes (never match on source text, never
  synthesise output that looks computed). The emulator's contract for an execution
  plane is a real interpreter or engine, bounded by a wall-clock timeout, a step budget
  and an output cap, run in-process — which is exactly how `src/state.rs` drives it.
* **What `bundled` means.** `libsqlite3-sys` compiles the SQLite C amalgamation at build
  time and links it in, so the process gains a native (unsafe-FFI, C) component. The
  requirement "no crate may use `unsafe`" is about the crates in THIS workspace: this
  crate keeps `#![forbid(unsafe_code)]` and the workspace `unsafe_code = "forbid"` lint,
  and contains no `unsafe` block, function or impl. The native component is a
  third-party dependency, not workspace code, and it is the same engine this workspace
  already links through `vendor/emulator-telemetry`, which depends on `rusqlite` with
  the same `bundled` feature — so this change introduces no new native library to the
  emulator process, only a second user of one already linked. The two components the
  engine crate adds to `Cargo.lock` beside it — `sqlite-wasm-rs`, a `cc`-built SQLite
  for the browser, and the `rsqlite-vfs` filesystem shim it uses — belong to its
  `wasm32` targets: `cargo tree -i sqlite-wasm-rs` (and `-i rsqlite-vfs`) finds no
  dependant on this host at all and reaches them only under `--target all`, so neither
  is built or linked here.
* **What bounds it.** Caller-supplied SQL reaches the engine only through
  `Database::open`, which installs, BEFORE any request is served: a default-deny
  authorizer (every host-reaching action — `ATTACH`/`DETACH`, `VACUUM INTO`, `PRAGMA`,
  extension loading, and any action a future engine version adds — is refused), an
  in-memory connection opened without `SQLITE_OPEN_URI`, `temp_store = MEMORY`,
  `max_page_count` on `main` and `temp`, `hard_heap_limit`, defensive mode, untrusted
  schema, `SQLITE_LIMIT_ATTACHED = 0`, and length, SQL-length, column, expression-depth,
  compound-select and bound-parameter limits. Execution is additionally bounded by a
  wall-clock deadline and a step budget enforced from the engine's progress handler, and
  the retained result set is capped in rows, cells and bytes — with a further cap on what
  the retained statement history holds in results at once, the oldest reported submission
  being dropped until it fits. `load_extension` is not an
  enabled feature of the dependency, so it is not reachable from this crate at all.
  `tests/all/statement_lifecycle.rs` asserts that statements which would reach the host
  are refused and that statements outside those bounds end FAILED.
* **Where it runs.** A submission's execution belongs to the SUBMISSION, not to the
  request that made it: the statement is recorded as `SUBMITTED` before anything runs,
  reports `PICKED` once its turn on its database is its and `STARTED` for as long as the
  engine is really running it, its SQL runs on a blocking pool under the bounds above,
  and its outcome is written
  back by the task that produced it — so a statement reaches a terminal status because
  the engine stopped and never because a caller stopped waiting, and two statements
  submitted at once are really concurrent. The statements of ONE database run one at a
  time and in submission order (`QueueSlot`), which is what makes a transactional batch
  a transaction and what serializes two submissions contending for the same row.
  `tests/all/contended_transactions.rs` asserts it over a table of contention shapes.
* **Alternatives considered.** A pure-Rust SQL engine would remove the native component,
  at the cost of a different SQL dialect and far less exercised code on the same
  untrusted-input path; the engine already linked by the workspace was preferred. If the
  workspace's curated `[workspace.dependencies]` is to own this dependency, the root
  manifest is the place for it — this crate declares it directly only because the root
  manifest is not editable from here.
* **Snapshots of a warehouse (`backup`).** A Redshift Serverless snapshot is a snapshot of
  "all databases in a namespace", and those databases live here — so `state.rs` exposes
  `capture_warehouse` / `restore_warehouse`, which the redshift-serverless control plane
  calls from `CreateSnapshot` and `RestoreFromSnapshot`. Both use the engine's own
  page-level copy (`sqlite3_backup_*`, the dependency's `backup` feature) between two
  connections this crate opened through `Database::open`: no path is named and no file is
  opened, unlike `VACUUM INTO`, which the authorizer denies. A captured database is
  confined exactly as a live one is, is reachable only through that opaque capture value,
  and is bounded by the same `max_page_count` — which is a property of the destination
  CONNECTION, so a restored database keeps it; the control plane bounds how many pages its
  snapshots retain between them. Both are BLOCKING engine work, documented and driven the
  way `run_on` is: the caller puts them on a blocking pool under `COPY_REQUEST_BUDGET` and
  holds no state lock across them, and each copy holds itself to `COPY_TIMEOUT`, checked
  while waiting for a database a statement is running on and between copy steps, so it is
  always the copy's own deadline that ends it.

### The formats and functions this crate does not hand-render

The SQL a caller submits is Amazon Redshift's dialect, and the engine behind this
service is a different SQL engine, so neither the statement nor the functions it
reaches for are read or computed by hand:

* `sqlparser` — the statement is READ by the dialect's own parser and tokenizer
  (`src/state/redshift_sql.rs`): the text of one submission is cut into the statements it
  carries at the semicolons the GRAMMAR reports, a construct the engine spells differently
  (a `::` cast, a schema-qualified name) is re-spelled from the parsed statement, and which
  named placeholders (`:name`) a statement references is read off its tokens, so a `:name`
  inside a string literal or a comment, a `::` cast and a real placeholder are told apart
  by the SQL grammar rather than by a scan of the text (`src/state/sql_parameters.rs`). A
  statement no rewrite applies to is run exactly as the caller wrote it.
* `md-5`, `sha1`, `sha2` — the Redshift hash functions (`MD5`, `SHA`, `SHA1`, `SHA2`) the
  engine does not carry are registered on every connection and COMPUTE the digest
  (`src/state/functions.rs`), beside the string and conditional functions implemented
  there; `tests/all/redshift_dialect.rs` pins them against their published test vectors.
* `csv` — `GetStatementResultV2` answers a statement's rows as CSV text, so the
  separator, the quoting of a value carrying a comma, a quote or a newline, and the
  CRLF record terminator are a CSV writer's rather than this crate's own rendering.

None of them adds a native component: all are pure Rust, and this crate keeps
`#![forbid(unsafe_code)]`.

## Layout

```
service-com-amazonaws-redshiftdata-redshiftdata/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   └── com-amazonaws-redshiftdata-redshiftdata.json      # the Smithy model — drives routing + wire shapes
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsRedshiftdataRedshiftdataState — this service's in-memory state
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
