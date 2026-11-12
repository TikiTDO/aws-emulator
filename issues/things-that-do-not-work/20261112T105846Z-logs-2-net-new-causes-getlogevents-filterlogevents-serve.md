Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.unmask-without-permission-denied-while-masked-read-still-served logs.data-protection-audit-findings-delivered-to-the-findings-destination
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.unmask-without-permission-denied-while-masked-read-still-served|_client_error#43|AssertionError|raise AssertionError(
Issue-Key: logs.data-protection-audit-findings-delivered-to-the-findings-destination|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# logs: 2 net-new causes — GetLogEvents/FilterLogEvents serve unmask=true without logs:Unmask, and Audit FindingsDestination never receives findings

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLogEvents

## Description
Two net-new root causes on the CloudWatch Logs data-protection path. Both are grounded in failing behavioral checks of the `logs` service; masking on the ingest path itself works (the canary leg of both checks passes), so these are defects in the read-path authorization and in the Audit half of a data-protection policy, not in masking.

Cause 1 of 2: the `unmask` read-path parameter is served to a principal that has no `logs:Unmask` permission.
  AWS:      Smithy: `GetLogEventsRequest.unmask` (target `Unmask`, boolean, default false) is documented as "Specify true to display the log event fields with all sensitive data unmasked and visible. The default is false. To use this operation with this parameter, you must be signed into an account with the logs:Unmask permission." `FilterLogEventsRequest.unmask` carries the same member and the same requirement. A caller whose identity-based policy allows logs:GetLogEvents / logs:FilterLogEvents / logs:DescribeLogStreams on the log group but NOT logs:Unmask is therefore refused when, and only when, it passes unmask=true: AWS answers HTTP 400 with error code `AccessDeniedException` (logs is a JSON-1.1 service and classifies authorization refusals as 400 client errors) and the standard IAM implicit-deny message. The message's FIXED text is `User: ` … ` is not authorized to perform: logs:Unmask on resource: ` … ` because no identity-based policy allows the logs:Unmask action`. Its VARIABLE parts come from the request and from IAM state: the caller's session ARN (`arn:aws:sts::<account>:assumed-role/<role-name>/<role-session-name>`, built from the AssumeRole call that produced the signing credentials) and the resource ARN being read — for GetLogEvents that is the log-group ARN with the log-stream qualifier appended, `arn:aws:logs:<region>:<account>:log-group:<logGroupName>:log-stream:`, and for FilterLogEvents it is the bare log-group ARN `arn:aws:logs:<region>:<account>:log-group:<logGroupName>` (FilterLogEvents is scoped to the group, not to a stream). The same call WITHOUT unmask must still succeed and return the de-identified event, and granting logs:Unmask to that same role must make the identical unmask=true call succeed and return the original event verbatim.
  Emulator: the refusal never happens. `GetLogEvents(logGroupName=<group>, logStreamName=<stream>, startFromHead=true, unmask=true)`, signed by the assumed role whose only inline policy allows GetLogEvents/FilterLogEvents/DescribeLogStreams on that group, returns HTTP 200 with the events. The check's own words: "GetLogEvents(unmask=True) was expected to be DENIED and it SUCCEEDED; the caller was served de-identified data it has no permission to see" (raised at check line 281 via the `_client_error` helper, which only raises when the call did not fault). The emulator therefore evaluates no `logs:Unmask` authorization at all on the read path; the FilterLogEvents(unmask=true) leg immediately after is never reached, so it is unverified but is the same hook and must be guarded identically.
  Why:      `logs:Unmask` is the permission boundary that makes data-protection masking meaningful. A client that relies on AWS's behavior uses it to let operators read a protected log group while withholding the raw PII from them; against the emulator every such operator recovers the raw identifier by flipping one request boolean. Any test or policy review that asserts an unprivileged principal is denied unmasked reads passes against the emulator and is wrong on AWS-equivalent infrastructure, and the inverse — code that catches AccessDeniedException to fall back to a masked read — never exercises its fallback.
  Fix:      the read-path authorization for GetLogEvents and FilterLogEvents in the cloudwatchlogs service: the `unmask` parameter must be gated on an IAM authorization decision for the `logs:Unmask` action against the log-group (and, for GetLogEvents, log-stream-qualified) resource ARN before the handler de-identifies-or-not, and the refusal must serialize as AccessDeniedException with the message template above. The unmask=false path must be left untouched.
  Traces:   logs.unmask-without-permission-denied-while-masked-read-still-served.

