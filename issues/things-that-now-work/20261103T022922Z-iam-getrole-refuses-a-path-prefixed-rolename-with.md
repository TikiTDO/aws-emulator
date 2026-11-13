Issue-Source: trace-sweep-divergence
Issue-Trace: lakeformation-gtr-lf-path-role-principal-forms
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lakeformation-gtr-lf-path-role-principal-forms
Issue-Shape-Id: com.amazonaws.lakeformation#AWSLakeFormation
Issue-Key: iam|GetRole|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# IAM GetRole refuses a path-prefixed roleName with the generic constraint-violation template instead of AWS's roleName sentence

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
Cause 1 of 1 — GetRole's 400 refusal for a roleName that violates the modeled pattern carries the emulator's generic constraint-violation sentence instead of IAM's own roleName message.
AWS: Smithy: GetRole.RoleName targets roleNameType with @length min 1 max 64 and @pattern ^[\w+=,.@-]+$ (a path-prefixed value containing '/' therefore fails the pattern); the operation models NoSuchEntityException (404) and ServiceFailureException (500), and this input refusal is an awsQuery ValidationError serialized under $.ErrorResponse.Error. Recorded response: HTTP 400 with $.ErrorResponse.Error.Message = "The specified value for roleName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-" — the ENTIRE sentence is fixed text naming the member ('roleName') and the permitted character set; no part of the request is interpolated, so the rejected value is NOT echoed.
Emulator: HTTP 400 (status correct, same $.ErrorResponse.Error envelope) with Message = "1 validation error detected: Value '<rejectedRoleName>' at 'roleName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$" — the generic cross-service constraint-violation template, which echoes the rejected value and the raw regex.
Why: IAM does not speak the generic validation template for roleName; a client that surfaces the refusal to a user, or matches on IAM's documented sentence to tell 'name is malformed (e.g. a path was passed where a plain name belongs)' from other 400s, gets an unrecognised message and leaks the raw Smithy regex into its own output.
Fix: the GetRole handler's RoleName validation / its awsQuery error serializer in the iam crate — raise the IAM roleName ValidationError with AWS's fixed sentence rather than falling through to the shared pattern-constraint formatter. Distinct from the open issue 20261029T184359Z-iam-policy-documents-are-returned-as-raw-json.md, which owns the GetRole success-body divergence (policy-document encoding and RoleLastUsed) on this same fixture.
Traces: lakeformation-gtr-lf-path-role-principal-forms#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, /__admin/health 200). Cluster verdict=mismatch, service=iam, operation=GetRole, status 400->400, not owned by any open issue and not stub-blocked:

lakeformation-gtr-lf-path-role-principal-forms#task#5 — $.ErrorResponse.Error.Message: value_mismatch; expected 'The specified value for roleName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-', got "1 validation error detected: Value '<path-prefixed role name>' at 'roleName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$".

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or call iam GetRole directly with a RoleName that includes a path prefix (a value containing '/', e.g. 'service-role/<role-name>'), as fixture lakeformation-gtr-lf-path-role-principal-forms does at step task#5.
3. Compare the 400 response's $.ErrorResponse.Error.Message against the recorded AWS text 'The specified value for roleName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-'.
