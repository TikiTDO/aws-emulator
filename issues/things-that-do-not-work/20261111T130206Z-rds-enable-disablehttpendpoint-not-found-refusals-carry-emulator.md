Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-cluster-http-endpoint
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-cluster-http-endpoint
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|EnableHttpEndpoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DisableHttpEndpoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 23e4ac40a56978a5a155c920fdf4b093f96724d4

# rds: Enable/DisableHttpEndpoint not-found refusals carry emulator-authored message text

> _Note: this issue was identified against the code base at commit `23e4ac40a56978a5a155c920fdf4b093f96724d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableHttpEndpoint

## Description
Two net-new wire causes on the rds Data-API-enablement pair: when the ResourceArn names a DB cluster that does not exist, the HTTP status (404) and the awsQuery error class match AWS, but the error MESSAGE is the emulator's own internal wording instead of the sentence RDS emits. Each is a separate replay cluster and must be closed on its own.

Cause 1 of 2: EnableHttpEndpoint's not-found refusal is worded by the emulator.
AWS: EnableHttpEndpoint takes a single required ResourceArn and models ResourceNotFoundFault (Smithy: EnableHttpEndpoint.ResourceArn -> String, @required; errors InvalidResourceStateFault (400) and ResourceNotFoundFault, awsQueryError code 'ResourceNotFoundFault', httpResponseCode 404; ResourceNotFoundFault documentation: 'The specified resource ID was not found.'). The model does not state the message text, so recorded behavior supplies it: HTTP 404 with the awsQuery envelope ErrorResponse/Error whose Message is the fixed text 'The DB cluster ' + <the ResourceArn exactly as sent by the request> + " wasn't found." The only variable part of the sentence is that request-supplied ARN; the leading 'The DB cluster ' and the trailing " wasn't found." are fixed, and the RequestId in the recording is a per-run value, not a requirement.
Emulator: the status 404 and the error code match, but the Message is the emulator's internal formatting 'DBCluster not found: ' + <the ARN> — a colon-delimited debug form that uses the emulator's internal type name 'DBCluster', has no sentence structure, no leading article and no trailing " wasn't found."
Why: a client that keys its retry/branching on the refusal text (and every log scraper, assertion or operator runbook written against AWS's "The DB cluster ... wasn't found.") never matches, so a plain missing-cluster condition is reported as an unrecognised failure instead of a known one. The surrounding status/code match makes this silent: the client sees the right class of error with text no AWS SDK integration test accepts.
Fix: the EnableHttpEndpoint handler's not-found path (the error construction that resolves ResourceArn to a cluster in state) — raise ResourceNotFoundFault with AWS's fixed sentence and the request's ResourceArn interpolated, rather than the generic '<Type> not found: <id>' formatter shared by the emulator's state lookups.
Traces: rds-db-cluster-http-endpoint#task#1.

Cause 2 of 2: DisableHttpEndpoint's not-found refusal is worded the same emulator way.
AWS: DisableHttpEndpoint has the identical surface (Smithy: DisableHttpEndpoint.ResourceArn -> String, @required; errors InvalidResourceStateFault (400) and ResourceNotFoundFault, awsQueryError code 'ResourceNotFoundFault', httpResponseCode 404). Recorded behavior: HTTP 404 with ErrorResponse/Error/Message = 'The DB cluster ' + <ResourceArn from the request> + " wasn't found." — same fixed text, same single request-derived variable.
Emulator: status and code match; the Message is again 'DBCluster not found: ' + <the ARN>.
Why: same client impact as cause 1, on the disable half of the pair — a teardown or idempotent-disable path that distinguishes 'already gone' from a real fault by message text mis-classifies the response.
Fix: the DisableHttpEndpoint handler's not-found path, the same error construction as cause 1; both operations must emit RDS's wording.
Traces: rds-db-cluster-http-endpoint#task#2.

These two clusters are reported unowned by the replay: the open issues '20261111T091517Z-rds-control-plane-refusals-are-missing-or-carry.md' and '20261109T205028Z-rds-invalid-input-and-not-found-refusals-are.md' cover different operations (ModifyDBClusterParameterGroup, CreateGlobalCluster, CreateDBCluster and a different set of four requests) and do not carry these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 52.2%, 95 divergence clusters; these two are the only clusters not already owned by an open issue or blocked by an unimplemented-operation stub).

Cause 1 - rds-db-cluster-http-endpoint#task#1, EnableHttpEndpoint, status expected 404, got 404:
  $.ErrorResponse.Error.Message: value_mismatch
    expected: "The DB cluster <ResourceArn sent by the request> wasn't found."
    got:      'DBCluster not found: <ResourceArn sent by the request>'
  (In the recording the ARN is the fixture's nonexistent cluster ARN; fixed text = 'The DB cluster ' ... " wasn't found.", variable part = the request's ResourceArn.)

Cause 2 - rds-db-cluster-http-endpoint#task#2, DisableHttpEndpoint, status expected 404, got 404:
  $.ErrorResponse.Error.Message: value_mismatch
    expected: "The DB cluster <ResourceArn sent by the request> wasn't found."
    got:      'DBCluster not found: <ResourceArn sent by the request>'

In both clusters only the Message member diverges: the 404 status and the ResourceNotFoundFault code/Type are already correct, so the defect is strictly the message wording.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture rds-db-cluster-http-endpoint step task#1 - EnableHttpEndpoint with a ResourceArn naming a DB cluster that does not exist answers 404 ResourceNotFoundFault with Message 'DBCluster not found: <arn>' instead of AWS's "The DB cluster <arn> wasn't found."
4. Cause 2: replay fixture rds-db-cluster-http-endpoint step task#2 - DisableHttpEndpoint on the same nonexistent ResourceArn answers the same emulator-worded message instead of AWS's sentence.
5. Compare $.ErrorResponse.Error.Message in each response against the recorded expectation; status and error code already match.