Cause 2 of 2: a data-protection Audit statement never delivers findings to its CloudWatch Logs FindingsDestination.
  AWS:      recorded behavior only — the Smithy model types `PutDataProtectionPolicy.policyDocument` as an opaque `DataProtectionPolicyDocument` string and says nothing about Audit findings, so the requirement is the recorded behavior of the policy document. A policy whose statements are (a) `Operation.Audit.FindingsDestination.CloudWatchLogs.LogGroup = <destination group>` over DataIdentifier `arn:aws:dataprotection::aws:data-identifier/EmailAddress` and (b) `Operation.Deidentify.MaskConfig` over the same identifier is accepted, and a subsequent PutLogEvents into the protected group whose message contains an email address causes BOTH halves to fire on that one ingestion: the source event is stored with only the matched identifier redacted, and a finding — a JSON document that NAMES the detected identifier type (`EmailAddress`) and does NOT reproduce the matched value — is delivered as a log event into the destination log group, readable by FilterLogEvents on that group. An event containing no matched identifier is stored verbatim and produces no finding.
  Emulator: the Deidentify half fires (the check's canary leg, which polls until an ingested email comes back masked, passes), but the destination log group stays empty: the check hangs in the `_findings` poll — FilterLogEvents on the findings destination returns no JSON-parsable event — and is killed by the harness, reported as `Check timed out after 60.0s`. Hypothesis: the ingest-time detector applies only the Deidentify statement and the Audit statement's FindingsDestination is parsed and stored but never routed to, so no finding is ever emitted. The downstream assertions (the finding names EmailAddress; the finding does not leak the raw value; the benign event is stored verbatim) are consequently unverified.
  Why:      a client relying on AWS's behavior uses the audit findings stream as the detection record: it alarms, counts or reports on which data identifiers were seen in which log group without ever being granted access to the raw values. Against the emulator that stream is permanently empty, so detection looks like "no sensitive data was ever found" — a silent false negative in exactly the signal that exists to be noisy.
  Fix:      the ingest-path data-protection evaluation in the cloudwatchlogs service: after a DataIdentifier match, the Audit statement must emit a finding event into the log group named by `Operation.Audit.FindingsDestination.CloudWatchLogs.LogGroup` (describing the detection — the identifier type and location — and never the matched value), in addition to, not instead of, applying the Deidentify statement to the stored event.
  Traces:   logs.data-protection-audit-findings-delivered-to-the-findings-destination.

Neither cause is covered by an already-filed issue. The nearest open issues are 20261029T182816Z-logs-transformers-are-not-applied-on-ingest-data.md (which claims data-protection masking never happens — masking now DOES happen, so it does not describe either cause above) and 20261029T222444Z-logs-policy-operations-account-resource-index-data-protection.md (status/message divergences of the policy CRUD operations, not the ingest or read path). The replay report marks neither failing check as ALREADY FILED or OWNED.

## Evidence
Cause 1 — logs.unmask-without-permission-denied-while-masked-read-still-served [mismatch]
  Setup that the emulator did honour: create_log_group, create_log_stream, PutDataProtectionPolicy with an Audit + Deidentify(MaskConfig) pair over arn:aws:dataprotection::aws:data-identifier/EmailAddress; the canary poll confirms the policy is live on ingest; the privileged masked read returns the event with the address redacted; an IAM role is created whose single inline policy allows logs:GetLogEvents, logs:FilterLogEvents, logs:DescribeLogStreams on the group ARN and the group ARN + ":*" and does NOT mention logs:Unmask; AssumeRole on it succeeds; the positive control (the same assumed role, plain GetLogEvents, no unmask) returns exactly the one masked event.
  Expected: GetLogEvents(..., unmask=True) by that assumed role faults.
    HTTP 400, Error.Code == "AccessDeniedException",
    Error.Message == "User: <session-arn> is not authorized to perform: logs:Unmask on resource: <log-group-arn>:log-stream: because no identity-based policy allows the logs:Unmask action"
    where <session-arn> = arn:aws:sts::<account>:assumed-role/<role-name>/<role-session-name> and <log-group-arn> = arn:aws:logs:<region>:<account>:log-group:<logGroupName>.
    And FilterLogEvents(logGroupName=<group>, unmask=True) faults the same way with resource <log-group-arn> (no ":log-stream:" suffix).
    And the unmasked content must reach that principal through nothing.
  Actual: HTTP 200 with the log events. The check's assertion fires in its _client_error helper, which is only reached on a non-faulting call:
    File "check_logs_unmask_without_permission_denied_while_masked_read_still_served.py", line 281, in run
      code, message = _client_error(lambda: _limited_read(unmask=True))
    File "...", line 31, in _client_error
      raise AssertionError(
    AssertionError: GetLogEvents(unmask=True) was expected to be DENIED and it SUCCEEDED; the caller was served de-identified data it has no permission to see
  Delta: expected error code AccessDeniedException (HTTP 400); actual: no error, 200 + de-identified-then-unmasked payload. No logs:Unmask authorization check exists on either read path.

Cause 2 — logs.data-protection-audit-findings-delivered-to-the-findings-destination [mismatch]
  Setup: destination log group created first; protected source group + stream created; PutDataProtectionPolicy accepted with statement Sid "audit-email" = Operation.Audit.FindingsDestination.CloudWatchLogs.LogGroup = <destination> over the EmailAddress data identifier, and statement Sid "mask-email" = Operation.Deidentify.MaskConfig over the same identifier. The canary poll (PutLogEvents of "canary <email> canary", GetLogEvents until the address is gone and "*" is present) SUCCEEDS, so masking is live. Two events are then ingested into the source stream: "login attempt from user <email> failed" and "login attempt from user with no address failed".
  Expected: FilterLogEvents on the destination log group returns at least one JSON finding; some finding's JSON names "EmailAddress"; no finding's JSON contains the matched address; the source stream then reads back two events, the first redacted but still starting with "login attempt from user ", the second stored verbatim.
  Actual: the destination group yields no JSON-parsable event at all. The check never leaves its 240s/10s findings poll and is killed by the harness:
    Error: Check timed out after 60.0s
  Delta: expected >= 1 audit finding delivered to the FindingsDestination log group; actual: 0 findings, indefinitely. The Deidentify half of the identical policy fires, so acceptance and detection work and only the Audit routing is missing.

## Steps to Reproduce
Cause 1 (logs:Unmask not authorized):
1. logs.CreateLogGroup(logGroupName=G); logs.CreateLogStream(logGroupName=G, logStreamName=S).
2. logs.PutDataProtectionPolicy(logGroupIdentifier=G, policyDocument={Name:N, Version:"2021-06-01", Statement:[{Sid:"audit-email", DataIdentifier:["arn:aws:dataprotection::aws:data-identifier/EmailAddress"], Operation:{Audit:{FindingsDestination:{}}}}, {Sid:"mask-email", DataIdentifier:[same], Operation:{Deidentify:{MaskConfig:{}}}}]}).
3. Ingest a canary into a throwaway stream until GetLogEvents returns it masked, so the policy is live on the ingest path.
4. logs.PutLogEvents into S with message "login attempt from user jane.doe@example.com failed"; confirm the privileged GetLogEvents returns it masked.
5. iam.CreateRole R trusting arn:aws:iam::<account>:root; iam.PutRolePolicy on R allowing ONLY logs:GetLogEvents, logs:FilterLogEvents, logs:DescribeLogStreams on arn:aws:logs:<region>:<account>:log-group:G and ...:log-group:G:* (no logs:Unmask).
6. sts.AssumeRole(RoleArn=R, RoleSessionName="sess") and build a logs client from those credentials.
7. With that client: GetLogEvents(logGroupName=G, logStreamName=S, startFromHead=true) — must return the one masked event (positive control; passes today).
8. With that client: GetLogEvents(logGroupName=G, logStreamName=S, startFromHead=true, unmask=true).
   Observed: HTTP 200 + events. Expected: HTTP 400 AccessDeniedException with the logs:Unmask implicit-deny message naming the session ARN and arn:aws:logs:<region>:<account>:log-group:G:log-stream:.
9. With that client: FilterLogEvents(logGroupName=G, unmask=true) — must fault the same way, naming the bare log-group ARN.
10. iam.PutRolePolicy on the SAME role R granting logs:Unmask on those two ARNs, then re-issue step 8 — it must now succeed and return the original message verbatim.

Cause 2 (Audit findings never delivered):
1. logs.CreateLogGroup(logGroupName=D)  # findings destination
2. logs.CreateLogGroup(logGroupName=G); logs.CreateLogStream(logGroupName=G, logStreamName=S).
3. logs.PutDataProtectionPolicy(logGroupIdentifier=G, policyDocument={Name:N, Version:"2021-06-01", Statement:[{Sid:"audit-email", DataIdentifier:["arn:aws:dataprotection::aws:data-identifier/EmailAddress"], Operation:{Audit:{FindingsDestination:{CloudWatchLogs:{LogGroup:D}}}}}, {Sid:"mask-email", DataIdentifier:[same], Operation:{Deidentify:{MaskConfig:{}}}}]}).
4. Ingest a canary into a throwaway stream of G until GetLogEvents returns it masked (confirms the policy is live).
5. logs.PutLogEvents into S with two events: "login attempt from user jane.doe@example.com failed" and "login attempt from user with no address failed".
6. Poll logs.FilterLogEvents(logGroupName=D) (paging nextToken) for JSON-parsable events.
   Observed: none, ever — the poll never terminates. Expected: at least one finding whose JSON names "EmailAddress" and does not contain jane.doe@example.com.
7. logs.GetLogEvents(logGroupName=G, logStreamName=S, startFromHead=true): expect two events, the first redacted but still prefixed "login attempt from user ", the second verbatim.
