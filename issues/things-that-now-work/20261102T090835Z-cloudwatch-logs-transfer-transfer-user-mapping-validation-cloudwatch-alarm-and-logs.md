Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs__transfer.refarch-sftp-create-user-rejects-overlapping-home-directory-mappings cloudwatch__logs__transfer.refarch-sftp-update-user-overlapping-mappings-rejected-and-stored-mappings-kept cloudwatch__logs__transfer.refarch-sftp-put-metric-alarm-rejected-update-leaves-existing-config-unchanged cloudwatch__logs__transfer.refarch-sftp-put-metric-filter-rejects-malformed-filter-pattern cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle
Issue-Key: cloudwatch__logs__transfer.refarch-sftp-create-user-rejects-overlapping-home-directory-mappings|_error#4|AssertionError|raise AssertionError(f"expected the call to be refused, it succeeded: {result!r}")
Issue-Key: cloudwatch__logs__transfer.refarch-sftp-update-user-overlapping-mappings-rejected-and-stored-mappings-kept|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, it succeeded: {result!r}")
Issue-Key: cloudwatch__logs__transfer.refarch-sftp-put-metric-alarm-rejected-update-leaves-existing-config-unchanged|_error#6|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, which is the whole claim")
Issue-Key: cloudwatch__logs__transfer.refarch-sftp-put-metric-filter-rejects-malformed-filter-pattern|_error#4|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, which is the whole claim")
Issue-Key: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# Transfer user mapping validation, CloudWatch alarm and logs metric-filter validation are missing, and Transfer creates no vended log group (5 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
Five net-new root causes. Each must be closed before submitting.

Cause 1 of 5 — Transfer CreateUser accepts overlapping HomeDirectoryMappings.
AWS: Smithy (com.amazonaws.transfer) CreateUser with HomeDirectoryType=LOGICAL and HomeDirectoryMappings. Recorded behavior only for the text: two mappings whose Entry paths overlap (here `/` and `/inbound`, where `/` subsumes the other) are refused with InvalidRequestException and the message `[Contains overlapping mapping with: <entry>]` — fixed text `[Contains overlapping mapping with: ` and the closing `]`, with the overlapped Entry taken from the request — and no user is created (ListUsers stays empty). Two disjoint entries are accepted, so the overlap, not the count, is what is validated.
Emulator: answers HTTP 200 and creates the user, returning ServerId/UserName.
Why: a logical home directory with overlapping entries has no well-defined resolution, so AWS refuses it at create. Accepting it produces an SFTP user whose path mapping is ambiguous, and a caller relying on the refusal provisions a broken user and reports success.
Fix: the HomeDirectoryMappings overlap validation in CreateUser's validation layer in the transfer crate.
Traces: cloudwatch__logs__transfer.refarch-sftp-create-user-rejects-overlapping-home-directory-mappings

Cause 2 of 5 — Transfer UpdateUser accepts the same overlapping HomeDirectoryMappings, and the stored mappings are replaced.
AWS: UpdateUser applies the same overlap rule as CreateUser, refusing with InvalidRequestException `[Contains overlapping mapping with: <entry>]` and LEAVING the user's existing mappings in force.
Emulator: answers HTTP 200, returning ServerId/UserName, so the refused edit is applied to a live user.
Why: the refusal exists so a bad edit cannot strand a working user's path mapping; applying it silently breaks an in-service SFTP user.
Fix: the same overlap validator, wired into UpdateUser (it must run before any state mutation).
Traces: cloudwatch__logs__transfer.refarch-sftp-update-user-overlapping-mappings-rejected-and-stored-mappings-kept

Cause 3 of 5 — CloudWatch PutMetricAlarm accepts an out-of-enum TreatMissingData value and rewrites the alarm with it.
AWS: Smithy (com.amazonaws.cloudwatch) PutMetricAlarm, member TreatMissingData. Recorded behavior only for the text: a value outside {breaching, notBreaching, ignore, missing} is refused with error code `ValidationError` and the message `The value <value> is not supported for TreatMissingData parameter. Supported values are [breaching, notBreaching, ignore, missing].` — fixed text `The value `, ` is not supported for TreatMissingData parameter. Supported values are [breaching, notBreaching, ignore, missing].`, with only the rejected value taken from the request — and the existing alarm's configuration and its ConfigurationUpdate history are untouched.
Emulator: answers success, so the re-put replaces the existing alarm using the unsupported value.
Why: PutMetricAlarm is an upsert; a rejected request must not mutate the alarm it targets. Accepting an unsupported TreatMissingData both loses the refusal a caller validates against and silently reconfigures a live alarm's missing-data treatment, which decides whether it fires.
Fix: the TreatMissingData enum validation in PutMetricAlarm's validation layer in the cloudwatch crate, evaluated before the alarm is written.
Traces: cloudwatch__logs__transfer.refarch-sftp-put-metric-alarm-rejected-update-leaves-existing-config-unchanged

