Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.resolver-query-log-config-bad-destination-rejected
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.resolver-query-log-config-bad-destination-rejected|run#29|AssertionError|assert refusal == ("AccessDeniedException", "[RSLVR-01600] Access denied"), (
Issue-Commit: 40d5826a48481a0d3e9807fb6b86c6b32d9a493b

# CreateResolverQueryLogConfig: the missing-destination refusal carries the wrong RSLVR code and message ([RSLVR-01803] Access to resource is denied. instead of [RSLVR-01600] Access denied)

> _Note: this issue was identified against the code base at commit `40d5826a48481a0d3e9807fb6b86c6b32d9a493b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolverQueryLogConfig

## Description
ONE net-new root cause in the route53resolver service.

Context on what does NOT cover this. The open issue 20261027T062807Z-route53resolver-5-net-new-causes-query-log-config.md (Cause 3) and the fixed wire issue 20261029T234450Z-route53resolver-route-53-resolver-vpc-config-dnssec-query-log.md (Cause 7) both concerned CreateResolverQueryLogConfig ACCEPTING (HTTP 200) a DestinationArn whose log group does not exist. That accept is gone: the handler now refuses with HTTP 400 AccessDeniedException, which is the right status and the right modeled error. What remains, and what this issue is about, is a different failure mode: the refusal's Error.Message text (and therefore its bracketed RSLVR service error code) is not the one Route 53 Resolver produces for this refusal class. The emulator is NOT leaving a call refused that should succeed — AWS refuses this call too.

Cause 1 of 1: the AccessDeniedException Route 53 Resolver returns when CreateResolverQueryLogConfig names a CloudWatch Logs destination the account does not hold carries the message '[RSLVR-01600] Access denied'; the emulator returns '[RSLVR-01803] Access to resource is denied.'

AWS: Smithy: CreateResolverQueryLogConfig(Name: ResolverQueryLogConfigName @length 1..64 @pattern ^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$ required, DestinationArn: DestinationArn @length 1..600 required, CreatorRequestId @length 1..255 required, Tags) -> {ResolverQueryLogConfig}; error AccessDeniedException at httpError 400 (defaulted), documented "The current account doesn't have the IAM permissions required to perform the specified Resolver operation. This error can also be thrown when a customer has reached the 5120 character limit for a resource policy for CloudWatch Logs." The model states no message text. Recorded behavior (wire/route53resolver/query-log-config-missing-resource-errors.json step 3, the recording the behavioral check names): a well-formed CloudWatch Logs log-group ARN for a log group that was never created is refused HTTP 400 with Error.Code AccessDeniedException and Error.Message '[RSLVR-01600] Access denied'. That message is ENTIRELY fixed text for this refusal class — it interpolates no name, id, ARN or limit; the bracketed RSLVR-01600 code is what ties the refusal to the destination check rather than to the caller's own IAM. Resolver appends its per-request ' Trace Id: "..."' suffix, which is a per-run value and no part of the requirement (the check strips it before comparing). No configuration is stored in any status, which the emulator already gets right.

Emulator: HTTP 400, Error.Code AccessDeniedException (correct), Error.Message '[RSLVR-01803] Access to resource is denied.' — a different bracketed RSLVR code and different prose (an extra 'to resource is' clause and a terminating period AWS's text does not have). Produced by the destination-ownership branch of the handler, services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_resolver_query_log_config.rs:240-252, which builds the exception through the shared helper crate::state::resolver_access_denied(...); that helper's text is the RSLVR-01803 sentence.

Why: every Route 53 Resolver fault answers HTTP 400, so the error code plus the bracketed [RSLVR-nnnnn] code inside Error.Message are the only discriminators a client has. A client, log pipeline or runbook that matches '[RSLVR-01600]' (or the whole sentence) to distinguish 'the query-log destination is missing or unwritable' from a genuine IAM denial never matches against the emulator, and operators reading the emulator's text are pointed at the wrong Resolver error code when they look it up. Since Error.Code is identical for both classes, nothing else in the response carries the distinction.

Fix: the destination-ownership refusal branch of CreateResolverQueryLogConfig's handler must raise AccessDeniedException with the RSLVR-01600 fixed sentence for this class, rather than reusing the generic RSLVR-01803 access-denied helper text; if crate::state::resolver_access_denied is legitimately the RSLVR-01803 message for other callers, this branch needs its own RSLVR-01600 constant beside the existing RSLVR_CODE_* ones in the crate's state module. The 400 status, the AccessDeniedException variant, the write-nothing behavior and the existing wildcard-stripping canonicalisation of DestinationArn are already correct and must not change.

Traces: route53resolver.resolver-query-log-config-bad-destination-rejected (the only non-owned failing check in this cluster).

## Evidence
Failing behavioral check route53resolver.resolver-query-log-config-bad-destination-rejected [mismatch].

All control legs of the check pass: a log group is created, CreateResolverQueryLogConfig against it is accepted, DestinationArn is stored in the canonical wildcard-stripped form, a second config naming the same destination is a distinct resource, and the first settles to CREATED with OwnerId = the caller's account.

The check then calls CreateResolverQueryLogConfig(Name=<unique>, DestinationArn='arn:aws:logs:<region>:<account>:log-group:<never-created-name>:*', CreatorRequestId=<unique>) and compares (Error.Code, Error.Message) with the X-Ray ' Trace Id: "..."' suffix stripped:

  AssertionError: expected AccessDeniedException '[RSLVR-01600] Access denied' (as captured in wire/route53resolver/query-log-config-missing-resource-errors.json step 3) for a query log config naming a nonexistent log group, got ('AccessDeniedException', '[RSLVR-01803] Access to resource is denied.')

(check line 178: assert refusal == ("AccessDeniedException", "[RSLVR-01600] Access denied")).

So: status 400 correct, Error.Code correct, Error.Message diverges. Expected fixed text: '[RSLVR-01600] Access denied' (no variable parts). Actual: '[RSLVR-01803] Access to resource is denied.'. The log-group name in the request is that run's generated value and is not part of either message.

Source: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_resolver_query_log_config.rs:240-252 — the !crate::state::account_holds_query_log_destination(...) branch returning AccessDeniedException built from crate::state::resolver_access_denied(crate::state::request_trace_root(...)).

This check's previous replay key was the 'call succeeded' assertion (the emulator returned HTTP 200); it now fails on the message comparison, i.e. the refusal exists but its text is wrong, which is why the open issue carrying the old key does not own this failure.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (region us-west-2, account 000000000000).
2. Create a CloudWatch Logs log group and a query log config against it to confirm the positive path still works: logs.CreateLogGroup(logGroupName=<lg>); r53r.CreateResolverQueryLogConfig(Name=<unique>, DestinationArn='arn:aws:logs:<region>:<account>:log-group:<lg>:*', CreatorRequestId=<unique>) -> 200, DestinationArn stored without the trailing ':*'.
3. Call r53r.CreateResolverQueryLogConfig(Name=<unique>, DestinationArn='arn:aws:logs:<region>:<account>:log-group:<a-name-never-created>:*', CreatorRequestId=<unique>).
4. Observe HTTP 400 with Error.Code 'AccessDeniedException' (correct) and Error.Message '[RSLVR-01803] Access to resource is denied.' instead of '[RSLVR-01600] Access denied' (ignoring the per-request ' Trace Id: "..."' suffix).
5. Confirm the rest of the behavior is already right: ListResolverQueryLogConfigs(Filters=[{Name:'Name',Values:[<the refused name>]}]) returns an empty list with TotalFilteredCount 0, and the control config is still the only match for its own name in status CREATED.
6. Replay behavioral check route53resolver.resolver-query-log-config-bad-destination-rejected to confirm the fix.
