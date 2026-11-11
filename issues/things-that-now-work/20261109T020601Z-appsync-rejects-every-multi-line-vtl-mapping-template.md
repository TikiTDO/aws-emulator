Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-idp__dynamodb.appsync-apikey-expired-negative appsync__cognito-idp__dynamodb.appsync-cognito-admin-created-user-can-query-positive appsync__cognito-idp__dynamodb.appsync-cognito-groups-claim-authorization-positive appsync__cognito-idp__dynamodb.appsync-cognito-invalid-token-unauthorized-negative appsync__cognito-idp__dynamodb.appsync-cognito-userpool-auth-valid-token-positive appsync__cognito-idp__dynamodb.appsync-cognito-wrong-userpool-token-negative appsync__cognito-idp__dynamodb.appsync-ddb-batch-pipeline-resolver-positive appsync__cognito-idp__dynamodb.appsync-ddb-datasource-missing-role-permission-negative appsync__cognito-idp__dynamodb.appsync-ddb-resolver-context-identity-write-positive appsync__cognito-idp__dynamodb.appsync-ddb-resolver-delete-positive appsync__cognito-idp__dynamodb.appsync-ddb-resolver-put-get-positive appsync__cognito-idp__dynamodb.appsync-ddb-resolver-query-gsi-positive appsync__cognito-idp__dynamodb.appsync-ddb-resolver-update-conditional-positive appsync__cognito-idp__dynamodb.appsync-ddb-wrong-table-arn-negative appsync__cognito-idp__dynamodb.appsync-iam-auth-missing-policy-negative appsync__cognito-idp__dynamodb.appsync-invalid-api-key-rejected appsync__cognito-idp__dynamodb.appsync-mutation-triggers-subscription-positive appsync__cognito-idp__dynamodb.appsync-subscription-cognito-group-filter-positive appsync__cognito-idp__dynamodb.appsync-subscription-filter-mismatch-no-delivery-negative appsync__cognito-idp__dynamodb.appsync-subscription-missing-subscribe-permission-negative appsync__cognito-idp__dynamodb.appsync-subscription-payload-format appsync__cognito-idp__dynamodb.cli-appsync-apikey-query-ddb-positive appsync__cognito-idp__dynamodb.cli-appsync-create-api-cognito-auth-workflow appsync__cognito-idp__dynamodb.cli-appsync-ddb-datasource-create-workflow appsync__cognito-idp__dynamodb.cli-appsync-delete-datasource-negative appsync__cognito-idp__dynamodb.cli-cognito-admin-disable-user-appsync-auth-negative appsync__cognito-idp__dynamodb.cli-cognito-signup-confirm-appsync-query
Issue-Key: appsync|CreateResolver|ValidationException
Issue-Key: appsync__cognito-idp__dynamodb|CreateFunction|ValidationException
Issue-Key: appsync__cognito-idp__dynamodb.cli-appsync-apikey-query-ddb-positive|_run_shell#10|AssertionError|raise AssertionError(
Issue-Key: appsync__cognito-idp__dynamodb.cli-appsync-create-api-cognito-auth-workflow|_run_shell#10|AssertionError|raise AssertionError(
Issue-Key: appsync__cognito-idp__dynamodb.cli-appsync-ddb-datasource-create-workflow|_run_shell#9|AssertionError|raise AssertionError(
Issue-Key: appsync__cognito-idp__dynamodb.cli-appsync-delete-datasource-negative|_run_shell#9|AssertionError|raise AssertionError(
Issue-Key: appsync__cognito-idp__dynamodb.cli-cognito-admin-disable-user-appsync-auth-negative|_run_shell#10|AssertionError|raise AssertionError(
Issue-Key: appsync__cognito-idp__dynamodb.cli-cognito-signup-confirm-appsync-query|_run_shell#10|AssertionError|raise AssertionError(
Issue-Commit: fe8d5c951eea5d6f54d934df17b141049d405276

# AppSync rejects every multi-line VTL mapping template: the MappingTemplate @pattern "^.*$" is applied without multiline semantics, so CreateResolver/CreateFunction answer ValidationException for templates AWS accepts (1 cause)

