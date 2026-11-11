# service-com-amazonaws-sso-swbportalservice

Emulator implementation for the `com-amazonaws-sso-swbportalservice` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-sso-swbportalservice
cargo test  -p service-com-amazonaws-sso-swbportalservice
```

## Layout

```
service-com-amazonaws-sso-swbportalservice/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   └── com-amazonaws-sso-swbportalservice.json      # the Smithy model — drives routing + wire shapes
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsSsoSwbportalserviceState — this service's in-memory state
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

## Where a sign-in and an assignment come from

The portal API creates nothing: it has no operation that creates a sign-in session, an
account or a role assignment. Both events happen elsewhere in the emulator, and this
service RESOLVES them where they are recorded, in the same shared engine:

- the sign-in is the access token IAM Identity Center OIDC's `CreateToken` minted
  (`RegisterClient` -> `StartDeviceAuthorization` -> approval at the portal page ->
  `CreateToken`), which is what `aws sso login` performs;
- the accounts and roles are IAM Identity Center's own account assignments
  (`CreatePermissionSet` + `CreateAccountAssignment`), a role being the NAME of the
  permission set an assignment grants;
- an assigned account's `accountName`/`emailAddress` are the organization's records of that
  account, and are reported absent for an account the emulator holds no record of.

So a client that signs in against the emulator's own OIDC endpoint and then calls
`ListAccounts` is served, with no fixture code involved; an assignment an administrator
withdraws stops being reported on the next request, because nothing is copied into this
service's store. `state::federated_sign_in` does that resolution and
`ComAmazonawsSsoSwbportalserviceState::signed_in` is the one place it is combined with what
this store owns — that a `Logout` ended a sign-in, and the role credentials this portal
minted.

A sign-in and an assignment can also be handed to this store directly, for a test or
whatever stands an emulator up, and are reported exactly the same way:

```rust
let store = service_com_amazonaws_sso_swbportalservice::state::service_state(&*engine)
    .expect("the portal store");
let mut store = store.lock();
store.put_account("111111111111", "Production", "production@example.com", ["AdministratorAccess", "ReadOnly"]);
store.put_session("tok-1", "alice@example.com", ["111111111111"], std::time::Duration::from_secs(8 * 60 * 60));
```

A token that names neither — one the emulator never issued, one past its own lifetime, or
one whose sign-in was logged out — is `UnauthorizedException` (401). That refusal is one
constant for the whole service
(`state::INVALID_SESSION_MESSAGE`, returned through `state::unauthorized_session`): the
Portal API's authentication gate answers the same message on every operation and for
every token, so no handler words it itself. `tests/all/minted_sign_in.rs` drives the whole
loop — sign-in, assignments, listings, credentials and logout — over HTTP against the
services that record them, `tests/all/portal_session.rs` drives it over seeded state, and
`tests/all/unauthorized_rejection.rs` sweeps the gate over
the operations × token classes × the rest of the request.
