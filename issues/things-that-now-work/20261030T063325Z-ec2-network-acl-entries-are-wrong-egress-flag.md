Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-network-acl-create-configure-entries-delete
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-network-acl-create-configure-entries-delete ec2-network-acl-entries-lifecycle ec2-network-acl-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateNetworkAcl|2xx->2xx|$.CreateNetworkAclResponse.networkAcl.entrySet.item[*].egress
Issue-Key: ec2|CreateNetworkAcl|2xx->2xx|$.CreateNetworkAclResponse.networkAcl.entrySet.item[*].icmpTypeCode
Issue-Key: ec2|CreateNetworkAcl|2xx->2xx|$.CreateNetworkAclResponse.networkAcl.entrySet.item[*].portRange
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.entrySet.item[*].egress
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.entrySet.item[*].portRange
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.entrySet.item[*].protocol
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.entrySet.item[*].ruleAction
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.entrySet.item[*].ruleNumber
Issue-Related: 20261029T215142Z-ec2-lifecycle-state-never-becomes-observable-after-mutations.md
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# EC2 network ACL entries are wrong: egress flag inverted, icmpTypeCode and portRange omitted, added entries invisible

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkAcl

## Description
Three net-new wire divergences share one root cause: the emulator's representation of a network ACL's entry list does not match AWS - the two default rules are emitted in the wrong egress orientation and without their empty icmpTypeCode/portRange members, and entries added by CreateNetworkAclEntry never appear in subsequent reads. This issue bundles 3 causes; all 3 must be closed.

Cause 1 of 3: CreateNetworkAcl's default entrySet.
AWS: Smithy: NetworkAclEntry has members CidrBlock (cidrBlock), Egress (egress, boolean), IcmpTypeCode (icmpTypeCode), Ipv6CidrBlock, PortRange (portRange), Protocol (protocol), RuleAction (ruleAction), RuleNumber (ruleNumber). The recording shows a new ACL carrying two rule-32767 deny-all entries, the first with egress 'true' and the second with egress 'false', and each entry serializing icmpTypeCode and portRange as present-but-empty elements.
Emulator: entry[0].egress 'false' and entry[1].egress 'true' (the pair is emitted in the opposite order), and icmpTypeCode and portRange are omitted from both entries.
Why: a client that reads entrySet[0] to find the egress default rule configures the wrong direction, and XML/struct deserializers that expect the icmpTypeCode/portRange elements to exist see absent members instead of empty ones.
Fix: the CreateNetworkAcl handler's default-entry construction plus the NetworkAclEntry serializer (empty icmpTypeCode/portRange must be emitted).
Traces: ec2-network-acl-create-configure-entries-delete#setup#2 (same cluster: ec2-network-acl-entries-lifecycle#task#2, ec2-network-acl-lifecycle#task#2).

Cause 2 of 3: DescribeNetworkAcls after CreateNetworkAclEntry - added entries are missing and entry[0] is the wrong rule.
AWS: recorded behavior only for ordering: the recording shows the entry list containing, in order, the ingress/egress rules the client added (ruleNumber 100, protocol 6, ruleAction 'allow', with portRange {from 80, to 80}) followed by the rule-32767 deny-all defaults for each direction - four entries in total for the recorded ACL.
Emulator: entry[0] is the default deny (ruleNumber 32767, protocol -1, ruleAction 'deny', egress 'false') with no portRange, and entries [2] and [3] are absent entirely - the added rules were never recorded in state or never projected.
Why: a client that adds an ACL rule and reads the ACL back cannot confirm its own rule exists, and any convergence loop on the entry list never terminates.
Fix: the CreateNetworkAclEntry state mutation and the DescribeNetworkAcls entry projection/ordering.
Traces: ec2-network-acl-create-configure-entries-delete#task#5 (same cluster: #task#7).

Cause 3 of 3: DescribeNetworkAcls in the ACL lifecycle fixture shows the same missing/misordered entries on a second fixture path.
AWS: recorded behavior only - entry[0] ruleNumber '100', ruleAction 'allow', egress 'true'; entries [2] (ruleNumber 100, protocol 6, allow, egress false, portRange 80-80) and [3] (ruleNumber 32767, protocol -1, deny, egress false) also present.
Emulator: entry[0] is ruleNumber '32767', ruleAction 'deny', egress 'false'; entries [2] and [3] missing.
Why: as cause 2 - the ACL's rule table is not observable, so no client can verify its own configuration.
Fix: same components as cause 2 (entry state + DescribeNetworkAcls projection).
Traces: ec2-network-acl-lifecycle#task#5.

## Evidence
Cause 1: $.CreateNetworkAclResponse.networkAcl.entrySet.item[0].egress expected 'true', got 'false'; item[1].egress expected 'false', got 'true'; item[0]/item[1].icmpTypeCode and .portRange expected '' (present, empty), got None.
Cause 2: $.DescribeNetworkAclsResponse.networkAclSet.item.entrySet.item[0].ruleNumber expected '100' got '32767'; .protocol expected '6' got '-1'; .ruleAction expected 'allow' got 'deny'; .egress expected 'true' got 'false'; .portRange expected {'from': '80', 'to': '80'} got None; item[2] and item[3] expected present, got None.
Cause 3: same key set on ec2-network-acl-lifecycle#task#5 (item[0].ruleNumber 100 vs 32767, ruleAction allow vs deny, egress true vs false; item[2], item[3] missing).

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-network-acl-create-configure-entries-delete#setup#2 (default entries), then #task#5, then ec2-network-acl-lifecycle#task#5 (entries after CreateNetworkAclEntry). Compare entrySet ordering, the egress flag, the presence of empty icmpTypeCode/portRange elements and the presence of the client-added rules against the recorded responses.
