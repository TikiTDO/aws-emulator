Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__logs__sts.team-session-vended-reads-denied-runtime-reads-allowed
Issue-Key: iam__logs__sts.team-session-vended-reads-denied-runtime-reads-allowed|run#4|AssertionError|assert got == want, f"FilterLogEvents on the {kind} group returned {got!r}, expected {want!r}"
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# iam__logs__sts: the AccessDeniedException message for logs:FilterLogEvents appends ':log-stream:' to the log-group resource ARN

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FilterLogEvents

## Description
One net-new root cause. The check is not marked ALREADY FILED and is not OWNED.

Cause 1 of 1: the resource ARN rendered into the logs authorization-failure message carries a trailing ':log-stream:' that AWS does not emit for a log-group-scoped action.
AWS: when an assumed-role session is denied logs:FilterLogEvents by an explicit deny, CloudWatch Logs answers HTTP 400 with error code AccessDeniedException and the message `User: <session arn> is not authorized to perform: logs:FilterLogEvents on resource: <log-group arn> with an explicit deny in an identity-based policy`. The fixed text is `User: `, ` is not authorized to perform: `, ` on resource: ` and ` with an explicit deny in an identity-based policy`; the variable parts are the caller's assumed-role session arn, the action name, and the resource arn — which for a log-group-scoped read is the log-group ARN and nothing more (`arn:aws:logs:<region>:<account>:log-group:<name>`). Smithy: logs FilterLogEvents takes logGroupName/logGroupIdentifier and declares InvalidParameterException / ResourceNotFoundException / ServiceUnavailableException; the AccessDeniedException is the service-common authorization refusal.
Emulator: the same refusal is returned with the resource rendered as `arn:aws:logs:<region>:<account>:log-group:<name>:log-stream:` — a stray `:log-stream:` suffix with an empty stream component. Error code (AccessDeniedException) and status are right; only the resource ARN inside the message diverges.
Why: the resource ARN in a denial message is what a client parses or greps to repair the policy — it is copied into the Resource element of the fix. The emulator's form names a log-stream resource that was never requested, so an operator (or an automated policy-repair tool) widens the wrong resource, and message assertions in tests fail against an ARN shape AWS never produces.
Fix: the CloudWatch Logs authorization layer's resource-ARN rendering for the AccessDeniedException message — a log-group-scoped action must render the log-group ARN without appending the log-stream segment.
Traces: iam__logs__sts.team-session-vended-reads-denied-runtime-reads-allowed.

## Evidence
Cause 1 — Expected: `User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: logs:FilterLogEvents on resource: arn:aws:logs:<region>:<account>:log-group:<group> with an explicit deny in an identity-based policy`. Actual: the same message but with the resource rendered as `arn:aws:logs:<region>:<account>:log-group:<group>:log-stream:` — per the replay, `...:log-group:/aws/vendedlogs/bedrock-agentcore/runtime/APPLICATION_LOGS/<name>:log-stream: with an explicit deny in an identity-based policy` (role, session, account, region and group name are per-run values; the stray ':log-stream:' is the defect).

## Steps to Reproduce
1. Create a role with a policy that explicitly denies logs:FilterLogEvents on the application log group, and assume it via sts.
2. Call logs FilterLogEvents(logGroupName=<that group>) with the assumed-role session.
Observed: AccessDeniedException whose message renders the resource as '<log-group arn>:log-stream:'. Expected: the resource rendered as the bare log-group ARN.
