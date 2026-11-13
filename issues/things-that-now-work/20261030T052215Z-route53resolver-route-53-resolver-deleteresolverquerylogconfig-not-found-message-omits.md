Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-query-log-config-missing-resource-errors
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-query-log-config-missing-resource-errors
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|DeleteResolverQueryLogConfig|4xx->4xx|$.Message
Issue-Commit: bd260fbb92f697f6565f73ef409cec21a7b2614a

# Route 53 Resolver DeleteResolverQueryLogConfig not-found Message omits the trailing Trace Id clause

> _Note: this issue was identified against the code base at commit `bd260fbb92f697f6565f73ef409cec21a7b2614a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResolverQueryLogConfig

## Description
One net-new wire cause in service route53resolver (crate services/service-com-amazonaws-route53resolver-route53resolver). The emulator returns the correct HTTP status and error class for a delete of a query logging configuration that does not exist, but the refusal Message does not conform to the recorded Resolver message shape: it stops at the sentence's final period and never carries the trailing ' Trace Id: "<Root>"' clause that every recorded Resolver refusal in this corpus ends with.

Cause 1 of 1: DeleteResolverQueryLogConfig's RSLVR-01601 not-found Message has no Trace Id clause.
AWS: Smithy: DeleteResolverQueryLogConfig(ResolverQueryLogConfigId required, target ResourceId @length 1..64) -> {ResolverQueryLogConfig}; modeled errors AccessDeniedException, InternalServiceErrorException, InvalidParameterException, InvalidRequestException, ResourceNotFoundException, ThrottlingException, all httpError 400. The error shapes model only a free-text Message, so the wording is recorded behavior only. The corpus asserts this step's Message by shape ('<shaped-not-asserted>') at status 400, and other recorded literals in the same corpus show the service's shape explicitly, e.g. '[RSLVR-02035] The specified IAM policy isn't valid: "<policy>". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account. Trace Id: "<Root>"' (PutFirewallRuleGroupPolicy) and '[RSLVR-02103] The DNS Firewall rule group has an associated rule: "<id>".  Delete the rule and try again later. Trace Id: "<Root>"' (DeleteFirewallRuleGroup). So a Resolver refusal Message is: '[RSLVR-nnnnn] ' + the service's fixed sentence for that refusal class (ids/values interpolated from the request and from state) + ' Trace Id: "<Root of the request's X-Amzn-Trace-Id>"'. Fixed text here: '[RSLVR-01601] The specified query logging configuration doesn't exist.' plus the ' Trace Id: "..."' framing; the only variable part is the Root, which is per-request and never a literal requirement — the PRESENCE of the clause, its exact '. Trace Id: "' framing and the RSLVR code are what is required.
Emulator: status 400 with Message "[RSLVR-01601] The specified query logging configuration doesn't exist." — correct code, correct sentence, message ends at the period with no trace clause, so the shaped assertion on $.Message fails.
Why: a client or support workflow that extracts the trace Root from a refusal to correlate it with the request it came from has nothing to parse on this delete path, and exact-message assertions against AWS fail; a teardown that distinguishes refusal classes by the full recorded message text mis-classifies the response.
Fix: the crate's shared RSLVR refusal formatter / error serializer — the delete-query-log-config handler's not-found path must go through the one service-wide formatter that appends the trace clause from the request's X-Amzn-Trace-Id, rather than formatting its sentence locally.
Traces: route53resolver-query-log-config-missing-resource-errors#task#8.

Related open issues that do NOT cover this cause: 20261029T234450Z-route53resolver-route-53-resolver-not-found-refusal-messages-fail.md records the same omission for GetResolverQueryLogConfig, GetResolverQueryLogConfigAssociation, DisassociateResolverQueryLogConfig and the firewall association reads, and 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing.md names the trailing-clause omission generally — neither carries this operation's divergence key or this fixture step's replay key, so this cause is filed here and may be closed by the same formatter fix.

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). One mismatch cluster, not stub-blocked and not owned by any open issue (owned_by empty).

Cause 1 — DeleteResolverQueryLogConfig, fixture route53resolver-query-log-config-missing-resource-errors step task#8, expected status 400, actual status 400:
  $.Message: value_mismatch (expected '<shaped-not-asserted>', got "[RSLVR-01601] The specified query logging configuration doesn't exist.")
Affected steps in the cluster: route53resolver-query-log-config-missing-resource-errors#task#8 (1).

Required shape, from recorded literals elsewhere in the same corpus: '[RSLVR-02035] ... shared with another AWS account. Trace Id: "1-6a76e074-570e69780dc8a349626ecf87"' and '[RSLVR-02103] ... Delete the rule and try again later. Trace Id: "1-6a76e073-7375192c5bab22b5782fa6c7"'. The Root inside the clause is a per-run value and is not itself a requirement; the clause, its framing and the RSLVR code are. Sibling query-log paths in the same fixture (task#4, task#6, task#7) show the identical omission and are already owned by 20261029T234450Z-route53resolver-route-53-resolver-not-found-refusal-messages-fail.md; this operation's key is absent from it.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a Route 53 Resolver client at its data listener, sending header X-Amzn-Trace-Id: Root=<root>;Sampled=0.
2. Call DeleteResolverQueryLogConfig(ResolverQueryLogConfigId='rqlc-0123456789abcdef0') — a well-formed query-log-config id that was never created.
3. Observe HTTP 400 with Message "[RSLVR-01601] The specified query logging configuration doesn't exist." and no trailing trace clause.
4. Expected: the Message carries the RSLVR code for that refusal class and ends with ' Trace Id: "<root>"' taken from the request's X-Amzn-Trace-Id.
5. Full replay: replay fixture route53resolver-query-log-config-missing-resource-errors and check step task#8.
