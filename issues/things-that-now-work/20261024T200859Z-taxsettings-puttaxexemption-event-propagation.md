Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# PutTaxExemption returns a fabricated caseId that names no AWS Support case, and drops the submitted exemptionCertificate

_Filed: 2026-10-24T20:08:59Z_
_Operation: PutTaxExemption | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a tax-exemption submission opens an AWS Support case with the submitted exemption certificate attached, and `PutTaxExemptionResponse$caseId` is "The customer support case ID" of that case — an id of the AWS Support case-id form (`case-<12-digit account>-<realm>-<year>-<16 hex>`) that the caller can then hand to `support:DescribeCases` / `support:DescribeAttachment` to track the review of its certificate. The whole point of the member is to be a handle into Support; that is also why this operation models `AttachmentUploadException` ("Failed to upload the tax exemption document to Amazon Web Services Support case") and `CaseCreationLimitExceededException` ("You've exceeded the Amazon Web Services Support case creation limit for your account").

Actual: the handler mints `caseId` from a private counter in this service's own state — `format!("tax-exemption-case-{:08}", self.minted_exemption_cases)` — and creates nothing anywhere. Two consequences are observable over HTTP:

1. The returned id is not an AWS Support case id and resolves to nothing. This repository ships a working AWS Support emulator (`services/service-com-amazonaws-support-awssupport-20130415`, whose `CreateCase`/`DescribeCases` are implemented and whose `state::new_case_id` mints `case-<account>-<realm>-<year>-<hex>`), so a client that takes the `caseId` from `PutTaxExemption` and calls `support:DescribeCases(caseIdList=[caseId])` gets a not-found instead of the case its submission opened. No case, no communication and no attachment is created for a submission the emulator reports as accepted.
2. `exemptionCertificate` — a `@required` input carrying `documentName` and up to 4 MiB of `documentFile` — is read only by the boundary validator and then dropped: `crate::state::store_exemptions(&input.account_ids, exemption_type, &input.authority, now)` is never given the certificate, and `StoredTaxExemption` has no field for it. Nothing in the emulator ever reflects that a certificate was submitted, which is why the exemption can also never be reviewed.

A faithful emulation records the submission as a Support case (one case per submission, as the handler's own comment intends) with a Support-shaped case id, attaches the certificate to it, and returns that case's id.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.taxsettings#PutTaxExemptionResponse", member="caseId")` -> "The customer support case ID."; `smithy_query(query="shape", shape="AttachmentUploadException")` -> "Failed to upload the tax exemption document to Amazon Web Services Support case."; `smithy_query(query="shape", shape="CaseCreationLimitExceededException")` -> "You've exceeded the Amazon Web Services Support case creation limit for your account." — both modeled on this operation, so the submission is documented as a Support case creation with a document upload. `exemptionCertificate` (`ExemptionCertificate{documentName @required, documentFile @required blob 1..4194304}`) is a required member of `PutTaxExemptionRequest`.

Handler/state: `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_tax_exemption.rs` `handle()` lines ~228-254 — `store.store_exemptions(&input.account_ids, exemption_type, &input.authority, SystemTime::now())` followed by `let case_id = store.mint_exemption_case_id();`, and the certificate is never referenced again. `src/state.rs` `mint_exemption_case_id()` (approx. line 443) returns `format!("tax-exemption-case-{:08}", self.minted_exemption_cases)`; `store_exemptions()` (approx. line 481) takes no certificate and `StoredTaxExemption` (approx. line 783) has no certificate/case members. Cross-service: `services/service-com-amazonaws-support-awssupport-20130415/src/state.rs` `new_case_id()` (approx. line 441) shows the case-id form the Support emulator mints and matches against, and that crate's `create_case.rs` / `describe_cases.rs` are implemented (no `SCAFFOLDED, NOT IMPLEMENTED` header).

Tests: `tests/all/tax_exemptions.rs` lines ~930-937 and ~1098-1107 assert only that `caseId` is a 1..200-character string and that two submissions get different ids; nothing asserts the id is a Support case id, and no test resolves it against the Support emulator — so the behavior is unasserted.

## Steps to Reproduce

1. POST /PutTaxExemption with {"accountIds":["000000000000"],"authority":{"country":"US"},"exemptionType":"<a description GetTaxExemptionTypes publishes>","exemptionCertificate":{"documentName":"resale.pdf","documentFile":"<base64 bytes>"}} and observe 200 with {"caseId":"tax-exemption-case-00000001"}.
2. Hand that caseId to the AWS Support emulator: POST / with X-Amz-Target AWSSupport_20130415.DescribeCases and body {"caseIdList":["tax-exemption-case-00000001"]}. Observe that no case is found — against AWS the id names the case the submission opened.
3. Compare the id's shape with a case the Support emulator creates itself (CreateCase returns `case-<12-digit account>-<realm>-<year>-<16 hex>`): the taxsettings id is not of that form, so no client can treat it as a support case id.
4. Call support:DescribeCommunications / DescribeAttachment looking for the submitted resale.pdf: nothing was uploaded anywhere, and no taxsettings read ever reports the certificate, so the 4 MiB document the request was required to carry left no trace.
