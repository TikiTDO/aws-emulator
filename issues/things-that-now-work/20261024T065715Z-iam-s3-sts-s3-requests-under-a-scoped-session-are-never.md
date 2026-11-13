Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session iam__s3__sts.xsvc-iam-2-s3-role-arn-principal-authorizes-every-session
Issue-Key: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session|_poll#34|Timeout|time.sleep(interval_s)
Issue-Key: iam__s3__sts.xsvc-iam-2-s3-role-arn-principal-authorizes-every-session|_settle#29|Timeout|time.sleep(interval_s)
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# S3 requests under a scoped session are never denied: the MFA-conditioned deny and the role-ARN-principal bucket policy legs never converge and hit the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
ONE root cause (cause 1 of 1).

AWS: S3 evaluates the caller's session against the bucket policy and the session's identity/session policy on every request. A policy statement with Deny ... Condition BoolIfExists aws:MultiFactorAuthPresent=false refuses a plain (non-MFA) session with AccessDenied/403 while permitting an MFA-backed session; a bucket policy whose Principal is a ROLE ARN authorizes every session of that role (and only that role). Both checks establish a positive control first and then poll for the expected outcome, so a correct target converges in seconds.
Emulator: neither outcome is ever produced — both checks run to the 60 s harness cap with no divergent response captured, because the S3 request path performs no session-policy/bucket-policy condition evaluation at all (the same absence this suite shows for DynamoDB and STS sessions). The expected AccessDenied/403 for the non-MFA session never arrives, so the poll cannot terminate.
Why: aws:MultiFactorAuthPresent and role-ARN principals are the two standard ways a bucket is locked down; an emulator that silently permits everything tells a developer their bucket is protected when it is not.
Fix: the S3 request authorization step — evaluate the bucket policy (including Principal matching on the role ARN) and the caller's session policy, with condition-key support for aws:MultiFactorAuthPresent. Related but NOT covering this: open issue 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md (service token iam).
Traces: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session, iam__s3__sts.xsvc-iam-2-s3-role-arn-principal-authorizes-every-session.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

- iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session: "Error: Check timed out after 60.0s". The check polls for the plain session's S3 call to be refused with AccessDenied after a positive control proved the session live.
- iam__s3__sts.xsvc-iam-2-s3-role-arn-principal-authorizes-every-session: "Error: Check timed out after 60.0s". The check polls for a stable streak of the expected verdict ("wanted <want> <n>x in a row within <timeout>s").

No divergent response body was captured for either check: the expected refusal never occurs.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay both check ids in trace_ids.
3. Manual repro: create a bucket, attach a bucket policy with Effect=Deny, Action=s3:*, Condition {BoolIfExists: {aws:MultiFactorAuthPresent: false}}, assume a role without MFA and call s3.get_object/put_object with those credentials — the emulator serves the request where AWS answers AccessDenied/403.
