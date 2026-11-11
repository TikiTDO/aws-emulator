Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow
Issue-Key: lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# The in-process Lambda runtime's bundled SDK has no s3 client, so a bucket-policy Deny vs identity Allow cannot be exercised

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issue 20261106T151148Z-lambda-s3-in-function-s3-calls-raise-unknownserviceerror.md owns another check of this service and does not carry this replay key.

Cause 1 of 1: function code cannot construct an S3 client, so neither prefix is ever read.
AWS: the managed python runtime's boto3 builds a client for any service. With an identity policy allowing s3:GetObject on both prefixes, a handler reads both (200 with the bodies); adding a bucket-policy Deny for one prefix then refuses that one 403 AccessDenied with "... with an explicit deny in a resource-based policy" while the other keeps succeeding.
Emulator: the handler faults before the call. The check polls 120 s for the baseline and reports "both prefixes readable via the identity policy before the Deny not met within 120.0s. Last result: {'denied': {'code': 'UnknownServiceError', 'message': None}, 'allowed': {'code': 'UnknownServiceError', 'message': None}}" — the bundled SDK's client factory refuses 's3' (its registry lists only sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager). Note the payload also carries message: None, so even the fault gives the caller no message to act on.
Why: the baseline both-readable control cannot pass, so the explicit-Deny precedence it gates is unfalsifiable; and because the same UnknownServiceError is returned whether a call would have been allowed or denied, no in-function authorization behaviour can be observed.
Fix: the in-process Lambda runtime's bundled AWS SDK shim — its service registry and the client factory that refuses unknown services.
Traces: lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow.

## Evidence
Expected both prefixes readable, then the Deny'd prefix refused 403 AccessDenied ('with an explicit deny in a resource-based policy'). Actual: {'denied': {'code': 'UnknownServiceError', 'message': None}, 'allowed': {'code': 'UnknownServiceError', 'message': None}} for 120s.

## Steps to Reproduce
1) Grant the function role s3:GetObject on two prefixes and write an object under each.
2) Invoke a handler that calls boto3.client('s3').get_object on both.
3) Observe UnknownServiceError for both instead of the two bodies.