Cause 4 of 5 — logs PutMetricFilter accepts an unparseable JSON-selector filterPattern.
AWS: Smithy (com.amazonaws.cloudwatchlogs) PutMetricFilter, members logGroupName, filterName, filterPattern, metricTransformations; models InvalidParameterException with httpError 400. Model text only; a JSON-selector pattern that does not parse (here `{$.a = }`, a selector with no right-hand value) is refused with InvalidParameterException and the message `Invalid metric filter pattern` — entirely fixed text, no variable part — and no filter is created.
Emulator: answers success and stores the filter.
Why: the parse check is the only point at which a malformed pattern is reported. Stored, it silently matches nothing, so a metric filter that a caller believes is counting errors emits no datapoints and the alarm built on it never fires — with no error anywhere to explain it.
Fix: the filterPattern grammar validation shared by PutMetricFilter/TestMetricFilter/PutSubscriptionFilter in the logs crate's validation layer.
Traces: cloudwatch__logs__transfer.refarch-sftp-put-metric-filter-rejects-malformed-filter-pattern

Cause 5 of 5 — a Transfer server created with a LoggingRole has no /aws/transfer/<serverId> log group.
AWS: recorded behavior only. CreateServer with a LoggingRole provisions the service's vended log group named `/aws/transfer/<serverId>`, and DescribeLogGroups lists exactly that group for the server.
Emulator: DescribeLogGroups returns zero groups for that name, so the phase that would read SFTP activity logs cannot start.
Why: the vended group name is the documented, predictable place a client reads a Transfer server's logs and the destination its metric filters and alarms are built on; without it the whole logging/alarm chain for Transfer is unbuildable.
Fix: the vended-log-group provisioning performed by CreateServer in the transfer crate.
Traces: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

Cause 1 — refarch-sftp-create-user-rejects-overlapping-home-directory-mappings:
  AssertionError: expected the call to be refused, it succeeded: {'ServerId': '<serverId>', 'UserName': 'sftp-user', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  expected ('InvalidRequestException', '[Contains overlapping mapping with: /inbound]') for mappings [{'Entry': '/', ...}, {'Entry': '/inbound', ...}]

Cause 2 — refarch-sftp-update-user-overlapping-mappings-rejected-and-stored-mappings-kept:
  AssertionError: expected the call to be refused, it succeeded: {'ServerId': '<serverId>', 'UserName': 'sftp-user', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 3 — refarch-sftp-put-metric-alarm-rejected-update-leaves-existing-config-unchanged:
  AssertionError: the call SUCCEEDED; real AWS rejects it, which is the whole claim
  expected ('ValidationError', 'The value sometimes is not supported for TreatMissingData parameter. Supported values are [breaching, notBreaching, ignore, missing].')

Cause 4 — refarch-sftp-put-metric-filter-rejects-malformed-filter-pattern:
  AssertionError: the call SUCCEEDED; real AWS rejects it, which is the whole claim
  expected ('InvalidParameterException', 'Invalid metric filter pattern') for filterPattern '{$.a = }'

Cause 5 — refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle:
  AssertionError: [phase 1/4 deploy] DescribeLogGroups returned 0 groups named '/aws/transfer/<serverId>'

Server ids, user names and the value 'sometimes' above are this run's inputs/state filling the message templates; they are not requirements.

## Steps to Reproduce
Boot the emulator from this checkout.

Cause 1: aws transfer create-server --protocols SFTP --identity-provider-type SERVICE_MANAGED --logging-role <role> ; aws transfer create-user --server-id <id> --user-name sftp-user --role <role> --home-directory-type LOGICAL --home-directory-mappings 'Entry=/,Target=/b/sftp-user' 'Entry=/inbound,Target=/b/inbound' -> 200; AWS answers InvalidRequestException '[Contains overlapping mapping with: /inbound]' and creates no user.

Cause 2: create the user with two disjoint mappings, then aws transfer update-user with the overlapping pair above -> 200 and the mappings are replaced; AWS refuses and keeps the stored mappings.

Cause 3: aws cloudwatch put-metric-alarm --alarm-name a --metric-name M --namespace N --statistic Sum --period 60 --evaluation-periods 1 --threshold 1 --comparison-operator GreaterThanOrEqualToThreshold --treat-missing-data notBreaching ; repeat with --treat-missing-data sometimes -> success; AWS answers ValidationError with the supported-values sentence and leaves the alarm unchanged.

Cause 4: aws logs create-log-group --log-group-name g ; aws logs put-metric-filter --log-group-name g --filter-name f --filter-pattern '{$.a = }' --metric-transformations metricName=M,metricNamespace=N,metricValue=1,defaultValue=0 -> success; AWS answers InvalidParameterException 'Invalid metric filter pattern'.

Cause 5: create a Transfer server with --logging-role, then aws logs describe-log-groups --log-group-name-prefix /aws/transfer/<serverId> -> empty; AWS lists that group.

Then re-replay the five check ids in trace_ids.
