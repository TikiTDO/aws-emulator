Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# AddProfilePermission accepts a principal that is not an AWS principal

_Filed: 2026-10-24T05:40:20Z_
_Operation: AddProfilePermission | Class: validation_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler validates `action` against the four documented grantable actions and rejects an empty/whitespace `principal` or `statementId`, but performs no check that `principal` is actually an AWS principal. Any non-empty string — "not-a-principal", "hello world", "12345" (a non-12-digit number), "arn:aws:iam::bogus" — is stored as a policy statement's Principal.AWS and later reported by ListProfilePermissions.

Expected (AWS): `principal` is documented as "The AWS principal receiving cross-account permissions. This may be an IAM role or another AWS account ID" — i.e. a 12-digit account id, an account root ARN, or an IAM role/user ARN. Signer validates the resulting resource policy the way every AWS resource-policy API does (the invalid-principal refusal is well-established behavior across AWS resource-policy operations), and refuses a malformed principal with the operation's modeled ValidationException (HTTP 400), writing nothing and leaving the policy's revisionId unchanged.

Actual (emulator): HTTP 200, a new revisionId is minted, and the garbage principal becomes a permanent statement in the profile's policy document (also inflating policySizeBytes), so a caller that relied on AWS rejecting a typo'd account id silently ends up with a policy statement that grants nothing to nobody.

## Evidence

Smithy model: AddProfilePermissionRequest$principal (required String; doc via smithy_query(query="doc", shape="AddProfilePermissionRequest", member="principal"): "The AWS principal receiving cross-account permissions. This may be an IAM role or another AWS account ID."); the operation declares ValidationException (HTTP 400) for exactly this kind of refusal. Handler: src/ops/add_profile_permission.rs validates only `is_grantable_action(&input.action)` and that `principal`/`statementId` are not blank (`value.trim().is_empty()`), then inserts crate::state::permissions::ProfilePermissionRecord with the principal verbatim; crate::state::permissions has helpers for actions (GRANTABLE_ACTIONS/is_grantable_action) but none for principals. Tests: tests/all/profile_permissions.rs only exercises well-formed principals ("123456789012", "arn:aws:iam::123456789012:role/CodeSigner", "arn:aws:iam::210987654321:root") and the empty-principal refusal (a_grant_with_no_principal_or_no_statement_id_is_refused); no test covers a malformed non-empty principal.

## Steps to Reproduce

1. PutSigningProfile PUT /signing-profiles/Perm_Profile with platformId AWSLambda-SHA384-ECDSA (and signingMaterial) to create a profile.
2. AddProfilePermission POST /signing-profiles/Perm_Profile/permissions with body {"statementId":"bad-principal","action":"signer:StartSigningJob","principal":"not-a-principal"}.
3. Observe HTTP 200 with a fresh revisionId instead of HTTP 400 ValidationException naming the invalid principal.
4. GET /signing-profiles/Perm_Profile/permissions and observe the statement is persisted with principal "not-a-principal" and counted in policySizeBytes.
5. Repeat with principal "12345" (not a 12-digit account id) — also accepted.
