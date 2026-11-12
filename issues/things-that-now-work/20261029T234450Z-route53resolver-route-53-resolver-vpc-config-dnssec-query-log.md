Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-vpc-config-and-association-missing-resource-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-vpc-config-and-association-missing-resource-errors route53resolver-query-log-config-missing-resource-errors route53resolver-dns-firewall-rule-group-and-rules
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|GetFirewallConfig|4xx->2xx|$.Message
Issue-Key: route53resolver|GetFirewallConfig|4xx->2xx|$.__type
Issue-Key: route53resolver|GetFirewallConfig|4xx->2xx|status
Issue-Key: route53resolver|UpdateFirewallConfig|4xx->2xx|$.Message
Issue-Key: route53resolver|UpdateFirewallConfig|4xx->2xx|$.__type
Issue-Key: route53resolver|UpdateFirewallConfig|4xx->2xx|status
Issue-Key: route53resolver|GetResolverConfig|4xx->2xx|$.Message
Issue-Key: route53resolver|GetResolverConfig|4xx->2xx|$.__type
Issue-Key: route53resolver|GetResolverConfig|4xx->2xx|status
Issue-Key: route53resolver|UpdateResolverConfig|4xx->2xx|$.Message
Issue-Key: route53resolver|UpdateResolverConfig|4xx->2xx|$.__type
Issue-Key: route53resolver|UpdateResolverConfig|4xx->2xx|status
Issue-Key: route53resolver|GetResolverDnssecConfig|4xx->2xx|$.Message
Issue-Key: route53resolver|GetResolverDnssecConfig|4xx->2xx|$.__type
Issue-Key: route53resolver|GetResolverDnssecConfig|4xx->2xx|status
Issue-Key: route53resolver|UpdateResolverDnssecConfig|4xx->2xx|$.Message
Issue-Key: route53resolver|UpdateResolverDnssecConfig|4xx->2xx|$.__type
Issue-Key: route53resolver|UpdateResolverDnssecConfig|4xx->2xx|status
Issue-Key: route53resolver|CreateResolverQueryLogConfig|4xx->2xx|$.Message
Issue-Key: route53resolver|CreateResolverQueryLogConfig|4xx->2xx|$.__type
Issue-Key: route53resolver|CreateResolverQueryLogConfig|4xx->2xx|status
Issue-Key: route53resolver|DeleteFirewallRuleGroup|4xx->2xx|$.Message
Issue-Key: route53resolver|DeleteFirewallRuleGroup|4xx->2xx|$.__type
Issue-Key: route53resolver|DeleteFirewallRuleGroup|4xx->2xx|status
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# Route 53 Resolver VPC config, DNSSEC, query-log create and rule-group delete answer 200 for requests AWS refuses with 400 (8 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFirewallConfig

## Description
Eight net-new wire causes where the emulator answers HTTP 200 for a request Route 53 Resolver refuses with HTTP 400. The shared fault: the VPC-scoped config operations (firewall config, resolver config, DNSSEC config) never check that the ResourceId names a real VPC, CreateResolverQueryLogConfig never checks its destination, and DeleteFirewallRuleGroup never checks for rules in the group — so every one of these calls fabricates a success. Service route53resolver, crate services/service-com-amazonaws-route53resolver-route53resolver.

