Issue-Source: trace-sweep-divergence
Issue-Trace: logs-put-syslog-configuration-invalid-vpc-endpoint-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-put-syslog-configuration-invalid-vpc-endpoint-error logs-syslog-config-invalid-vpc-endpoint-error logs-syslog-configuration-error-contracts logs-syslog-configuration-invalid-vpc-endpoint-id logs-syslog-configuration-validation-errors logs-syslog-config-delete-without-existing-config logs-syslog-config-missing-vpc-endpoint-error
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|PutSyslogConfiguration|4xx->2xx|$
Issue-Key: logs|PutSyslogConfiguration|4xx->2xx|status
Issue-Key: logs|ListSyslogConfigurations|2xx->2xx|$.syslogConfigurations[*]
Issue-Key: logs|DeleteSyslogConfiguration|4xx->2xx|$
Issue-Key: logs|DeleteSyslogConfiguration|4xx->2xx|status
Issue-Key: logs|DeleteSyslogConfiguration|4xx->4xx|$.message
Issue-Key: logs|PutSyslogConfiguration|4xx->4xx|$.__type
Issue-Key: logs|PutSyslogConfiguration|4xx->4xx|$.message
Issue-Key: logs|DeleteSyslogConfiguration|4xx->4xx|$.__type
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs syslog configuration operations skip VPC-endpoint validation and misclassify errors (6 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSyslogConfiguration

## Description
Six root causes on the syslog-configuration surface. The shared fault is that the vpcEndpointId is never validated or matched against the log group: calls AWS refuses are accepted and persisted, the read path then lists a configuration AWS does not have, and the refusals that do occur name the wrong error class or the wrong text. Cause count: 6 — all six must be closed. The open issue 20261027T074309Z-logs-syslog-configurations-are-ungradeable-ec2-describevpcendpointservices-omits.md is about EC2 not publishing the syslog PrivateLink service (a different root cause, different replay keys); the wire contracts below are independent of it.

=== Cause 1 of 6: PutSyslogConfiguration accepts a vpcEndpointId that does not resolve ===
AWS: refuses with HTTP 400 InvalidParameterException whose message is 'VPC endpoint <vpcEndpointId> failed validation. Verify the VPC endpoint ID is correct, belongs to your account, and you have the required EC2 permissions.' — fixed text around one variable part, the vpcEndpointId from the request. Smithy: PutSyslogConfiguration.vpcEndpointId @pattern '^vpce-[0-9a-f]{1,64}$' (so a syntactically valid id still has to exist) and the op binds InvalidParameterException (httpError 400).
Emulator: answers HTTP 200 with an empty body and stores the configuration.
Why: a client believes syslog ingestion is wired to a PrivateLink endpoint that does not exist; nothing will ever be ingested and no error is ever surfaced. Five fixtures hit this one check.
Fix: the vpcEndpointId resolution check in the PutSyslogConfiguration handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-put-syslog-configuration-invalid-vpc-endpoint-error#task#2.

=== Cause 2 of 6: ListSyslogConfigurations lists the configuration the refused put should never have stored ===
AWS: after that refusal the log group has NO syslog configuration, so ListSyslogConfigurations returns an empty list.
Emulator: returns an entry {logGroupArn, sourceType 'VPCE', vpcEndpointId, createdAt}.
Why: the read path confirms an ingestion path that does not exist on AWS — this is the step a client uses to verify its own configuration.
Fix: the same validation branch as cause 1 (nothing must be persisted); the ListSyslogConfigurations read path then agrees.
Traces: logs-put-syslog-configuration-invalid-vpc-endpoint-error#task#3.

=== Cause 3 of 6: DeleteSyslogConfiguration for an endpoint not associated with the group answers 200 ===
AWS: HTTP 400 InvalidParameterException with 'VPC endpoint <vpcEndpointId> is not associated with log group <logGroupName>' — fixed text plus two variable parts taken from the request.
Emulator: HTTP 200 with an empty body.
Why: delete is silently idempotent in the emulator but not on AWS, so a teardown that asserts the refusal never fires and a typo'd endpoint id looks like a successful detach.
Fix: the association check in the DeleteSyslogConfiguration handler.
Traces: logs-syslog-config-invalid-vpc-endpoint-error#task#3 (also logs-syslog-configuration-validation-errors#task#4).

=== Cause 4 of 6: DeleteSyslogConfiguration's not-associated message is emulator-authored ===
AWS: 'VPC endpoint <vpcEndpointId> is not associated with log group <logGroupName>' — the same fixed text as cause 3.
Emulator: 'No syslog configuration for vpcEndpointId [<vpcEndpointId>] exists on this log group.' — different wording, and the log group is named only as 'this log group'.
Why: the status and code are right but a client matching AWS's sentence does not recognise the refusal and cannot extract the group name from it.
Fix: the message in that same branch.
Traces: logs-syslog-config-delete-without-existing-config#task#3.

=== Cause 5 of 6: PutSyslogConfiguration validates the log group before the required vpcEndpointId ===
AWS: when vpcEndpointId is absent the refusal is HTTP 400 InvalidParameterException 'vpcEndpointId is required' — AWS checks the parameter before resolving the logGroupIdentifier, so this is the answer even when the log group also does not exist.
Emulator: HTTP 400 ResourceNotFoundException 'The specified log group does not exist.' — wrong error code and wrong message.
Why: a caller that omitted a required parameter is told its log group is missing, so it creates a log group and retries with the same incomplete request forever.
Fix: the parameter-presence check must precede log-group resolution in the PutSyslogConfiguration handler.
Traces: logs-syslog-config-missing-vpc-endpoint-error#task#1.

=== Cause 6 of 6: DeleteSyslogConfiguration has the same inverted validation order ===
AWS: with vpcEndpointId absent the refusal is HTTP 400 InvalidParameterException 'vpcEndpointId is required'.
Emulator: HTTP 400 ResourceNotFoundException 'The specified log group does not exist.'
Why: same as cause 5 on the delete path.
Fix: the same ordering fix in the DeleteSyslogConfiguration handler.
Traces: logs-syslog-config-missing-vpc-endpoint-error#task#2.

## Evidence
Cause 1: "status: expected 400, got 200; $: empty_body (expected {'__type': 'InvalidParameterException', 'message': 'VPC endpoint vpce-0000000000000000 failed validation. Verify the VPC endpoint ID is correct, belongs to your account, and you have the required EC2 permissions.'}, got {})" — 5 affected steps; the endpoint id is the request-derived variable part.
Cause 2: "$.syslogConfigurations[0]: extra (expected None, got {'logGroupArn': <arn>, 'sourceType': 'VPCE', 'vpcEndpointId': 'vpce-0000000000000000', 'createdAt': ...})" — the ARN and createdAt are per-run; the requirement is the empty list.
Cause 3: "status: expected 400, got 200; $: empty_body (expected {'__type': 'InvalidParameterException', 'message': 'VPC endpoint vpce-0000000000000000 is not associated with log group <logGroupName>'}, got {})" — 2 affected steps.
Cause 4: "$.message: value_mismatch (expected 'VPC endpoint vpce-0123456789abcdef0 is not associated with log group <logGroupName>', got 'No syslog configuration for vpcEndpointId [vpce-0123456789abcdef0] exists on this log group.')" at HTTP 400 -> 400.
Cause 5: "$.__type: value_mismatch (expected 'InvalidParameterException', got 'ResourceNotFoundException'); $.message: value_mismatch (expected 'vpcEndpointId is required', got 'The specified log group does not exist.')" on PutSyslogConfiguration.
Cause 6: the identical pair on DeleteSyslogConfiguration.

## Steps to Reproduce
1/2: CreateLogGroup, then PutSyslogConfiguration(logGroupIdentifier=<it>, vpcEndpointId='vpce-0000000000000000') — expect 400 InvalidParameterException 'VPC endpoint <id> failed validation. ...'; then ListSyslogConfigurations and expect an empty list (logs-put-syslog-configuration-invalid-vpc-endpoint-error#task#2, #task#3).
3/4: DeleteSyslogConfiguration(logGroupIdentifier=<a group with no syslog configuration>, vpcEndpointId=<any valid-looking id>) — expect 400 InvalidParameterException 'VPC endpoint <id> is not associated with log group <name>' (logs-syslog-config-invalid-vpc-endpoint-error#task#3, logs-syslog-config-delete-without-existing-config#task#3).
5/6: PutSyslogConfiguration and DeleteSyslogConfiguration with vpcEndpointId omitted and a logGroupIdentifier that does not exist — expect 400 InvalidParameterException 'vpcEndpointId is required' from both (logs-syslog-config-missing-vpc-endpoint-error#task#1, #task#2).
