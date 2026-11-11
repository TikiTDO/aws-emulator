Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds.deploy-cfn-rds-db-parameter-group-invalid-family-rejected-and-rolled-back cloudformation__rds.deploy-rds-db-cluster-parameter-group-invalid-family-rejected
Issue-Key: cloudformation__rds.deploy-cfn-rds-db-parameter-group-invalid-family-rejected-and-rolled-back|run#8|AssertionError|assert got == _REJECTED, f"the ParamGroup CREATE_FAILED reason was {got!r}, expected {_REJECTED!r}"
Issue-Key: cloudformation__rds.deploy-rds-db-cluster-parameter-group-invalid-family-rejected|run#15|AssertionError|assert got == want, f"handler reason {got!r}, expected {want!r}"
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# cloudformation: a failed RDS resource's CREATE_FAILED reason drops the SDK/service suffix AWS appends to the handler message

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
1 net-new root cause in how CloudFormation composes the ResourceStatusReason of a resource whose provisioning was refused by its downstream AWS service.

Cause 1 of 1: the resource-handler failure reason carries only the bare downstream message, with no service/status/request-id/attempt-count suffix.
AWS: when a CloudFormation resource handler's downstream AWS call is refused, the CREATE_FAILED stack event's ResourceStatusReason is the handler envelope wrapping the downstream SDK exception's full message. Its fixed text is `Resource handler returned message: "` ... `" (RequestToken: ` ... `, HandlerErrorCode: ` ... `)`, and the quoted message itself is the downstream service message followed by the SDK's own suffix, whose fixed text is ` (Service: `, `, Status Code: `, `, Request ID: `, `)` and then ` (SDK Attempt Count: `, `)`. The variable parts all come from the request and the downstream response: the downstream service's message (for an invalid DB parameter-group family, `ParameterGroupFamily <family> is not a valid parameter group family`, with <family> taken verbatim from the template's DBParameterGroup/DBClusterParameterGroup Family property), the service name (`Rds`), the HTTP status code (`400`), the request id of the refused call, the SDK attempt count, the stack operation's request token, and the handler error code (`InvalidRequest`). AWS's own Smithy model does not describe stack-event text, so the envelope and suffix are recorded behavior only; the refusal itself is the modeled RDS client error (HTTP 400, InvalidParameterValue class, handler-mapped to HandlerErrorCode InvalidRequest).
Emulator: the event's ResourceStatusReason stops at the downstream message — it reads `ParameterGroupFamily <family> is not a valid parameter group family` with HandlerErrorCode `InvalidRequest`, and the ` (Service: Rds, Status Code: 400, Request ID: <id>)` and ` (SDK Attempt Count: <n>)` segments are absent entirely. Both failing checks normalise the request id and attempt count before comparing, so the divergence is the missing fixed segments, not per-run values.
Why: a client that reads a rollback reason to recover WHICH service refused the create and with WHAT HTTP status — CDK/CFN tooling, rollback triage, and any automation that regex-matches the `(Service: …, Status Code: …)` suffix — finds neither, so it cannot attribute or classify the failure and sees only an unqualified sentence.
Fix: the stack-event ResourceStatusReason composer on the cloudformation resource-provider error path, where a downstream service refusal is turned into a handler message.
Traces: cloudformation__rds.deploy-cfn-rds-db-parameter-group-invalid-family-rejected-and-rolled-back, cloudformation__rds.deploy-rds-db-cluster-parameter-group-invalid-family-rejected.

## Evidence
Expected (AWS), per check: ('ParameterGroupFamily sim2real-not-a-real-family is not a valid parameter group family (Service: Rds, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest') — the message plus the SDK suffix, with id/count normalised.
Actual (emulator): ('ParameterGroupFamily sim2real-not-a-real-family is not a valid parameter group family', 'InvalidRequest').

Identically for the cluster parameter group:
Expected: ('ParameterGroupFamily sim2real-bogus-family-9000 is not a valid parameter group family (Service: Rds, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest')
Actual:   ('ParameterGroupFamily sim2real-bogus-family-9000 is not a valid parameter group family', 'InvalidRequest')

The family names above are the request's own values filling the template; the requirement is the fixed suffix text and the Service/Status Code values (`Rds`, `400`), not those names.

## Steps to Reproduce
1. CreateStack with an AWS::RDS::DBParameterGroup whose Family is a nonexistent parameter-group family (e.g. a `sim2real-not-a-real-family` string).
2. Poll DescribeStacks until the stack reaches a terminal status (ROLLBACK_COMPLETE).
3. DescribeStackEvents and read the ResourceStatusReason of the parameter group's CREATE_FAILED event; parse it with the handler envelope regex `^Resource handler returned message: "(?P<message>.*)" \(RequestToken: [0-9a-f-]+, HandlerErrorCode: (?P<code>[A-Za-z]+)\)$`.
4. Observe the extracted message lacks the ` (Service: Rds, Status Code: 400, Request ID: …) (SDK Attempt Count: …)` suffix.
5. Repeat with an AWS::RDS::DBClusterParameterGroup and a bogus Family for the same result.
