Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-policy-and-logging-reads-on-wrong-references
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-policy-and-logging-reads-on-wrong-references network-firewall-nfwr-traj-rule-group-lookup-needs-type network-firewall-nfwr-traj-stateless-default-action-spelling
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|DescribeFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.Tags
Issue-Key: network-firewall|DescribeRuleGroup|2xx->2xx|$.RuleGroupResponse.Tags
Issue-Key: network-firewall|DescribeFirewallPolicy|2xx->2xx|$.FirewallPolicy.StatelessDefaultActions[*]
Issue-Key: network-firewall|DescribeFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.FirewallPolicyArn
Issue-Commit: c1ca043e5c77dd90c2a96695b3dfa06a20838f7d

# Describe responses omit the empty Tags list and echo an unnormalized stateless default action

> _Note: this issue was identified against the code base at commit `c1ca043e5c77dd90c2a96695b3dfa06a20838f7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFirewallPolicy

## Description
Successful Describe responses are shaped differently from AWS: the Tags member of the *Response metadata structure is dropped when the resource has no tags, and the stateless default action a policy was created with is returned in the caller's casing. This bundle has 3 causes.

Cause 1 of 3 - DescribeFirewallPolicy omits FirewallPolicyResponse.Tags (fixture network-firewall-nfwr-traj-policy-and-logging-reads-on-wrong-references, step task#8).
AWS: FirewallPolicyResponse.Tags targets TagList (Smithy: FirewallPolicyResponse.Tags -> TagList, length 1..200, documented as "The key:value pairs to associate with the resource"). The recording shows AWS serializing Tags as an empty array for an untagged policy rather than omitting the member, on an HTTP 200 whose other members all match.
Emulator: the Tags key is absent from FirewallPolicyResponse.

Cause 2 of 3 - DescribeRuleGroup omits RuleGroupResponse.Tags (fixture network-firewall-nfwr-traj-rule-group-lookup-needs-type, step task#4; same cluster: task#5).
AWS: RuleGroupResponse.Tags targets the same TagList (Smithy: RuleGroupResponse.Tags). Serialized as an empty array for an untagged rule group on HTTP 200.
Emulator: the Tags key is absent from RuleGroupResponse.

Cause 3 of 3 - DescribeFirewallPolicy returns the stateless default action in request casing (fixture network-firewall-nfwr-traj-stateless-default-action-spelling, step task#8).
AWS: FirewallPolicy.StatelessDefaultActions is a list of action strings (Smithy: StatelessActions, list of string - the accepted values are not an enum in the model, so this is recorded behavior). The described policy reports the canonical action name 'AWS:FORWARD_TO_SFE', upper-case with the 'AWS:' prefix, regardless of the spelling the create request used; Tags is likewise an empty array here.
Emulator: returns 'forward_to_sfe' - the string as the create request spelled it, unvalidated and unnormalized - and again omits Tags. (The same step also shows FirewallPolicyResponse.FirewallPolicyArn differing from the ARN the fixture captured earlier; that is a consequence of which create succeeded in this fixture - see the CreateFirewallPolicy validation issue - and the generated ARN value itself is not a requirement.)

Why: a client that reads response.FirewallPolicyResponse.Tags or RuleGroupResponse.Tags gets a null/None where AWS gives an empty list, so tag-reconciliation code that iterates the list without a null guard panics or takes a "never tagged" branch against the emulator only. For Cause 3, a client that compares its desired stateless default action against the described policy sees a mismatch it can never converge - it writes the canonical name, reads back something else - and the emulator's acceptance of a non-canonical spelling means the policy it stores is not one AWS would store.

Fix: the response serializers for DescribeFirewallPolicy and DescribeRuleGroup in the network-firewall service crate (always emit Tags, as an empty list when the resource has none), and the CreateFirewallPolicy/DescribeFirewallPolicy handling of StatelessDefaultActions so the stored/returned action is the canonical 'AWS:'-prefixed upper-case form.

## Evidence
policy-and-logging-reads-on-wrong-references#task#8: HTTP 200 both sides; $.FirewallPolicyResponse.Tags expected [], got None.
rule-group-lookup-needs-type#task#4 (and task#5): HTTP 200 both sides; $.RuleGroupResponse.Tags expected [], got None.
stateless-default-action-spelling#task#8: HTTP 200 both sides; $.FirewallPolicy.StatelessDefaultActions[0] expected 'AWS:FORWARD_TO_SFE', got 'forward_to_sfe'; $.FirewallPolicyResponse.Tags expected [], got None.

## Steps to Reproduce
Replay the listed fixture steps. Equivalently: create an untagged firewall policy and an untagged rule group, then DescribeFirewallPolicy and DescribeRuleGroup and inspect the Tags member of FirewallPolicyResponse / RuleGroupResponse; and create a policy whose StatelessDefaultActions entry is spelled 'forward_to_sfe', then DescribeFirewallPolicy and inspect FirewallPolicy.StatelessDefaultActions[0].
