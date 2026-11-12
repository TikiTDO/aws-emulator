Issue-Source: trace-sweep-divergence
Issue-Trace: taxsettings-tax-inheritance-toggle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: taxsettings-tax-inheritance-toggle
Issue-Shape-Id: com.amazonaws.taxsettings#TaxSettings
Issue-Key: tax|PutTaxInheritance|4xx->2xx|$
Issue-Key: tax|PutTaxInheritance|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: tax|PutTaxInheritance|4xx->2xx|status
Issue-Commit: 3502977f7c166dd1d891f1ce135a92106599242c

# PutTaxInheritance accepts a non-management-account opt-in with 200 instead of the ValidationException refusal

> _Note: this issue was identified against the code base at commit `3502977f7c166dd1d891f1ce135a92106599242c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTaxInheritance

## Description
One cause.

Cause 1 of 1: PutTaxInheritance does not enforce the organizations management-account gate, so a call made by an account that is not the AWS Organizations management account is accepted and answered 200 with an empty body instead of being refused.

AWS: Smithy (com.amazonaws.taxsettings) PutTaxInheritance POST /PutTaxInheritance, input member heritageStatus -> HeritageStatus enum (OptIn|OptOut), output PutTaxInheritanceResponse is empty, modeled errors ValidationException (httpError 400), ConflictException (409), ResourceNotFoundException (404), InternalServerException (500). ValidationException requires `message` (ErrorMessage, sensitive, length 0..1024) and `errorCode` (ValidationExceptionErrorCode enum, whose values include FieldValidationFailed), with optional `fieldList`. The management-account restriction itself is recorded behavior only: the recording shows AWS refusing the call with HTTP 400, header `x-amzn-errortype: ValidationException`, and a body of `{"errorCode": "FieldValidationFailed", "fieldList": null, "message": "Only AWS orgs management account is allowed to set tax inheritance."}`. That message is entirely fixed text for this error class — it names no request value, no account id and no member — so it is reproduced verbatim; `errorCode` is the fixed enum value FieldValidationFailed and `fieldList` is absent/null because the failure is not attributable to one input member.

Emulator: the request is accepted. The replayed step answers HTTP 200 with an empty JSON body `{}`, no `x-amzn-errortype` header and no error payload at all, i.e. it reports that the inheritance setting was applied.

Why: a client that relies on AWS's behavior cannot distinguish an authorized management-account caller from an unauthorized member-account caller. Code that catches ValidationException/FieldValidationFailed to surface "run this from the management account" never fires, the caller believes tax inheritance was turned on for the organization, and any later read of the inheritance state contradicts the success it was just told about. Negative-path tests for the organization gate pass against the emulator and fail against AWS.

Fix: the PutTaxInheritance handler's authorization/validation stage in the taxsettings service crate. It must determine whether the calling account is the organization's management account before mutating state, and on the negative branch return the modeled ValidationException (HTTP 400, `x-amzn-errortype: ValidationException`) with errorCode FieldValidationFailed, fieldList omitted, and the fixed message text above, rather than falling through to the empty 200 success response. The success path (mutating and persisting heritageStatus) must only run for the management account.

Traces: taxsettings-tax-inheritance-toggle#task#2 and taxsettings-tax-inheritance-toggle#task#3 (same cluster, divergence_key service=tax|PutTaxInheritance|status=400->200).

Related open issues (neither covers this failure): 20261103T181220Z-tax-gettaxexemptiontypes-publishes-a-7-row-invented-catalog-instead.md and 20261109T182821Z-tax-gettaxregistration-omits-statusreason-so-a-rejected-trn-reports.md own the other two tax divergences in this replay. Previously resolved issues 20261024T151653Z-taxsettings-puttaxinheritance-wrong-behavior.md, 20261026T235903Z-tax-two-aws-outcomes-the-emulator-can-never.md and 20261029T100532Z-taxsettings-puttaxinheritance-event-propagation.md touched this operation but are closed; the gate is now unconditionally permissive rather than unconditionally refusing.

## Evidence
Fixture taxsettings-tax-inheritance-toggle, step task#2 (affected: task#2, task#3), operation PutTaxInheritance, verdict mismatch.

Expected (recorded AWS response):
  status: 400
  headers: x-amzn-errortype: ValidationException
  body: {"errorCode": "FieldValidationFailed", "fieldList": null, "message": "Only AWS orgs management account is allowed to set tax inheritance."}

Actual (emulator):
  status: 200
  headers: x-amzn-errortype: <missing>
  body: {} (empty_body)

Replay detail, verbatim:
  status: expected 400, got 200
  $: empty_body (expected {'errorCode': 'FieldValidationFailed', 'fieldList': None, 'message': 'Only AWS orgs management account is allowed to set tax inheritance.'}, got {})
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)

Message breakdown: "Only AWS orgs management account is allowed to set tax inheritance." is fixed text in full for this error class; it interpolates nothing from the request or from emulator state. errorCode is the fixed ValidationExceptionErrorCode value FieldValidationFailed; fieldList is null/absent.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture taxsettings-tax-inheritance-toggle, step task#2 (and task#3): POST /PutTaxInheritance with a heritageStatus value (OptIn/OptOut) as an account that is NOT the AWS Organizations management account.
3. Observe the emulator answers HTTP 200 with an empty body `{}` and no x-amzn-errortype header.
4. AWS answers HTTP 400 with x-amzn-errortype: ValidationException and body {"errorCode":"FieldValidationFailed","fieldList":null,"message":"Only AWS orgs management account is allowed to set tax inheritance."}.