> _Note: this issue was identified against the code base at commit `fe8d5c951eea5d6f54d934df17b141049d405276`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolver

## Description
Cause 1 of 1: the shared request-validation walk refuses any `MappingTemplate` value that contains a newline, so no AppSync resolver or pipeline function with a real VTL template can be created.

AWS: Smithy model `com.amazonaws.appsync#MappingTemplate` is `string` with `@length(min: 1, max: 65536)` and `@pattern("^.*$")`. It is the target of `CreateResolver.requestMappingTemplate`, `CreateResolver.responseMappingTemplate`, `CreateFunction.requestMappingTemplate` and `CreateFunction.responseMappingTemplate` (same members on UpdateResolver/UpdateFunction). AWS AppSync accepts multi-line Apache Velocity (VTL) templates on these members — multi-line is the normal and documented form (a DynamoDB GetItem request template is a pretty-printed JSON object spanning several lines; a batch template contains `#foreach`/`#if`/`#end` directives on their own lines). The recordings confirm it: every one of the 27 checks below submitted a multi-line template to a real account and `CreateResolver`/`CreateFunction` succeeded with HTTP 200. The `^.*$` pattern is effectively a no-op at AWS for these members: `^` and `$` are evaluated per line, so a value containing `\n` still satisfies it, and AWS applies no newline restriction to mapping templates at all. Neither the model nor the recordings show any AppSync refusal of a newline-containing template; AWS's modeled client-error channel for these two operations is `BadRequestException` (HTTP 400), and `ValidationException` is not among the modeled errors of either operation.

Emulator: the centralized constraint check that runs between deserialize and `handle` treats `^.*$` as a whole-string anchored match with `.` not matching a line terminator, so the first newline in the template fails the constraint and the request is refused before the handler runs. Replay output, `CreateResolver` (check appsync-apikey-expired-negative and 20 siblings):

  HTTP 400, code `ValidationException`, message:
  `1 validation error detected: Value '{\n  "version": "2017-02-28",\n  "operation": "GetItem",\n  "key": { "id": $util.dynamodb.toDynamoDBJson($ctx.args.id) }\n}' at 'requestMappingTemplate' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*$`

and `CreateFunction` (check appsync-ddb-batch-pipeline-resolver-positive) refuses both template members at once:

  `2 validation errors detected: Value '...' at 'requestMappingTemplate' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*$; Value '...' at 'responseMappingTemplate' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*$`

In that message the fixed text is `<n> validation error(s) detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <regex>`; the variable parts are the error count, the submitted template value, the member name (`requestMappingTemplate` / `responseMappingTemplate`) and the regex. The requirement is that this refusal not be produced at all for a newline-containing template — AWS returns 200 and the created Resolver/Function record. Note this is NOT the already-filed anchor-rendering divergence (20261031T170200Z-cognito-idp-pattern-constraint-violation-messages-drop-the.md and the AppSync `ValidationException`-instead-of-`BadRequestException` cause in 20261024T032443Z-appsync-control-plane-fidelity-domain-name-operations-unimplemented.md): those are about how a *legitimate* pattern refusal is worded/classified, whereas here no refusal is legitimate.

This is also distinct from the open stub issue 20261109T003200Z-appsync-createresolver-stub.md. That issue covers `CreateResolver` answering 501 `NotImplemented`; these checks never reach the handler and never see a 501, and the same refusal is produced by `CreateFunction`, which is already implemented (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_function.rs line 4 declares it implemented). Implementing `CreateResolver` therefore does not clear this: the defect is upstream of both handlers.

Why: a client relying on AWS's behavior cannot create any AppSync resolver or pipeline function, because every real VTL mapping template is multi-line. The whole AppSync data plane becomes unreachable behind a 400: GraphQL query/mutation execution, DynamoDB resolver reads and writes, pipeline/batch resolvers, `$ctx.identity` propagation, subscription delivery, and every authorization outcome that depends on a resolver existing (API-key expiry, Cognito user-pool and IAM auth, group-claim field authorization) all fail at setup time instead of exercising the behavior under test. A client that single-lines its template to get past the refusal then changes the template semantics, since VTL directives such as `#if`/`#end` and `#foreach`/`#end` are line-oriented.

