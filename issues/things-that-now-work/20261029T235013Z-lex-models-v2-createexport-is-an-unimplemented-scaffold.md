Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models.create-export-rejects-locale-of-nonexistent-bot
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models.create-export-rejects-locale-of-nonexistent-bot|run#4|AssertionError|assert code == "PreconditionFailedException", (
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# lex-models-v2 CreateExport is an unimplemented scaffold stub: a locale export of a nonexistent bot answers 501 NotImplemented instead of 412 PreconditionFailedException

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExport

## Description
One net-new root cause.

Cause 1 of 1: CreateExport never runs any behaviour at all — the handler is still the generated scaffold and returns the 501 `NotImplemented` channel for every request, so neither the happy path nor the nonexistent-bot refusal exists. Failing check: lex-models-v2.create-export-rejects-locale-of-nonexistent-bot.

AWS: CreateExport validates the resourceSpecification against existing state before it creates anything. For a `botLocaleExportSpecification` naming a botId/botVersion pair that does not exist, Lex V2 refuses the call with HTTP 412 and error code `PreconditionFailedException`, message `Bot with id <botId> and version <botVersion> in the request path does not exist.` — the fixed text is `Bot with id `, ` and version `, ` in the request path does not exist.`, and the two variable parts are the `botId` and `botVersion` copied verbatim out of the request's botLocaleExportSpecification (the check sends a syntactically valid 10-alphanumeric botId nothing created, and botVersion `DRAFT`). The refusal is terminal: no export resource is created, so ListExports scoped to that botId/botVersion/localeId stays empty and hands out no nextToken. Smithy (services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json): `PreconditionFailedException` is a modeled client error of this service with httpError 412 and a single `message` member (target ExceptionMessage); `BotLocaleExportSpecification.botId` is @required Id (length 10..10, pattern ^[0-9a-zA-Z]+$) and `botVersion` is @required BotVersion (length 1..5, pattern ^(DRAFT|[0-9]+)$); CreateExport's own modeled @errors list is ConflictException/InternalServerException/ResourceNotFoundException/ServiceQuotaExceededException/ThrottlingException/ValidationException and does NOT include PreconditionFailedException, so the 412 + PreconditionFailedException pairing for this precondition is recorded behavior only, documented at https://docs.aws.amazon.com/lexv2/latest/dg/export.html. CreateExport's modeled success is HTTP 202 with exportId (Id, 10..10), exportStatus (one of Completed|Deleting|Failed|InProgress), fileFormat and the echoed resourceSpecification.

Emulator: the request is accepted by deserialization and validation (the input satisfies every modeled constraint), then the handler returns the scaffold error channel: HTTP 501, error code `NotImplemented`, message `operation CreateExport is scaffolded but not implemented in this emulator`. That answer is identical for a valid bot and for a bot that does not exist, so the emulator has no CreateExport behaviour and no precondition check at all. No export is created either, but for the wrong reason — the operation never reaches state.

Why: a client that exports a bot locale cannot do so at all against this emulator, and a client that relies on the refusal gets the wrong class of failure. `NotImplemented` is a 501 server-side error: botocore/the AWS SDKs classify it as a server fault and several SDK retry policies retry it, so a caller that expects a deterministic client-side 412 `PreconditionFailedException` (the signal that it named a bot/version that does not exist, and must fix its arguments rather than retry) instead burns its retry budget and surfaces a transport-level server error. Error-handling branches keyed on `PreconditionFailedException` — and on the bot-id/version echoed in its message, which is how a caller tells WHICH identifier was wrong — never run. Export/import flows built on CreateExport -> DescribeExport -> downloadUrl cannot be exercised at all.

Fix: the operation's handler, services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_export.rs — `CreateExport::handle` currently ends in `Err(crate::scaffolded::Scaffolded::not_implemented("CreateExport"))`. It needs the resource-existence precondition (resolve the botLocaleExportSpecification's botId + botVersion, and the localeId, against the service store reached via crate::state::service_state, refusing with the 412 PreconditionFailedException and the message template above when the bot/version is absent), plus the export resource creation and the 202 envelope in the success path, and the SCAFFOLDED marker comment at the top of the file deleted once implemented. ListExports must keep returning an empty exportSummaries page for a botId no CreateExport ever succeeded against.

Traces: lex-models-v2.create-export-rejects-locale-of-nonexistent-bot (verdict: mismatch — `CreateExport against a nonexistent bot answered 'NotImplemented', expected 'PreconditionFailedException'`).

## Evidence
Check: lex-models-v2.create-export-rejects-locale-of-nonexistent-bot (verdict: mismatch).

Request: CreateExport with resourceSpecification={"botLocaleExportSpecification": {"botId": <10 alphanumerics that were never created>, "botVersion": "DRAFT", "localeId": "en_US"}}, fileFormat="LexJson".

Expected (AWS): HTTP 412, Error.Code = "PreconditionFailedException", Error.Message = "Bot with id <botId> and version DRAFT in the request path does not exist." where <botId> is the botId from the request and DRAFT is the requested botVersion; then ListExports(botId=<botId>, botVersion="DRAFT", localeId="en_US") returns exportSummaries == [] with no nextToken.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation CreateExport is scaffolded but not implemented in this emulator".

Assertion raised:
  File "check_lex_models_v2_create_export_rejects_locale_of_nonexistent_bot.py", line 59, in run
    assert code == "PreconditionFailedException"
  AssertionError: CreateExport against a nonexistent bot answered 'NotImplemented', expected 'PreconditionFailedException'

Source confirmation in the checkout: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_export.rs line 4 carries the marker `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, and line 187 is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateExport"))`. The ListExports leg of the check was never reached, so it is not independently graded here.

## Steps to Reproduce
1. Boot the emulator and point a botocore `lexv2-models` client at it.
2. Mint a syntactically valid botId that was never created: 10 characters matching ^[0-9a-zA-Z]+$ (e.g. secrets.token_hex(5).upper()).
3. Call CreateExport(resourceSpecification={"botLocaleExportSpecification": {"botId": <that id>, "botVersion": "DRAFT", "localeId": "en_US"}}, fileFormat="LexJson").
4. Observe the ClientError: Code is "NotImplemented" with HTTP 501 and the message "operation CreateExport is scaffolded but not implemented in this emulator". AWS answers HTTP 412 with Code "PreconditionFailedException" and Message "Bot with id <that id> and version DRAFT in the request path does not exist.".
5. Then call ListExports(botId=<that id>, botVersion="DRAFT", localeId="en_US", maxResults=100) and page on nextToken: exportSummaries must stay empty, which is the second half of the check and cannot be reached today.

Replay: run_behavioral_suite and re-run check lex-models-v2.create-export-rejects-locale-of-nonexistent-bot.
