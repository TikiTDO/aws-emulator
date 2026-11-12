Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# CreateConnector never produces a FAILED connector: the referenced private CA and VPC endpoint are never checked

_Filed: 2026-10-24T07:15:25Z_
_Operation: CreateConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator accepts any syntactically valid `CertificateAuthorityArn` (and any `VpcEndpointId`) and unconditionally schedules the connector's transition CREATING -> ACTIVE. A connector can therefore never be observed in the modeled terminal failure status.

Expected (real service): `CreateConnector` answers 202 and provisioning is asynchronous; the connector reaches `ACTIVE` only when the referenced Amazon Web Services Private CA really exists, is usable and is in the caller's Region, and when the named VPC endpoint resolves. Otherwise the connector settles into `ConnectorStatus` `FAILED` and `GetConnector`/`ListConnectors` report a `StatusReason` — `PRIVATECA_RESOURCE_NOT_FOUND` for a CA ARN that names nothing, `PRIVATECA_INVALID_STATE` / `PRIVATECA_ACCESS_DENIED` for one that cannot be used, `VPC_ENDPOINT_RESOURCE_NOT_FOUND` / `VPC_ENDPOINT_DNS_ENTRIES_NOT_FOUND` for a bad `VpcEndpointId`. (Alternatively the real service refuses the call outright with a modeled client error — either way the result is not a healthy ACTIVE connector.)

Actual (emulator): `CreateConnector` with a CA ARN that no `acm-pca` CA was ever created under — or one whose Region/account differ from the request's — stores the record as-is, settles it to `ACTIVE` with `StatusReason` absent, and happily mints challenge passwords against it. `FAILED` and every `ConnectorStatusReason` value are unreachable code paths in this crate.

## Evidence

Smithy model: `ConnectorStatus` = [CREATING, ACTIVE, DELETING, FAILED]; `ConnectorStatusReason` = [INTERNAL_FAILURE, PRIVATECA_ACCESS_DENIED, PRIVATECA_INVALID_STATE, PRIVATECA_RESOURCE_NOT_FOUND, VPC_ENDPOINT_RESOURCE_NOT_FOUND, VPC_ENDPOINT_DNS_ENTRIES_NOT_FOUND]; `Connector$StatusReason` doc: "Information about why connector creation failed, if status is FAILED". Handler `src/ops/create_connector.rs` lines ~287-311 always stores `status: STATUS_CREATING`, `status_reason: None` and `pending: PendingOutcome::Status(STATUS_ACTIVE)`; `src/state.rs` `StoredConnector::settle` only ever applies that pending status. `grep` for `FAILED` across `src/` finds no handler that can set it (only the enum value list in `shared_types.rs`). The CA ARN is stored verbatim and never looked up; `services/service-com-amazonaws-acmpca-acmprivateca/src/state.rs` does hold real `certificate_authorities` state, and cross-service crate dependencies are an established pattern in this repo (e.g. eventbridge -> sqs), so the lookup is feasible. No test asserts a FAILED connector (tests/all/connector_lifecycle.rs and connector_deletion.rs only exercise CREATING/ACTIVE/DELETING).

## Steps to Reproduce

1. POST /connectors with `CertificateAuthorityArn` = arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555 while no such CA was ever created in the acm-pca emulator -> 202 with a ConnectorArn.
2. Wait for the transition span, then GET /connectors/{ConnectorArn}: emulator reports `Status: ACTIVE` with no `StatusReason`; expected a connector that never becomes healthy (`Status: FAILED`, `StatusReason: PRIVATECA_RESOURCE_NOT_FOUND`).
3. POST /challenges with that ConnectorArn -> emulator mints a challenge password against a connector whose CA does not exist.
4. Repeat step 1 with `VpcEndpointId` = vpce-0123456789abcdef0 (no such endpoint) -> emulator still settles to ACTIVE instead of FAILED/VPC_ENDPOINT_RESOURCE_NOT_FOUND.