Fix: the shared constraint-validation step of the request walk (deserialize → validate → handle) in the vendored smithy schema server, where a modeled `@pattern` is compiled and matched. `^` and `$` in an AWS `@pattern` must be evaluated with multiline semantics (equivalently, the pattern must be matched as a search over the value rather than as a whole-string anchored match), so a value containing newlines is not refused by a pattern whose only anchoring is `^...$`. Fixing it in the validator covers both `CreateResolver` and `CreateFunction` (and the Update* twins) at once; no change in either operation's handler is required for this cause.

Traces: the 27 failing checks listed in trace_ids, all under the suite heading `appsync__cognito-idp__dynamodb`. 21 fail on `CreateResolver`, 1 (`appsync-ddb-batch-pipeline-resolver-positive`) on `CreateFunction`, and 6 are AWS-CLI-driven checks whose `aws appsync create-resolver` exits rc=254 with the same `ValidationException`.

## Evidence
Expected (AWS, as recorded): CreateResolver / CreateFunction with a multi-line VTL template on requestMappingTemplate and responseMappingTemplate answer HTTP 200 and return the Resolver / FunctionConfiguration record, with the template echoed byte-for-byte including its newlines. MappingTemplate is constrained only by @length(1..65536); @pattern("^.*$") admits newline-containing values.

Actual (emulator): HTTP 400 ValidationException before the handler runs.
  CreateResolver: "1 validation error detected: Value '{\n  \"version\": \"2017-02-28\",\n  \"operation\": \"GetItem\",\n  \"key\": { \"id\": $util.dynamodb.toDynamoDBJson($ctx.args.id) }\n}' at 'requestMappingTemplate' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*$"
  CreateFunction: "2 validation errors detected: Value '...' at 'requestMappingTemplate' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*$; Value '...' at 'responseMappingTemplate' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*$"
  AWS CLI form (6 checks): aws appsync create-resolver exits rc=254 with "aws: [ERROR]: An error occurred (ValidationException) when calling the CreateResolver operation: 1 validation error detected: ... Member must satisfy regular expression pattern: ^.*$"

A single-line template is accepted, which pins the cause to newline handling in the @pattern match rather than to the template content. CreateFunction is an implemented operation (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_function.rs:4) and is refused identically, so the refusal is not the CreateResolver scaffold stub (services/.../src/ops/create_resolver.rs:202 returns 501 NotImplemented, which these checks never reach). The model text is quoted in services/.../src/ops/create_resolver.rs:82-89 ("requestMappingTemplate (MappingTemplate, len 1..65536, pattern ^.*$)").

## Steps to Reproduce
1. Boot the emulator and point an appsync client at it.
2. CreateGraphqlApi, then CreateDataSource of type AMAZON_DYNAMODB for a DynamoDB table, then StartSchemaCreation with an SDL exposing a Query field.
3. Call CreateResolver with the api id, that type/field, dataSourceName, and a multi-line requestMappingTemplate, e.g. the three-plus-line JSON
   {\n  "version": "2017-02-28",\n  "operation": "GetItem",\n  "key": { "id": $util.dynamodb.toDynamoDBJson($ctx.args.id) }\n}
   with responseMappingTemplate "$util.toJson($ctx.result)".
   Observed: HTTP 400 ValidationException "... at 'requestMappingTemplate' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*$". Expected: HTTP 200 with the resolver record.
4. Repeat with the same template collapsed onto one line: the call is accepted, confirming only the newlines are refused.
5. Call CreateFunction on the same API with multi-line request AND response mapping templates: HTTP 400 with "2 validation errors detected", one clause per template member.
6. Equivalent CLI form: aws appsync create-resolver --request-mapping-template "$(printf '{\n  "version": "2017-02-28"\n}')" ... exits rc=254 with the same ValidationException.
