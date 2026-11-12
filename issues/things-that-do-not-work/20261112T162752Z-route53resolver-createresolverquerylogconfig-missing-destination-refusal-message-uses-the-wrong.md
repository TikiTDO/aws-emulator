Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-query-log-config-missing-resource-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-query-log-config-missing-resource-errors
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateResolverQueryLogConfig|4xx->4xx|$.Message
Issue-Commit: 116dfabd29b018288b31e7394f72aeff79ed3e64

# CreateResolverQueryLogConfig missing-destination refusal Message uses the wrong RSLVR code and invented wording

> _Note: this issue was identified against the code base at commit `116dfabd29b018288b31e7394f72aeff79ed3e64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolverQueryLogConfig

## Description
ONE net-new wire cause in service route53resolver (crate services/service-com-amazonaws-route53resolver-route53resolver). The emulator now refuses CreateResolverQueryLogConfig for a destination that does not exist with the right HTTP status and the right modeled error class, but the Error.Message it serializes is not the message AWS produces for that refusal class: it carries a different bracketed RSLVR code and emulator-invented prose.

Cause 1 of 1: CreateResolverQueryLogConfig's access-denied refusal for a non-existent destination emits '[RSLVR-01803] Access to resource is denied.' instead of the service's '[RSLVR-01600] Access denied' sentence.
AWS: Smithy: CreateResolverQueryLogConfig(Name: ResolverQueryLogConfigName @length 1..64 @pattern "^(?!^[0-9]+$)([a-zA-Z0-9\\-_' ']+)$" required, DestinationArn: DestinationArn @length 1..600 required, CreatorRequestId @idempotencyToken required, Tags) -> {ResolverQueryLogConfig}; modeled errors AccessDeniedException, InvalidRequestException, InvalidParameterException, ResourceExistsException, ResourceNotFoundException, LimitExceededException, ThrottlingException, InternalServiceErrorException, all at HTTP 400 (httpError defaulted). DestinationArn is documented as the ARN of an S3 bucket, a CloudWatch Logs log group or a Firehose delivery stream; the model constrains only its length and states no message text, so the wording is recorded behavior only. The recording for this step is a well-formed CloudWatch Logs log-group ARN for a log group that was never created, and the service answers HTTP 400 with class AccessDeniedException and Message '[RSLVR-01600] Access denied' — that sentence is ENTIRELY fixed text: the bracketed code '[RSLVR-01600] ' plus the words 'Access denied'. It interpolates no part of the request (no Name, no DestinationArn, no CreatorRequestId) and no state. Route 53 Resolver appends its per-request ' Trace Id: "<Root of X-Amzn-Trace-Id>"' clause to refusals; the Root itself is a per-run value and is never a requirement, only the clause's framing is. The same fixed literal is the one the open behavioral issue 20261027T062807Z-route53resolver-5-net-new-causes-query-log-config.md records for this exact path.
Emulator: HTTP 400 (correct) with the recorded error class (the step reports no $.__type divergence) but Message '[RSLVR-01803] Access to resource is denied. Trace Id: "1-6af5e74b-c9c6453d971571039bcffdef"' — a DIFFERENT RSLVR code (01803 where the service uses 01600) and a different sentence ('Access to resource is denied.' with a terminating period, where the service says 'Access denied'). The corpus asserts this Message by shape ('<shaped-not-asserted>') and the emulator's value fails that shape. The trace Root quoted here is this run's value and is not itself the defect.
Why: every Route 53 Resolver fault is HTTP 400, so the modeled error class plus the bracketed [RSLVR-nnnnn] code inside Error.Message are the only discriminators a client has. A caller, log pipeline or runbook that matches '[RSLVR-01600]' to recognise 'Resolver cannot read the destination you named' never matches against the emulator, and a caller that keys on the full message text diverges on every field of it; an operator correlating the refusal against AWS's documented Resolver error codes is pointed at an unrelated code.
Fix: the destination-validation branch of the CreateResolverQueryLogConfig handler in that crate must build its AccessDeniedException body through the crate's shared RSLVR message formatter with the RSLVR-01600 constant and the fixed sentence 'Access denied', keeping the trailing trace clause the formatter already appends, instead of formatting a local RSLVR-01803 string. The HTTP status, the error class and the write-nothing behaviour of the branch are already correct and must not change. Related but not this: the open behavioral issue 20261027T062807Z-route53resolver-5-net-new-causes-query-log-config.md (Cause 3) describes the earlier failure mode where this call was ACCEPTED with HTTP 200 — the emulator now refuses it, so what remains is only the message text, and that behavioral issue carries no wire fixture step; the fixed issue 20261029T234450Z-route53resolver-route-53-resolver-vpc-config-dnssec-query-log.md corrected the 200->400 status on this path but not the message. The open issue 20261029T234450Z-route53resolver-route-53-resolver-not-found-refusal-messages-fail.md covers the MISSING trace clause on other Resolver refusals; this message has the clause and diverges in its code and sentence instead, so it is not covered there.
Traces: route53resolver-query-log-config-missing-resource-errors#task#3

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped; the replay reports pending=[]). One mismatch cluster, status 400 -> 400, stub_path empty, owned_by empty (every other divergence in this replay is already owned by an open issue or stub-blocked).

Fixture route53resolver-query-log-config-missing-resource-errors, step task#3, operation CreateResolverQueryLogConfig:
  expected $.Message: asserted by shape ('<shaped-not-asserted>'); the recorded service literal for this refusal is '[RSLVR-01600] Access denied' (+ the service's per-request ' Trace Id: "<Root>"' clause)
  actual   $.Message: '[RSLVR-01803] Access to resource is denied. Trace Id: "1-6af5e74b-c9c6453d971571039bcffdef"'
  status: 400 expected, 400 actual (correct); $.__type did not diverge, so the modeled error class is already the recorded one.

Fixed text vs values: the whole message is fixed text here — '[RSLVR-01600] ' + 'Access denied'. Nothing in it comes from the request or from emulator state. The only per-run part of the wire value is the trace Root inside the ' Trace Id: "..."' clause, which is NOT part of the requirement; the divergence is the code digits (01803 vs 01600) and the sentence ('Access to resource is denied.' vs 'Access denied').

For contrast, other RSLVR literals recorded in this same corpus show the same code+sentence+trace-clause shape, e.g. '[RSLVR-02035] The specified IAM policy isn't valid: "...". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account. Trace Id: "..."' — so the formatter exists and only this branch's code/sentence are wrong.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (region us-west-2, account 000000000000). Send X-Amzn-Trace-Id: Root=<root>;Sampled=0 from the client.
2. Replay fixture route53resolver-query-log-config-missing-resource-errors and inspect step task#3.
3. Minimal reproduction: call route53resolver.CreateResolverQueryLogConfig(Name=<unique name matching the modeled pattern>, DestinationArn='arn:aws:logs:us-west-2:000000000000:log-group:<a-log-group-that-was-never-created>:*', CreatorRequestId=<unique>).
4. Observe HTTP 400 with the recorded error class (correct) and Error.Message '[RSLVR-01803] Access to resource is denied. Trace Id: "<root>"'. Required: Error.Message '[RSLVR-01600] Access denied' with the service's trailing ' Trace Id: "<root>"' clause — the bracketed code and the sentence are fixed text and must match exactly; only the trace Root varies per request.
5. Confirm no configuration was stored (ListResolverQueryLogConfigs filtered on that Name returns an empty list, TotalFilteredCount 0) — that part of the behaviour is already correct.
