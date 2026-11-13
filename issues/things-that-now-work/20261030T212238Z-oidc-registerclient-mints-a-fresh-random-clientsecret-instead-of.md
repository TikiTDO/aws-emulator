Issue-Source: trace-sweep-divergence
Issue-Trace: sso-oidc-register-public-client-basic
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-oidc-register-public-client-basic
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: oidc|RegisterClient|2xx->2xx|$.clientSecret
Issue-Commit: c0ce0118fd7c38fba0f2e9d7692bcc75f2ecf7c6

# RegisterClient mints a fresh random clientSecret instead of the registration's bound secret

## Resolution (cause declined: a per-run generated credential)

The emulator's behaviour is correct and the one cause is DECLINED, not implemented.
`RegisterClient` mints one registration per accepted call — its input names no existing
registration and carries no idempotency token — and the `clientId`/`clientSecret` it
answers with is the credential the handler generated and PERSISTED for that
registration: `src/ops/register_client.rs` reports exactly the stored record's members
and `src/state.rs` `authenticate_client` then accepts only that stored secret. The
characters of the secret are generated per registration, and the model fixes no value
for the member, so the recorded `{{client_secret_basic}}` is a redacted per-run
credential this (or any) run cannot reproduce; the replay key
`oidc|RegisterClient|2xx->2xx|$.clientSecret` therefore stays failing by construction.

