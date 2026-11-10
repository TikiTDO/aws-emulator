Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session cloudtrail.lookup-events-records-a-denied-call-with-errorcode-and-correlates-the-sdk-request-id cloudtrail__iam.obs-iam-1-absent-user-get-is-audited-as-no-such-entity-exception cloudtrail__cognito-idp.obs-cognito-idp-3-failed-and-denied-admin-get-user-calls-are-audited-with-their-error-code cloudtrail__stepfunctions.obs-sfn-2-lookup-events-audits-start-execution-and-a-rejected-describe-state-machine
Issue-Key: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session|_error_of#33|AssertionError|raise AssertionError("the call succeeded; expected it to be refused")
Issue-Key: cloudtrail.lookup-events-records-a-denied-call-with-errorcode-and-correlates-the-sdk-request-id|run#21|AssertionError|raise AssertionError(
Issue-Key: cloudtrail__iam|GetUser|NoSuchEntity
Issue-Key: cloudtrail__cognito-idp.obs-cognito-idp-3-failed-and-denied-admin-get-user-calls-are-audited-with-their-error-code|run#23|KeyError|ok_request_id = ok["ResponseMetadata"]["RequestId"]
Issue-Key: cloudtrail__stepfunctions.obs-sfn-2-lookup-events-audits-start-execution-and-a-rejected-describe-state-machine|run#23|KeyError|start_request_id = started["ResponseMetadata"]["RequestId"]
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# No IAM authorization is evaluated (calls a session's policy does not grant succeed), and responses carry no RequestId in ResponseMetadata

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
TWO root causes. BOTH must be closed before submitting.

