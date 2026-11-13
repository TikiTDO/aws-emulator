Issue-Source: trace-sweep-divergence
Issue-Trace: verifiedpermissions-token-authorization-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: verifiedpermissions-token-authorization-error-paths
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|BatchIsAuthorizedWithToken|4xx->4xx|$.message
Issue-Commit: 367d9ab1ca3e929e0a069d7cf8b323225e0ad310

# BatchIsAuthorizedWithToken reports a missing iss claim as a verify failure instead of the decode message

> _Note: this issue was identified against the code base at commit `367d9ab1ca3e929e0a069d7cf8b323225e0ad310`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchIsAuthorizedWithToken

## Description
ONE cause, in verifiedpermissions.

--- Cause 1 of 1: BatchIsAuthorizedWithToken ValidationException message for an identity token missing a required claim ---
AWS: Smithy (com.amazonaws.verifiedpermissions): BatchIsAuthorizedWithToken takes required `policyStoreId` and required `requests`, plus optional `identityToken`/`accessToken` (target Token, sensitive, @length 1..131072, pattern '^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$'), and models ValidationException (errorKind client, httpError 400, defaulted) with a required `message`. The model does not fix the message text, so the text is recorded behavior: when the supplied identityToken is syntactically a JWT but its decoded payload lacks the `iss` claim, the recorded response is HTTP 400 ValidationException with message 'Failed to decode IdentityToken: Invalid body: JSON error: missing field `iss` at line 1 column 15'. Structure of that message: the fixed text is the decode-stage framing 'Failed to decode IdentityToken: Invalid body: JSON error: missing field ' followed by ' at line <line> column <column>'; its variable parts are the name of the absent claim (here `iss`, taken from the claim set the token must supply) and the line/column position taken from the offending payload actually being decoded. The position numbers and the claim name come from the request's own token and are not constants to copy.
Emulator: answers HTTP 400 with the correct ValidationException shape but message 'Failed to verify IdentityToken: the token names no issuer' — it frames the failure as token VERIFICATION rather than DECODING, names no claim, and gives no position.
Why: Verified Permissions separates 'the token could not be decoded into the expected claim set' (the caller must repair the token) from 'the token decoded but failed verification' — bad signature, issuer/audience not trusted (the caller must repair trust configuration). The emulator collapses a decode failure into the verify wording, so a client branching on that distinction draws the wrong conclusion, and the claim name and position a caller needs to repair the token are absent. Any assertion on the documented sentence fails.
Fix: BatchIsAuthorizedWithToken's token-handling path, at the boundary between claim-set deserialization and token verification — a missing required claim must surface the decode-stage message naming the claim and its position, not the verify-stage message. Note the batch operation evidently shares its token handling with the single-request path: the open issue 20261029T192154Z-verifiedpermissions-error-messages-carry-internal-parser-prose-instead-of covers the SAME wording defect on IsAuthorizedWithToken (its Cause 4, fixture verifiedpermissions-token-authorization-error-paths step task#2) but carries no replay key for BatchIsAuthorizedWithToken, so this step is not exercised by that issue's fix gate and is filed separately here.
Traces: fixture verifiedpermissions-token-authorization-error-paths step task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 91 steps, 86 passed, 5 mismatched, 5 divergence clusters — four carry owned_by, this one carries an EMPTY owned_by and an empty stub_path, so it is net-new).

Cause 1 — BatchIsAuthorizedWithToken, service=verifiedpermissions, verdict=mismatch, expected_status 400 / actual_status 400, diverging key $.message:
  expected: 'Failed to decode IdentityToken: Invalid body: JSON error: missing field `iss` at line 1 column 15'
  actual:   'Failed to verify IdentityToken: the token names no issuer'
  divergence_key: service=verifiedpermissions|BatchIsAuthorizedWithToken|status=400->400|keys=$.message
  fixture_id: verifiedpermissions-token-authorization-error-paths, step_key: task#3

The HTTP status (400) and the ValidationException shape match the recording, so the failure path is selected correctly and the defect is confined to the message that path emits.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus against trace set simparity-wire-aws-v1), then inspect fixture verifiedpermissions-token-authorization-error-paths step task#3:

1. Create a policy store (the fixture's earlier steps do this).
2. Call BatchIsAuthorizedWithToken with that policyStoreId, a non-empty `requests` list, and an `identityToken` that is a well-formed three-segment JWT whose payload omits the `iss` claim.
3. Read `message` on the 400 ValidationException. Expect the decode-stage message 'Failed to decode IdentityToken: Invalid body: JSON error: missing field `iss` at line <line> column <column>', with the claim name and the line/column position taken from the token supplied in the request. The emulator instead returns the verify-stage 'Failed to verify IdentityToken: the token names no issuer'.

The step passes only when the status (400, unchanged) and the message string both match.