Do NOT implement the remedy the report proposes (resolve an existing registration from
the request's client metadata and re-report its secret): that would make the operation
idempotent on `clientName`/`clientType`, which the model does not support, and would hand
a later caller a credential minted for a different registration.

What the resolution added instead is the test that pins the rule:
`tests/all/client_secret_binding.rs` asserts, over a table of registration shapes, that
the reported secret is the one the store holds for the reported `clientId`, that it keeps
authenticating that registration without changing, that no other registration's secret
authenticates it, and that the same metadata registered again mints a separate
registration while leaving the earlier pair usable.

> _Note: this issue was identified against the code base at commit `c0ce0118fd7c38fba0f2e9d7692bcc75f2ecf7c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterClient

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: RegisterClient's `clientSecret` for a basic public-client registration does not reproduce the secret the recorded flow binds for that registration. Fixture sso-oidc-register-public-client-basic, step task#1.

AWS: RegisterClient (POST /client/register, success 200) answers with `clientSecret` — "A secret string generated for the client. The client will use this string to get authenticated by the service in subsequent calls" (Smithy: RegisterClientResponse$clientSecret -> ClientSecret, @sensitive, string with no length/pattern constraint; the companion `clientSecretExpiresAt` "indicates the time at which the clientId and clientSecret will become invalid"; errors InvalidClientMetadataException/InvalidRequestException/... all 400, InternalServerException 500). The model fixes no literal value for the member, so the requirement exercised here is recorded behavior only: the corpus binds this registration's secret as the fixture variable `client_secret_basic` and the recorded 200 response for this public-client registration carries exactly that bound value, so the secret this flow already holds for the registration is the secret the operation reports. Note that the sibling generated member `clientId` in the same response did NOT diverge — only `clientSecret` did, so this is not the corpus comparing a free-running random value.

Emulator: answers HTTP 200 (status matches) but `$.clientSecret` is an unconditionally fresh value minted per call. Replay detail, verbatim: "$.clientSecret: value_mismatch (expected '{{client_secret_basic}}', got '2ad43ace84b54454ba67c12be666445fd66625ebcb8f47398bc5e25c4659681e')". The emulator value is 64 hexadecimal characters — two de-hyphenated random UUIDs from `state::mint_opaque_value()` — drawn afresh on every RegisterClient call, derived from nothing in the request or in stored state, and the new registration is then inserted keyed only by its brand-new `clientId`, so no earlier registration of the same client is ever resolved. (The quoted 64-hex value is one run's generated value, not a requirement; the requirement is that the member report the registration's bound secret rather than a value independent of it. Hypothesis: the handler treats every RegisterClient call as a brand-new registration, with no resolution of an existing registration for the same client metadata, which is why the flow's bound secret can never be reported.)

Why: a client that carries a registration forward — the device-authorization and refresh-token grants both authenticate with the clientId/clientSecret pair from RegisterClient — gets a secret the recorded flow does not hold. Any consumer that re-drives the recorded flow (a replayed SSO login, a fixture-driven test, a cached ~/.aws/sso registration) presents the secret it already has and the emulator's CreateToken/CreateTokenWithIAM refuses it with InvalidClientException 401 `invalid_client`, because the pair it stored is a different one. The response is a 200 that silently carries a non-reproducible credential, so the failure surfaces one call later, away from the operation that caused it.

Fix: the RegisterClient handler's registration minting and the registration store — services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/register_client.rs (the `client_secret: st::mint_opaque_value()` construction and the following `store.insert_client(client)`) together with services/service-com-amazonaws-ssooidc-awsssooidcservice/src/state.rs (`mint_opaque_value`, `insert_client`, which indexes clients by `client_id` only). RegisterClient must resolve the registration the request names before minting, and report that registration's existing `clientSecret` (and its `clientId`/`clientIdIssuedAt`/`clientSecretExpiresAt`) instead of an unconditionally fresh random secret per call. No error path, status or error message is involved: both sides are 200 and the divergence is confined to the `clientSecret` member.

Traces: sso-oidc-register-public-client-basic#task#1 (RegisterClient, verdict mismatch, expected 200, actual 200, diverging key $.clientSecret).

Related open issues (neither covers this step; the divergence reported no owning issue): 20261029T184601Z-sso-oidc-the-modeled-unsupportedgranttypeexception-is-raised-where.md concerns the grant-type refusal class of CreateToken/CreateTokenWithIAM, not RegisterClient's response members.

## Evidence
Replay of trace set simparity-wire-aws-v1: 4 fixtures replayed, 6 steps, 5 passed, 1 mismatched, score 83.3%, emulator_up true, bootstrap_signal false, divergence_count 1.

Divergence cluster (verbatim fields):
  service: oidc
  operation: RegisterClient
  fixture_id: sso-oidc-register-public-client-basic
  step_key: task#1  (index 1)
  verdict: mismatch
  expected_status: 200, actual_status: 200
  divergence_key: service=oidc|RegisterClient|status=200->200|keys=$.clientSecret
  detail: $.clientSecret: value_mismatch (expected '{{client_secret_basic}}', got '2ad43ace84b54454ba67c12be666445fd66625ebcb8f47398bc5e25c4659681e')
  affected: [sso-oidc-register-public-client-basic#task#1] (affected_count 1)
  stub_path: "" (not blocked by an unimplemented operation), owned_by: [] (not owned by an open issue)

Expected vs actual:
  Expected: 200 with $.clientSecret equal to the secret the flow binds for this registration (fixture variable client_secret_basic); $.clientId and every other asserted member matched.
  Actual: 200 with $.clientSecret = a per-call random 64-hex value unrelated to any bound registration secret.

Emulator source read for this finding:
  services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops/register_client.rs lines 248-262 — `client_id: st::mint_opaque_value(), client_secret: st::mint_opaque_value()` built unconditionally for every call, then lines 301-313 `store.insert_client(client)`.
  services/service-com-amazonaws-ssooidc-awsssooidcservice/src/state.rs lines 810-816 — `mint_opaque_value()` concatenates two de-hyphenated random UUIDs (64 hex chars, 244 random bits); lines 574-576 — `insert_client` keys the map by `client_id` only, so no existing registration is ever matched on client metadata.

Model references used (services/service-com-amazonaws-ssooidc-awsssooidcservice/smithy-model/com-amazonaws-ssooidc-awsssooidcservice.json):
  RegisterClient http POST /client/register, code 200; output members clientId (ClientId), clientSecret (ClientSecret, @sensitive), clientIdIssuedAt (LongTimeStampType, default 0), clientSecretExpiresAt (LongTimeStampType, default 0), authorizationEndpoint/tokenEndpoint (URI).
  ClientSecret: string, @sensitive, no length/pattern constraint.
  RegisterClientResponse$clientSecret doc: "A secret string generated for the client. The client will use this string to get authenticated by the service in subsequent calls."

## Steps to Reproduce
1. Clone the repository and build/boot the emulator (the ssooidc service is bootstrapped and answers: /__admin/health 200, data listener accepting).
2. Replay the wire corpus simparity-wire-aws-v1 and watch fixture `sso-oidc-register-public-client-basic`, step `task#1`.
3. That step issues the fixture's RegisterClient request for a basic public client (clientType="public", a clientName, no unsupported grantTypes) against POST /client/register and compares the 200 body against the recorded response, in which $.clientSecret is the flow's bound value (fixture variable client_secret_basic).
4. Observe the comparison: status 200 matches and $.clientId matches, but $.clientSecret is reported value_mismatch — the emulator returns a freshly minted 64-hex random secret instead of the registration's bound secret.
5. Direct check without the harness: POST /client/register twice with the SAME clientName/clientType and compare the two responses' $.clientSecret (and $.clientId) — the emulator returns a brand-new pair each time and keeps both registrations, so the secret a caller already holds for that client is never reported back.
6. After the fix, re-replay sso-oidc-register-public-client-basic#task#1; it must pass with $.clientSecret equal to the registration's bound secret and the rest of the 200 body unchanged.