=== Cause 1 of 8: GetFirewallConfig answers 200 for a VPC that does not exist ===
AWS: Smithy: GetFirewallConfig(ResourceId required @length 1..64) -> {FirewallConfig}; errors AccessDenied | InternalServiceError | ResourceNotFound | Throttling | ValidationException, all httpError 400 (defaulted). Recorded behavior: a well-formed but nonexistent vpc id is refused HTTP 400 with __type 'ResourceNotFoundException' (Message asserted by shape: an RSLVR-coded can't-find sentence naming the id from the request).
Emulator: HTTP 200 with a fabricated FirewallConfig and no error members at all.
Why: a client that probes whether a VPC has DNS Firewall configured gets a config for a VPC that does not exist, so typo'd or stale vpc ids are never surfaced and the client acts on invented state.
Fix: the VPC-existence precondition in GetFirewallConfig's handler, refusing with ResourceNotFoundException from the shared RSLVR message formatter before projecting a config.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#3

=== Cause 2 of 8: UpdateFirewallConfig answers 200 for a VPC that does not exist ===
AWS: Smithy: UpdateFirewallConfig(ResourceId required, FirewallFailOpen required enum) -> {FirewallConfig}; same 400-only error set including ResourceNotFound. Recorded behavior: HTTP 400 ResourceNotFoundException.
Emulator: HTTP 200, and the write is accepted against a nonexistent VPC.
Why: worse than the read — the emulator persists fail-open configuration keyed to a VPC that does not exist, state AWS makes unreachable.
Fix: the same VPC-existence precondition on the write path.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#4

=== Cause 3 of 8: GetResolverConfig answers 200 for a VPC that does not exist ===
AWS: Smithy: GetResolverConfig(ResourceId required) -> {ResolverConfig}; errors AccessDenied | InternalServiceError | InvalidParameter | ResourceNotFound | Throttling | ValidationException (400). Recorded behavior: HTTP 400, __type 'ResourceNotFoundException', Message '[RSLVR-00913] Can\u2019t find the resource with ID "<ResourceId>".' — fixed text with the RSLVR-00913 prefix, the typographic apostrophe, and one variable part: the id taken from the request.
Emulator: HTTP 200 with a synthesized ResolverConfig.
Why: as cause 1; additionally the exact RSLVR-00913 sentence is the string clients key on, and it is never produced.
Fix: the VPC-existence precondition in GetResolverConfig's handler plus the RSLVR-00913 message from the shared formatter.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#5

=== Cause 4 of 8: UpdateResolverConfig answers 200 for a VPC that does not exist ===
AWS: Smithy: UpdateResolverConfig(ResourceId required, AutodefinedReverseFlag required enum) -> {ResolverConfig}; errors include ResourceNotFound, InvalidParameter, InvalidRequest, ResourceUnavailable (all 400). Recorded behavior: HTTP 400 ResourceNotFoundException with the same '[RSLVR-00913] Can\u2019t find the resource with ID "<ResourceId>".' sentence.
Emulator: HTTP 200, accepting the autodefined-reverse change for a VPC that does not exist.
Why: as cause 2 — invented per-VPC resolver configuration is persisted and reported.
Fix: the same precondition on UpdateResolverConfig.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#6

=== Cause 5 of 8: GetResolverDnssecConfig answers 200 where AWS refuses with AccessDeniedException ===
AWS: Smithy: GetResolverDnssecConfig(ResourceId required) -> {ResolverDNSSECConfig}; errors AccessDenied | InternalServiceError | InvalidParameter | InvalidRequest | ResourceNotFound | Throttling (400). AccessDeniedException is documented "The current account doesn't have the IAM permissions required to perform the specified Resolver operation." Recorded behavior: for a vpc id the caller cannot see, the DNSSEC read is refused HTTP 400 with __type 'AccessDeniedException' (Message asserted by shape) — the service does not disclose whether the VPC exists.
Emulator: HTTP 200 with a fabricated DNSSEC config and no error.
Why: the client's permission-error path never runs, and the emulator answers for VPCs the caller has no access to — the opposite of the service's deliberate non-disclosure.
Fix: the authorization/existence check in GetResolverDnssecConfig's handler, mapping an unreachable ResourceId to AccessDeniedException.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#7

=== Cause 6 of 8: UpdateResolverDnssecConfig answers 200 where AWS refuses with AccessDeniedException ===
AWS: Smithy: UpdateResolverDnssecConfig(ResourceId required, Validation required enum) -> {ResolverDNSSECConfig}; same error set as cause 5. Recorded behavior: HTTP 400 AccessDeniedException.
Emulator: HTTP 200, enabling/disabling DNSSEC validation for an unreachable VPC.
Why: a client believes it changed DNSSEC validation on a VPC it cannot touch; the change exists only in the emulator.
Fix: the same authorization check on the write path.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#8

=== Cause 7 of 8: CreateResolverQueryLogConfig answers 200 for a destination the caller cannot write to ===
AWS: Smithy: CreateResolverQueryLogConfig(Name required @pattern, DestinationArn required @length 1..600, CreatorRequestId required, Tags) -> {ResolverQueryLogConfig}; errors AccessDenied | InternalServiceError | InvalidParameter | InvalidRequest | LimitExceeded | ResourceExists | ResourceNotFound | Throttling (all 400). Recorded behavior: when the DestinationArn names a log destination that does not exist / cannot be written to, the create is refused HTTP 400 with __type 'AccessDeniedException' (Message asserted by shape) and no config is created.
Emulator: HTTP 200 — the query-log config is created against the missing destination, with no error members on the body.
Why: a client believes query logging is configured and delivering, while no destination exists; the misconfiguration is only discovered in production, and the emulator's state contains a config AWS would never have created.
Fix: the destination-validation step in CreateResolverQueryLogConfig's handler, refusing with AccessDeniedException before the config is written to the crate's state module.
Traces: route53resolver-query-log-config-missing-resource-errors#task#3

=== Cause 8 of 8: DeleteFirewallRuleGroup deletes a group that still has a rule, instead of ConflictException RSLVR-02103 ===
AWS: Smithy: DeleteFirewallRuleGroup(FirewallRuleGroupId required) -> {FirewallRuleGroup}; errors AccessDenied | Conflict | InternalServiceError | ResourceNotFound | Throttling | ValidationException (all 400); ConflictException is documented "The requested state transition isn't valid...". Recorded behavior: while the group still contains a FirewallRule, the delete is refused HTTP 400, __type 'ConflictException', Message '[RSLVR-02103] The DNS Firewall rule group has an associated rule: "<FirewallRuleGroupId>".  Delete the rule and try again later. Trace Id: "<trace Root>"' — fixed text (note the two spaces after the first sentence), one variable id from the request plus the per-request trace Root.
Emulator: HTTP 200 with no error members — the group is deleted out from under its rules.
Why: the rules are orphaned, a state AWS makes unreachable, and teardown/IaC code that relies on this conflict to order its deletes silently corrupts its own configuration here and then breaks against real AWS.
Fix: the in-use precondition in DeleteFirewallRuleGroup's handler must consult the rule index in the crate's state module and refuse with the RSLVR-02103 ConflictException before mutating state.
Traces: route53resolver-dns-firewall-rule-group-and-rules#task#19

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). Eight mismatch clusters, none stub-blocked, none owned.

