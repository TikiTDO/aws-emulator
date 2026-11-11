Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-idp.obs-cognito-idp-3-failed-and-denied-admin-get-user-calls-are-audited-with-their-error-code
Issue-Key: cloudtrail__cognito-idp.obs-cognito-idp-3-failed-and-denied-admin-get-user-calls-are-audited-with-their-error-code|run#42|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 0dd76f1da95766ea213c05d03a6e5ff9181cf064

# cloudtrail__cognito-idp: no cognito-idp.amazonaws.com AdminGetUser record is ever served by LookupEvents — successful, failed (UserNotFound) and denied calls are all unauditable

> _Note: this issue was identified against the code base at commit `0dd76f1da95766ea213c05d03a6e5ff9181cf064`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminGetUser

## Description
Cause 1 of 1: Cognito user-pool management calls are not recorded in CloudTrail event history, so an AdminGetUser that succeeded, one that failed with UserNotFoundException and one that was refused with AccessDeniedException are all invisible to LookupEvents; the check's bounded poll never completes and it is killed at the harness ceiling.

AWS: AdminGetUser is a cognito-idp management event and is recorded for all three outcomes (https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-event-reference-record-contents.html). Each record carries eventSource "cognito-idp.amazonaws.com", eventName "AdminGetUser", readOnly true — AdminGetUser is a read whether or not it succeeded — recipientAccountId the caller's account, and requestID byte-identical to the request id the SDK saw on that exact call, which is how the three records are told apart. The successful record has NO errorCode/errorMessage, requestParameters.userPoolId equal to the pool and additionalEventData.sub equal to the user's sub. The failed record carries the service error code, and the denied record carries the access-denied classification with AWS's denial template "User: <session-arn> is not authorized to perform: cognito-idp:AdminGetUser on resource: arn:aws:cognito-idp:<region>:<account>:userpool/<poolId> because no identity-based policy allows the cognito-idp:AdminGetUser action" — fixed text as quoted, with the session ARN, region, account and pool id taken from the request and the caller's session, never from one recording. On the API side the refusal is AccessDeniedException and the unknown-user refusal is UserNotFoundException with message exactly "User does not exist.".

Emulator: the check performs all three AdminGetUser calls and only then polls; the API-side assertions (codes, exact messages, three distinct request ids) precede the poll and fail fast, so the poll is where the budget goes. Paged LookupEvents on LookupAttributes EventName=AdminGetUser over a window covering the calls returns no event whose CloudTrailEvent contains any of the three request ids, for the whole budget, and the harness reports "Check timed out after 60.0s". Hypothesis: the emulator records no cognito-idp.amazonaws.com management events at all, so there is nothing for the requestID join to find for any outcome.

Why: user-pool administration is unauditable. A client cannot answer who read which user, cannot alarm on repeated UserNotFound probes (credential/user enumeration) or on denied AdminGetUser attempts, and cannot join an SDK failure to its audit record by request id — the record simply does not exist.

Note: open issue 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md covers the neighbouring 'event history only holds CloudTrail's own writes' gap and owns other checks, but not this check's replay key or the AdminGetUser success/failure/denial record set, so it is filed here.

Fix: the CloudTrail event-history recording path must receive cognito-idp management events from the cognito-idp request pipeline on all three outcomes (success, service error, authorization refusal), and the LookupEvents query path must serve them with the eventSource/eventName/readOnly/recipientAccountId/requestID and per-outcome error and requestParameters fields above.

Traces: cloudtrail__cognito-idp.obs-cognito-idp-3-failed-and-denied-admin-get-user-calls-are-audited-with-their-error-code (verdict mismatch, timed out in the audit poll).

## Evidence
Expected: cloudtrail.lookup_events(LookupAttributes=[{AttributeKey:EventName, AttributeValue:AdminGetUser}]) over a window covering the three calls returns three records joined by requestID — the control with no errorCode/errorMessage, requestParameters.userPoolId = the pool and additionalEventData.sub = the user's sub; the unknown-user call with its UserNotFound error classification; the unauthorized session's call with the access-denied classification and the AWS denial message naming cognito-idp:AdminGetUser, the session ARN and the user-pool ARN. All three: eventSource cognito-idp.amazonaws.com, eventName AdminGetUser, readOnly true, recipientAccountId = account.

Actual: LookupEvents returns no AdminGetUser event containing any of the three request ids, for the entire poll; the check is terminated with "Error: Check timed out after 60.0s". The assertion it would have raised reads: "DATAPLANE_UNIMPLEMENTED: CloudTrail produced no AdminGetUser record for the ['control','denied','failed'] call(s) ... Real AWS indexed all three in ~50 s. A target that audits only successful calls blinds every failed-access alarm."

## Steps to Reproduce
1. Boot the emulator. idp.create_user_pool(PoolName=<unique>) and idp.admin_create_user(UserPoolId=<pool>, Username=<user>, MessageAction="SUPPRESS").
2. Control: idp.admin_get_user(UserPoolId=<pool>, Username=<user>) -> 200; note ResponseMetadata.RequestId and the user's sub.
3. Failed: idp.admin_get_user(UserPoolId=<pool>, Username=<never-created>) -> UserNotFoundException, message "User does not exist."; note its RequestId.
4. Denied: iam.create_role (trust: account root may sts:AssumeRole) + iam.put_role_policy granting only cognito-idp:DescribeUserPool; sts.assume_role; from those credentials call admin_get_user on the existing user -> AccessDeniedException with the denial template above; note its RequestId.
5. Page cloudtrail.lookup_events with LookupAttributes EventName=AdminGetUser over a window covering steps 2-4. Expected: three records joined by those request ids. Actual: none, for the whole budget.