Cause 1 of 2: the emulator does not evaluate the caller's IAM policy — an action the session's only policy does not grant succeeds instead of being refused.
AWS: recorded behavior (both checks' measured oracles, us-west-2, 2026-09-13 / 2026-10-01). A call made with an assumed-role session whose single inline policy does not grant the action is refused: `iam:CreateUser` raises SDK error code `AccessDenied` (HTTP 403) with a message naming both `iam:CreateUser` and the assumed-role session ARN `arn:aws:sts::<account>:assumed-role/<role>/<session>`; `logs:DescribeLogGroups` raises SDK error code `AccessDeniedException` for a principal whose only permission is `sts:GetCallerIdentity`. (Smithy models no per-operation authorization error for these — the refusal is the service-common access-denied fault — so the requirement is the status and code, and the message rule is: it names the denied action and the requesting session principal.)
Emulator: both calls SUCCEED. `limited.create_user(UserName=...)` under a session whose role carries one inline policy granting only `sts:GetCallerIdentity` returns a created user; `logs.describe_log_groups()` under the same session returns the log groups. The emulator is over-permissive: no policy evaluation happens on the request path.
Why: every deny-path scenario is unreachable and every least-privilege test silently passes. A client cannot verify that its policies actually restrict anything, cannot exercise AccessDenied handling or retries/fallbacks, and a role that is misconfigured in production looks correct against the emulator. It also makes the emulator unusable for auditing rejected calls, since there are no rejections to audit.
Fix: the request-path authorization seam shared by the service crates (the authenticated principal's resolved session + its attached/inline policies must be evaluated before the handler runs, returning the service-common access-denied fault), not the individual CreateUser / DescribeLogGroups handlers. Reproduced for iam:CreateUser and logs:DescribeLogGroups; it is the same missing evaluation for both.
Traces: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session, cloudtrail.lookup-events-records-a-denied-call-with-errorcode-and-correlates-the-sdk-request-id.

Cause 2 of 2: responses carry no request id, so `ResponseMetadata['RequestId']` is absent.
AWS: recorded behavior: every AWS response, success or error, carries a request id (`x-amzn-RequestId` / `x-amz-request-id`, or `RequestId` in the awsQuery XML envelope), and botocore surfaces it as `ResponseMetadata.RequestId` on both success responses and on `ClientError.response`. It is the join key every audit/correlation workflow uses (each of these checks pins a CloudTrail record to its own call by exactly that value).
Emulator: `ResponseMetadata` has no `RequestId` key at all — the checks die with `KeyError: 'RequestId'` reading it off an IAM GetUser refusal, a Cognito AdminGetUser success and a Step Functions StartExecution success. Note some other services in this run DO return one (a CloudFormation CreateStack response in the same suite shows `'RequestId': '332ea88a-...'`), so the gap is in the response-metadata path these services take, not in the clients.
Why: any client that logs, correlates or de-duplicates by request id gets nothing; SDK retry/telemetry that records the request id of a failed call breaks, and support-style workflows ("quote the request id of the failing call") are impossible. Observed across iam, cognito-idp and sfn, so it is one framework-level response-metadata defect rather than three service defects.
Fix: the response-serialization/metadata path in the server runtime — mint a request id per request and emit it as the protocol's request-id header (and in the awsQuery/restXml envelopes) for success AND error responses.
Traces: cloudtrail__iam.obs-iam-1-absent-user-get-is-audited-as-no-such-entity-exception, cloudtrail__cognito-idp.obs-cognito-idp-3-failed-and-denied-admin-get-user-calls-are-audited-with-their-error-code, cloudtrail__sfn.obs-sfn-2-lookup-events-audits-start-execution-and-a-rejected-describe-state-machine.

## Evidence
Cause 1:
- `AssertionError: the call succeeded; expected it to be refused` raised at `lambda: limited.create_user(UserName=denied_user)` (cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session). That check's measured oracle: "the SDK raises error code 'AccessDenied' for the refused iam:CreateUser ... the audit errorMessage ... names both 'iam:CreateUser' and the assumed-role SESSION ARN arn:aws:sts::<acct>:assumed-role/<role>/<session>".
- `AssertionError: logs:DescribeLogGroups SUCCEEDED for a principal whose only permission is sts:GetCallerIdentity. There is no denial to audit, and the target is over-permissive: role 'sim2real-behav-f4denied-<id>' carries one inline policy granting sts:GetCallerIdentity alone.` (cloudtrail.lookup-events-records-a-denied-call-with-errorcode-and-correlates-the-sdk-request-id). That check's measured oracle: "the SDK raises AccessDeniedException; CloudTrail records errorCode 'AccessDenied'".

Cause 2:
- `exc.response["ResponseMetadata"]["RequestId"]` -> `KeyError: 'RequestId'` on an `iam.get_user(UserName=<absent>)` refusal (cloudtrail__iam.obs-iam-1-absent-user-get-is-audited-as-no-such-entity-exception).
- `ok["ResponseMetadata"]["RequestId"]` -> `KeyError: 'RequestId'` on a successful cognito-idp AdminGetUser (cloudtrail__cognito-idp.obs-cognito-idp-3-failed-and-denied-admin-get-user-calls-are-audited-with-their-error-code).
- `started["ResponseMetadata"]["RequestId"]` -> `KeyError: 'RequestId'` on a successful sfn StartExecution (cloudtrail__sfn.obs-sfn-2-lookup-events-audits-start-execution-and-a-rejected-describe-state-machine).

## Steps to Reproduce
Cause 1:
1. CreateRole with a trust policy allowing the caller to assume it, and attach ONE inline policy granting only `sts:GetCallerIdentity` on `*`.
2. AssumeRole into it and build clients from the returned session credentials.
3. Call `iam.create_user(UserName='u-1')`: expect HTTP 403 `AccessDenied` with a message naming `iam:CreateUser` and the assumed-role session ARN; observe a successful create.
4. Call `logs.describe_log_groups()` with the same session: expect `AccessDeniedException`; observe HTTP 200.

Cause 2:
5. Call `iam.get_user(UserName='<absent-user>')` and read `exc.response['ResponseMetadata']['RequestId']`: expect a request id string; observe KeyError (no RequestId key).
6. Call `sfn.start_execution(...)` or `cognito-idp AdminGetUser` and read `resp['ResponseMetadata']['RequestId']`: same KeyError. Check the raw response for an `x-amzn-RequestId` header — it is absent.