Cause 1 — GetFirewallConfig, route53resolver-vpc-config-and-association-missing-resource-errors#task#3:
  status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException', got None); $.Message: missing
Cause 2 — UpdateFirewallConfig, ...#task#4: status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException')
Cause 3 — GetResolverConfig, ...#task#5: status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException'); $.Message: missing (expected '[RSLVR-00913] Can\u2019t find the resource with ID "vpc-0123456789abcdef0".')
Cause 4 — UpdateResolverConfig, ...#task#6: status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException'); $.Message: missing (expected '[RSLVR-00913] Can\u2019t find the resource with ID "vpc-0123456789abcdef0".')
Cause 5 — GetResolverDnssecConfig, ...#task#7: status: expected 400, got 200; $.__type: missing (expected 'AccessDeniedException')
Cause 6 — UpdateResolverDnssecConfig, ...#task#8: status: expected 400, got 200; $.__type: missing (expected 'AccessDeniedException')
Cause 7 — CreateResolverQueryLogConfig, route53resolver-query-log-config-missing-resource-errors#task#3: status: expected 400, got 200; $.__type: missing (expected 'AccessDeniedException')
Cause 8 — DeleteFirewallRuleGroup, route53resolver-dns-firewall-rule-group-and-rules#task#19: status: expected 400, got 200; $.__type: missing (expected 'ConflictException'); $.Message: missing (expected '[RSLVR-02103] The DNS Firewall rule group has an associated rule: "rslvr-frg-910a2dec89025cc1".  Delete the rule and try again later. Trace Id: "1-6a76e073-7375192c5bab22b5782fa6c7"' — the trace Root is per-request, the sentence and clause are the requirement)

## Steps to Reproduce
Replay fixtures route53resolver-vpc-config-and-association-missing-resource-errors (task#3..task#8), route53resolver-query-log-config-missing-resource-errors (task#3) and route53resolver-dns-firewall-rule-group-and-rules (task#19). Minimal reproductions, all with a well-formed id that was never created:
1/2. GetFirewallConfig(ResourceId='vpc-0123456789abcdef0') and UpdateFirewallConfig(ResourceId=same, FirewallFailOpen=ENABLED) -> both must be 400 ResourceNotFoundException.
3/4. GetResolverConfig / UpdateResolverConfig(AutodefinedReverseFlag=DISABLE) on the same id -> 400 ResourceNotFoundException with the RSLVR-00913 sentence naming the id.
5/6. GetResolverDnssecConfig / UpdateResolverDnssecConfig(Validation=ENABLE) on the same id -> 400 AccessDeniedException.
7. CreateResolverQueryLogConfig(Name=<n>, CreatorRequestId=<id>, DestinationArn=<arn of a log group/bucket that does not exist>) -> 400 AccessDeniedException, and no config created.
8. CreateFirewallRuleGroup + CreateFirewallDomainList + CreateFirewallRule in the group, then DeleteFirewallRuleGroup(FirewallRuleGroupId=<group>) -> must be 400 ConflictException with the RSLVR-02103 sentence, not 200.
