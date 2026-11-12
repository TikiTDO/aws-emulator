Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-eval-code-error-detail
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-eval-code-error-detail appsync-cfg-resolver-batch-and-caching-rules appsync-cfg-resolver-sync-config-rules
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateFunction|4xx->4xx|$.detail.codeErrors[*].value
Issue-Key: appsync|CreateResolver|4xx->4xx|$.detail.codeErrors[*].value
Issue-Key: appsync|CreateFunction|4xx->2xx|$.detail
Issue-Key: appsync|CreateFunction|4xx->2xx|$.message
Issue-Key: appsync|CreateFunction|4xx->2xx|$.reason
Issue-Key: appsync|CreateFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateFunction|4xx->2xx|status
Issue-Key: appsync|ListFunctions|2xx->2xx|$.functions[name="appsync_eval_fn_bad_ec112d98"]
Issue-Key: appsync|CreateResolver|2xx->4xx|$.resolver
Issue-Key: appsync|CreateResolver|2xx->4xx|status
Issue-Commit: 9a53e127c8f44bbc45a6320e6c70fd06205845c1

# AppSync APPSYNC_JS code diagnostics and cachingKeys validation diverge on CreateFunction/CreateResolver (5 causes)

> _Note: this issue was identified against the code base at commit `9a53e127c8f44bbc45a6320e6c70fd06205845c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
One shared fault behind five net-new causes: the emulator's validation of APPSYNC_JS code and of cachingConfig.cachingKeys on the resolver/function write path is its own, not AWS's. Its code checker is a keyword scanner rather than a compiler+linter (so its PARSER diagnostics carry emulator-authored text and its linter rules are missing entirely), and its cachingKeys prefix check now refuses keys AWS accepts. All five causes are service=appsync, none is stub-blocked and none is owned by an open issue; all five must be closed before this issue is submitted.

Related open issues that do NOT cover these: 20261110T162826Z-appsync-createresolver-accepts-resolvers-aws-refuses-cachingkeys-prefix.md describes the OPPOSITE state of the same code path (CreateResolver answering 200 where AWS refuses, and no cachingKeys check at all); the emulator now refuses, so the surviving divergences below (wrong diagnostic text, missing lint rules, and a valid resolver wrongly refused) are new clusters with new replay keys. 20261111T092526Z-appsync-evaluatecode-diverges-across-the-board-context-not.md covers EvaluateCode, a different operation.

Cause 1 of 5 - CreateFunction's PARSER diagnostic text is emulator-authored (fixture appsync-eval-code-error-detail, step task#4).
AWS: Smithy: CreateFunction (POST /v1/apis/{apiId}/functions) models BadRequestException (httpError 400) which carries message, reason (BadRequestReason, only value CODE_ERROR) and detail (BadRequestDetail.codeErrors: list of CodeError {errorType, value, location{line,column,span}}); CodeError$value is documented as "A user presentable error" with examples like "Parsing error: Unterminated string literal" and CodeError$errorType as the diagnostic class ("LINT_ERROR", "PARSER_ERROR"). Recorded behavior: AWS compiles the submitted code with its TypeScript-based compiler and reports each diagnostic in the compiler's own form - fixed shape "code.js(<line>,<column>): error <TSxxxx>: <compiler message>\n", where the line/column come from the submitted code and the TS code and message come from the compiler. For the recorded body (an unterminated object literal) that is "code.js(7,1): error TS1005: '}' expected.\n".
Emulator: answers 400 with the right status, reason and location, but detail.codeErrors[0].value is its own scanner's text - expected "code.js(7,1): error TS1005: '}' expected.\n", got "code.js(7,1): error: unsupported keyword: export\n". The fixed part that diverges is the diagnostic itself: AWS names a TypeScript error code and the real syntax problem; the emulator names no TS code and blames the `export` keyword, which is legal APPSYNC_JS. The line/column (7,1) are values derived from the submitted code.
Why: a client or console that surfaces detail.codeErrors[*].value to the developer is told the wrong thing about its code - that a required `export` statement is unsupported rather than that a brace is missing - so code that AWS would accept once the brace is added looks unfixable, and any tooling matching on the TSxxxx code finds none.
Fix: CreateFunction's code validation (shared with CreateResolver) - run the APPSYNC_JS compiler/parser and emit each diagnostic in AWS's "code.js(<line>,<column>): error <TScode>: <message>" form instead of synthesising a keyword-scanner message.
Traces: appsync-eval-code-error-detail#task#4.

Cause 2 of 5 - the same emulator-authored PARSER text on CreateResolver (fixture appsync-eval-code-error-detail, step task#6).
AWS: as Cause 1; Smithy: CreateResolver (POST /v1/apis/{apiId}/types/{typeName}/resolvers) models the same BadRequestException with reason/detail, and Resolver$code (Code, @length 1..32768) with runtime AppSyncRuntime {name APPSYNC_JS, runtimeVersion 1.0.0}.
Emulator: 400 with detail.codeErrors[0].value "code.js(7,1): error: unsupported keyword: export\n" instead of "code.js(7,1): error TS1005: '}' expected.\n". This is a distinct cluster and replay key from Cause 1 (operation CreateResolver), so replaying Cause 1's step does not exercise it.
Why: same as Cause 1 on the resolver write path - the compile-failure contract a client codes against reports a different cause of failure than AWS.
Fix: the same shared code-diagnostics component, reached from CreateResolver's validation.
Traces: appsync-eval-code-error-detail#task#6.

Cause 3 of 5 - CreateFunction accepts code AWS refuses with LINT_ERROR diagnostics (fixture appsync-eval-code-error-detail, step task#5).
AWS: Smithy: CodeError$errorType examples include LINT_ERROR; BadRequestReason = CODE_ERROR; CreateFunction models BadRequestException 400. Recorded behavior: AWS does not only parse the code, it lints it against the APPSYNC_JS rule set and refuses code that parses but breaks a rule, with status 400, reason "CODE_ERROR", message "The code contains one or more errors." (entirely fixed text) and detail.codeErrors listing one CodeError per violated rule, errorType "LINT_ERROR" and value of the fixed form "code.js(<line>,<column>): error <rule-name>: <rule message>" plus location {line, column, span}. The recorded body (a `for` loop using `++`) produces the rules @aws-appsync/no-for ("For statements are not supported.") and @aws-appsync/no-disallowed-unary-operators ("The operator ++ is not supported"); the line/column/span are values from the submitted code, the rule names and their messages are the fixed text.
Emulator: answers 200 and creates the function - status 200 instead of 400, and $.reason, $.message, $.detail and the x-amzn-errortype header are all absent. Its code check only looks for unsupported keywords (see Cause 1), so no lint rule is evaluated.
Why: a client that relies on AWS rejecting unsupported JavaScript constructs at deploy time ships a function the emulator reports as created but that AWS would never accept; the whole reason+detail.codeErrors rendering path for lint diagnostics is unreachable, so an SDK/console error surface cannot be exercised at all.
Fix: CreateFunction's code validation (shared with CreateResolver) - add the APPSYNC_JS lint pass and raise BadRequestException with reason CODE_ERROR, message "The code contains one or more errors." and one LINT_ERROR CodeError per violated rule, with its line/column/span, instead of accepting the code.
Traces: appsync-eval-code-error-detail#task#5.

Cause 4 of 5 - ListFunctions then serves the function that was never created on AWS (fixture appsync-eval-code-error-detail, step task#8).
AWS: Smithy: ListFunctions output functions: FunctionConfiguration list. Because the create in Cause 3 is refused, no function record exists, so the listing for that API contains no entry with that name.
Emulator: ListFunctions answers 200 with an extra element - expected no entry for the lint-failing function name, got a full FunctionConfiguration (functionId/functionArn/name/dataSourceName/maxBatchSize/runtime/code) carrying the rejected code body. The names, ids and ARNs in the recording are per-run values; the requirement is that no entry exists for a function whose create AWS refused.
Why: the state divergence outlives the response divergence - a client that inventories functions (drift detection, teardown, codegen) sees a function AWS does not have, so the divergence is observable even for clients that never inspect the create response.
Fix: once Cause 3 refuses the create, the state write must not happen - the function store (and ListFunctions' projection of it) must contain no record for a refused CreateFunction.
Traces: appsync-eval-code-error-detail#task#8.

Cause 5 of 5 - CreateResolver refuses a resolver with VALID cachingKeys (fixture appsync-cfg-resolver-batch-and-caching-rules, step task#11).
AWS: Smithy: CachingConfig.cachingKeys (CachingKeys) - "The caching keys for a resolver that has caching activated. Valid values are entries from the $context.arguments, $context.source, and $context.identity maps"; CachingConfig.ttl is "Valid values are 1-3,600 seconds"; CreateResolver returns 200 with resolver: Resolver. Keys rooted at $context.arguments and $context.identity are therefore accepted and the resolver is created, with cachingConfig echoed back as submitted.
Emulator: answers 400 and no resolver - expected 200 with the Resolver body (kind UNIT, maxBatchSize 0, cachingConfig {ttl: 60, cachingKeys: ["$context.arguments.<arg>", "$context.identity.<field>"]}), got status 400 and $.resolver absent. Hypothesis: the cachingKeys prefix check added for the opposite divergence (see the related issue above) does not accept the full set of roots AWS allows - at minimum $context.arguments and/or $context.identity - so a well-formed caching configuration is rejected.
Why: this is a false refusal on the create path: a client configuring per-argument or per-identity resolver caching exactly as AWS documents cannot create the resolver at all, and the failure is a 400 that AWS never returns, so the client has no way to proceed.
Fix: CreateResolver's cachingConfig validation (shared with UpdateResolver) - accept any cachingKeys entry rooted at $context.args, $context.arguments, $context.identity or $context.source and create the resolver, refusing only entries outside that set.
Traces: appsync-cfg-resolver-batch-and-caching-rules#task#11 (same cluster: appsync-cfg-resolver-sync-config-rules#task#9).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, data listener on :40431 accepting). 733 steps: 522 passed, 201 mismatched, 10 blocked, score 71.2%, 64 divergence clusters; all clusters are service=appsync, 56 are owned by open issues and none is stub-blocked, leaving 8 net-new clusters - the 5 below plus the 3 ListTypes clusters filed separately.

1) appsync-eval-code-error-detail#task#4 - CreateFunction, expected 400, actual 400. divergence_key service=appsync|CreateFunction|status=400->400|keys=$.detail.codeErrors[0].value. detail: $.detail.codeErrors[0].value: value_mismatch (expected "code.js(7,1): error TS1005: '}' expected.\n", got 'code.js(7,1): error: unsupported keyword: export\n'). affected_count 1. owned_by [].

2) appsync-eval-code-error-detail#task#6 - CreateResolver, expected 400, actual 400. divergence_key service=appsync|CreateResolver|status=400->400|keys=$.detail.codeErrors[0].value. Same expected/actual value_mismatch as (1). affected_count 1. owned_by [].

3) appsync-eval-code-error-detail#task#5 - CreateFunction, expected 400, actual 200. divergence_key service=appsync|CreateFunction|status=400->200|keys=$.detail,$.message,$.reason,$headers.x-amzn-errortype,status. detail: status expected 400 got 200; $.reason missing (expected 'CODE_ERROR'); $.message missing (expected 'The code contains one or more errors.'); $.detail missing (expected {'codeErrors': [{'errorType': 'LINT_ERROR', 'value': 'code.js(2,3): error @aws-appsync/no-for: For statements are not supported.', 'location': {'line': 2, 'column': 3, 'span': 30}}, {'errorType': 'LINT_ERROR', 'value': 'code.js(2,26): error @aws-appsync/no-disallowed-unary-operators: The operator ++ is not supported', 'location': {'line': 2, 'column': 26, 'span': 3}}]}); $headers.x-amzn-errortype missing. affected_count 1. owned_by [].

4) appsync-eval-code-error-detail#task#8 - ListFunctions, 200 vs 200. divergence_key service=appsync|ListFunctions|status=200->200|keys=$.functions[name="appsync_eval_fn_bad_<id>"]. detail: $.functions[name="appsync_eval_fn_bad_ec112d98"]: extra (expected None, got a FunctionConfiguration with runtime {'name': 'APPSYNC_JS', 'runtimeVersion': '1.0.0'} and the for/++ code body accepted at task#5). The function name, functionId and ARN are per-run values. affected_count 1. owned_by [].

5) appsync-cfg-resolver-batch-and-caching-rules#task#11 - CreateResolver, expected 200, actual 400. divergence_key service=appsync|CreateResolver|status=200->400|keys=$.resolver,status. detail: status expected 200 got 400; $.resolver missing (expected {'typeName': 'Query', 'fieldName': 'f1', 'dataSourceName': '<ddb ds>', 'resolverArn': 'arn:aws:appsync:us-west-2:000000000000:apis/<apiId>/types/Query/resolvers/f1', 'requestMappingTemplate': '{"version":"2017-02-28","operation":"GetItem","key":{}}', 'responseMappingTemplate': '$util.toJson($ctx.result)', 'kind': 'UNIT', 'pipelineConfig': None, 'syncConfig': None, 'cachingConfig': {'ttl': 60, 'cachingKeys': ['$context.arguments.id', '$context.identity.sub']}, 'maxBatchSize': 0, 'runtime': None, 'code': None, 'metricsConfig': None}, got None). affected_count 2: also appsync-cfg-resolver-sync-config-rules#task#9. owned_by [].

Model evidence (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json): CreateFunction POST /v1/apis/{apiId}/functions, CreateResolver POST /v1/apis/{apiId}/types/{typeName}/resolvers, both erroring BadRequestException(400)/NotFoundException(404)/ConcurrentModificationException(409)/UnauthorizedException(401)/InternalFailureException(500). BadRequestException = {message, reason: BadRequestReason, detail: BadRequestDetail}; BadRequestReason enum values = [CODE_ERROR]; BadRequestDetail = {codeErrors: CodeErrors}; CodeError = {errorType ("Examples include, but aren't limited to: LINT_ERROR, PARSER_ERROR"), value ("A user presentable error"), location: CodeErrorLocation{line, column, span}}. CachingConfig = {ttl: Long required, "Valid values are 1-3,600 seconds"; cachingKeys: CachingKeys, "Valid values are entries from the $context.arguments, $context.source, and $context.identity maps"}. Code = String @length 1..32768; AppSyncRuntime = {name: APPSYNC_JS required, runtimeVersion: '1.0.0' required}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay appsync-eval-code-error-detail step task#4. CreateGraphqlApi, a NONE data source, then CreateFunction with runtime {APPSYNC_JS, 1.0.0} and a code body whose object literal is never closed. Expect 400 reason CODE_ERROR with detail.codeErrors[0].value "code.js(<line>,<col>): error TS1005: '}' expected.\n"; the emulator reports 'code.js(7,1): error: unsupported keyword: export\n'.
3. Cause 2: replay appsync-eval-code-error-detail step task#6 - the same body through CreateResolver; the same wrong diagnostic text comes back.
4. Cause 3: replay appsync-eval-code-error-detail step task#5. CreateFunction with code containing `for (let i = 0; i < 3; i++) {}`. Expect 400, reason CODE_ERROR, message 'The code contains one or more errors.' and two LINT_ERROR codeErrors (@aws-appsync/no-for and @aws-appsync/no-disallowed-unary-operators) with their line/column/span; the emulator answers 200 and creates the function.
5. Cause 4: replay appsync-eval-code-error-detail step task#8 - ListFunctions for that API. Expect no entry for the lint-failing function; the emulator lists it with the rejected code body.
6. Cause 5: replay appsync-cfg-resolver-batch-and-caching-rules step task#11 (cluster also covers appsync-cfg-resolver-sync-config-rules#task#9). CreateResolver on Query/f1 against a DynamoDB data source with cachingConfig {ttl: 60, cachingKeys: ['$context.arguments.id', '$context.identity.sub']}. Expect 200 with the Resolver echoing that cachingConfig; the emulator answers 400 with no resolver.
7. All five steps must pass before this issue is closed.
