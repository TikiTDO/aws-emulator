Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.lookup-events-records-a-denied-call-with-errorcode-and-correlates-the-sdk-request-id
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.lookup-events-records-a-denied-call-with-errorcode-and-correlates-the-sdk-request-id|run#34|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 0dd76f1da95766ea213c05d03a6e5ff9181cf064

# CloudTrail serves no LookupEvents record for an IAM-denied logs:DescribeLogGroups (nor for the allowed CreateLogGroup control), so a refused call is never auditable

> _Note: this issue was identified against the code base at commit `0dd76f1da95766ea213c05d03a6e5ff9181cf064`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Cause 1 of 1: a call the emulator REFUSES on authorization never becomes a CloudTrail event-history record, and neither does the allowed control call in the same service; the check's bounded LookupEvents poll therefore never completes and the check is killed at the harness ceiling.

AWS: CloudTrail records management events for refused calls exactly as it does for successful ones (https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-event-reference-record-contents.html). For an IAM-denied logs:DescribeLogGroups the record carries errorCode "AccessDenied" — IAM's code, NOT the SDK's exception name AccessDeniedException — and an errorMessage naming the denied action and the denied principal. Its fixed text is AWS's denial template "User: <principal-arn> is not authorized to perform: <service>:<Action> on resource: <resource-arn> because no identity-based policy allows the <service>:<Action> action"; the variable parts (the assumed-role session ARN, the action, the resource ARN) come from the request and the caller's session, never from any one recording. requestParameters and responseElements are both null on a refused call, the envelope's Resources is empty, readOnly is true (a property of the operation, not of the outcome) with the envelope's ReadOnly the string "true", userIdentity.type is "AssumedRole" with sessionContext.sessionIssuer.userName the role and arn the assumed-role session ARN, the envelope's Username is the role session name, eventSource is logs.amazonaws.com, managementEvent is true, the envelope's EventId equals the embedded record's eventID, and requestID is byte-identical to the request id the SDK saw on the failed call. The allowed control (logs:CreateLogGroup) is recorded with no errorCode, requestParameters {"logGroupName": <name>}, readOnly false, Resources naming the created group, and its own matching requestID.

Emulator: the check reaches the audit poll — the denial itself was produced with the expected code, message and a request id, since each of those is asserted (fast) before the poll — and then LookupEvents never returns either record. Repeated paged LookupEvents calls over the EventName=DescribeLogGroups and EventName=CreateLogGroup attributes inside a window spanning the calls yield no event naming the run's role or the control log group, so the check exhausts its wall clock and the harness reports "Check timed out after 60.0s". Hypothesis: the emulator only audits (if at all) on a successful dispatch path, so an authorization refusal short-circuits before the audit hook; the control's absence additionally points at cloudwatch-logs management events not being recorded at all.

Why: 'find the trace for this failed call' is impossible. Every denied-access alarm, every least-privilege investigation and every requestID-based correlation between an SDK failure and its audit trail returns nothing, and a client cannot distinguish 'no one tried' from 'the attempt was refused and never logged'.

Note: open issues 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md and 20261029T064922Z-cloudtrail-2-net-new-root-causes-no-management.md describe neighbouring gaps (no non-CloudTrail management events; no record for CloudTrail's own read-only/refused calls); neither carries this check's replay key or the logs.amazonaws.com denied-call path, so it is filed here.

Fix: the audit-emission hook must run on the refusal path of the request pipeline (after the authorization decision, before the error response is serialized) as well as on the success path, and the CloudTrail event-history store/LookupEvents query path must serve cloudwatch-logs (logs.amazonaws.com) management events with the refused-call field values above, carrying the same request id the error response returned.

Traces: cloudtrail.lookup-events-records-a-denied-call-with-errorcode-and-correlates-the-sdk-request-id (verdict mismatch, timed out in the audit poll).

## Evidence
Expected: after an assumed-role session whose only grant is sts:GetCallerIdentity is denied logs:DescribeLogGroups, cloudtrail.lookup_events(LookupAttributes=[{AttributeKey:EventName, AttributeValue:DescribeLogGroups}]) returns an event whose CloudTrailEvent has errorCode "AccessDenied", an errorMessage naming logs:DescribeLogGroups and the role, requestParameters null, responseElements null, envelope Resources [], readOnly true / envelope ReadOnly "true", userIdentity.type "AssumedRole", and requestID equal to the request id botocore saw on the denied call; and a CreateLogGroup event for the allowed control with requestParameters {"logGroupName": <group>} and readOnly false.

Actual: no such event is ever returned. Paged LookupEvents over the window returns nothing naming the run's role or the control group, for either EventName, so the poll never satisfies its exit condition and the check is terminated: "Error: Check timed out after 60.0s". The DATAPLANE_UNIMPLEMENTED assertion the check would have raised reads: "CloudTrail produced no audit record for an API call it REFUSED ... A target that audits only successful dispatch has wired its authorization decision and its audit pipeline together on one path and not the other."

## Steps to Reproduce
1. Boot the emulator. iam.create_role with a trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole, then iam.put_role_policy granting ONLY sts:GetCallerIdentity on "*".
2. sts.assume_role that role; build a logs client from the returned temporary credentials.
3. Call logs.describe_log_groups(logGroupNamePrefix=<unique>) with that session -> it is refused (AccessDeniedException); note ResponseMetadata.RequestId.
4. With the ambient principal, logs.create_log_group(logGroupName=<unique>) as the allowed control; note its RequestId.
5. Page cloudtrail.lookup_events over a window covering both calls with LookupAttributes EventName=DescribeLogGroups and EventName=CreateLogGroup. Expected: both records, the denied one with errorCode AccessDenied, null requestParameters/responseElements and requestID equal to step 3's id. Actual: neither record ever appears.
