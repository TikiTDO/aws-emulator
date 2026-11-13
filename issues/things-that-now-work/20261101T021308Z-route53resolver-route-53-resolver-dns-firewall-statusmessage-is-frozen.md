Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-dns-firewall-domain-list-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-dns-firewall-domain-list-lifecycle route53resolver-dns-firewall-rule-group-and-rules
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|GetFirewallDomainList|2xx->2xx|$.FirewallDomainList.StatusMessage
Issue-Key: route53resolver|UpdateFirewallDomains|2xx->2xx|$.StatusMessage
Issue-Key: route53resolver|GetFirewallRuleGroup|2xx->2xx|$.FirewallRuleGroup.StatusMessage
Issue-Related: 20261029T234450Z-route53resolver-route-53-resolver-dns-firewall-state-managed-catalogs.md
Issue-Commit: ce99e6c7a93acaaf8abb6a4a732944b33ffd4935

# Route 53 Resolver DNS Firewall StatusMessage is frozen at the creation message and a failed import never leaves COMPLETE (4 causes)

> _Note: this issue was identified against the code base at commit `ce99e6c7a93acaaf8abb6a4a732944b33ffd4935`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFirewallDomainList

## Description
Four net-new wire causes in services/service-com-amazonaws-route53resolver-route53resolver, all one shared fault: a DNS Firewall domain list / rule group gets its Status and StatusMessage written ONCE at creation and they are never recomputed by any later mutating operation. Every mutation path (UpdateFirewallDomains, ImportFirewallDomains, rule add/update inside a rule group) therefore reads back the creation message, and a failed import never reaches its terminal failure state.

NOTE on prior issues: the resolved issue 20261029T234450Z-route53resolver-route-53-resolver-dns-firewall-projections-omit-statusmessage.md made these projections emit a StatusMessage at all; the constant it now emits is what diverges here, so that resolved issue does not cover these causes. The open issue 20261029T234450Z-route53resolver-route-53-resolver-dns-firewall-state-managed-catalogs.md describes the failed-import outcome in its cause 4, but its replay key is GetFirewallDomainList|2xx->2xx|$.FirewallDomainList.Status and the cluster now also diverges on $.FirewallDomainList.StatusMessage, so the replay reports this cluster as unowned (owned_by empty) and it is re-filed here with a key that binds. Open issue 20261025T012620Z-route53resolver-importfirewalldomains-stub.md tracks ImportFirewallDomains being a scaffold stub; cause 3 is the status/message outcome, not the stub.

=== Cause 1 of 4: GetFirewallDomainList after UpdateFirewallDomains still reports the creation StatusMessage ===
AWS: Smithy: GetFirewallDomainList(FirewallDomainListId required) -> {FirewallDomainList}; FirewallDomainList.StatusMessage (@length 0..255) is "Additional information about the status of the list, if available." and Status is FirewallDomainListStatus = COMPLETE | COMPLETE_IMPORT_FAILED | IMPORTING | DELETING | UPDATING. The message text is recorded behavior only: after an UpdateFirewallDomains has settled, the list reads back with StatusMessage "Finished domain list update" — fixed text with no variable part (it describes the last operation on the list, not its creation).
Emulator: HTTP 200 with $.FirewallDomainList.StatusMessage = "Created Firewall Domain List" — the creation-time message, unchanged by the ADD and by the later REMOVE (the cluster covers both task#5 and task#8).
Why: a client that polls a domain list to learn whether its last write landed cannot distinguish "created, never written" from "update finished", so bulk blocklist loaders either proceed on an unwritten list or retry forever.
Fix: the domain-list status machine in the crate's state module — every mutating path must restamp Status/StatusMessage for the operation it performed; the GetFirewallDomainList projection must read that stamped value instead of a creation constant.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#5 (cluster also covers #task#8)

=== Cause 2 of 4: UpdateFirewallDomains' own response echoes the creation StatusMessage ===
AWS: Smithy: UpdateFirewallDomains(FirewallDomainListId required, Operation required (FirewallDomainUpdateOperation), Domains required) -> {Id, Name, Status (FirewallDomainListStatus), StatusMessage (@length 0..255)}. Recorded behavior: the update response's Status/StatusMessage describe the update just accepted (the update-class message, the same lifecycle field that settles to "Finished domain list update" per cause 1); the fixture asserts this member's shape rather than one run's text.
Emulator: HTTP 200 with $.StatusMessage = "Created Firewall Domain List" (replay: value_mismatch, expected '<shaped-not-asserted>', got 'Created Firewall Domain List') — the creation constant is served from the same frozen field instead of a message for the update.
Why: the write acknowledgement tells the caller nothing about the write; a client that keys its wait loop on the message returned by UpdateFirewallDomains sees a creation message and treats the update as not yet started.
Fix: the UpdateFirewallDomains handler — it must write the operation's status/message into the domain list and serialize that, not the stored creation text.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#6

=== Cause 3 of 4: a failed ImportFirewallDomains leaves the list COMPLETE with the creation message instead of COMPLETE_IMPORT_FAILED ===
AWS: Smithy: ImportFirewallDomains(FirewallDomainListId required, Operation required, DomainFileUrl required @length 1..1024) -> {Id, Name, Status, StatusMessage}; FirewallDomainListStatus includes the terminal COMPLETE_IMPORT_FAILED. Recorded behavior: when the import cannot fetch the supplied URL, GetFirewallDomainList settles at Status COMPLETE_IMPORT_FAILED with StatusMessage "Failed to download the file from provided url, please try with a smaller file or copy the file into a <region> S3 bucket." — that sentence is fixed text; its only variable part is the Region of the request (us-west-2 in this recording).
Emulator: HTTP 200 with $.FirewallDomainList.Status = "COMPLETE" and $.FirewallDomainList.StatusMessage = "Created Firewall Domain List"; the fixture's poll aborts because COMPLETE is a fail-on state ("poll: 'COMPLETE' is in fail_on after 1 attempt(s)").
Why: a failed import is reported as a successful one, so a client proceeds with an empty domain list and the rules referencing it block nothing — a silent security-relevant failure.
Fix: the ImportFirewallDomains execution path plus the domain-list status machine in the crate's state module — a fetch failure must terminate in COMPLETE_IMPORT_FAILED and stamp the download-failure message with the request's Region.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#10

=== Cause 4 of 4: GetFirewallRuleGroup after a rule mutation still reports the creation StatusMessage ===
AWS: Smithy: GetFirewallRuleGroup(FirewallRuleGroupId required) -> {FirewallRuleGroup}; FirewallRuleGroup.StatusMessage (@length 0..255) is "Additional information about the status of the rule group, if available." and Status is FirewallRuleGroupStatus = COMPLETE | DELETING | UPDATING. Recorded behavior: once the group's rules have been changed and the group has settled, it reads back with StatusMessage "Finished rule group update" — fixed text, no variable part.
Emulator: HTTP 200 with $.FirewallRuleGroup.StatusMessage = "Created Firewall Rule Group", i.e. the same frozen-at-creation defect on the rule-group record.
Why: a client reconciling a rule group cannot tell whether its rule writes have been applied to the group, so it re-applies rules or reports the group as never updated.
Fix: the rule-group status machine in the crate's state module — rule create/update/delete must restamp the owning group's Status/StatusMessage, and the GetFirewallRuleGroup projection must serialize that value.
Traces: route53resolver-dns-firewall-rule-group-and-rules#task#11

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). Four mismatch clusters, none stub-blocked, none owned by an open issue.

Cause 1 — GetFirewallDomainList, route53resolver-dns-firewall-domain-list-lifecycle#task#5, 200 -> 200:
  $.FirewallDomainList.StatusMessage: value_mismatch (expected 'Finished domain list update', got 'Created Firewall Domain List')   [2 affected steps: task#5, task#8]

Cause 2 — UpdateFirewallDomains, route53resolver-dns-firewall-domain-list-lifecycle#task#6, 200 -> 200:
  $.StatusMessage: value_mismatch (expected '<shaped-not-asserted>', got 'Created Firewall Domain List')

Cause 3 — GetFirewallDomainList, route53resolver-dns-firewall-domain-list-lifecycle#task#10, 200 -> 200:
  $.FirewallDomainList.Status: value_mismatch (expected 'COMPLETE_IMPORT_FAILED', got 'COMPLETE')
  $.FirewallDomainList.StatusMessage: value_mismatch (expected 'Failed to download the file from provided url, please try with a smaller file or copy the file into a us-west-2 S3 bucket.', got 'Created Firewall Domain List')
  $.FirewallDomainList.Status: poll: 'COMPLETE' is in fail_on after 1 attempt(s)

Cause 4 — GetFirewallRuleGroup, route53resolver-dns-firewall-rule-group-and-rules#task#11, 200 -> 200:
  $.FirewallRuleGroup.StatusMessage: value_mismatch (expected 'Finished rule group update', got 'Created Firewall Rule Group')

Fixed text vs variable parts: "Finished domain list update", "Finished rule group update" and "Failed to download the file from provided url, please try with a smaller file or copy the file into a <region> S3 bucket." are fixed AWS text; the only variable part anywhere is the Region in the import-failure sentence, taken from the request. The list/group ids, ARNs and timestamps in the recordings are per-run values and are not requirements.

## Steps to Reproduce
Replay fixtures route53resolver-dns-firewall-domain-list-lifecycle (task#5, task#6, task#8, task#10) and route53resolver-dns-firewall-rule-group-and-rules (task#11). Minimal reproductions:
1. CreateFirewallDomainList, then UpdateFirewallDomains(Operation=ADD, Domains=[...]) -> the response's StatusMessage must describe the update, not the creation (cause 2); then GetFirewallDomainList -> StatusMessage must be 'Finished domain list update', not 'Created Firewall Domain List' (cause 1). Repeat with Operation=REMOVE: the settled message must again be the update message.
2. ImportFirewallDomains(FirewallDomainListId=<id>, Operation=REPLACE, DomainFileUrl=<unreachable s3 url>), then poll GetFirewallDomainList -> Status must settle at COMPLETE_IMPORT_FAILED with StatusMessage 'Failed to download the file from provided url, please try with a smaller file or copy the file into a <region> S3 bucket.' (cause 3).
3. CreateFirewallRuleGroup, CreateFirewallRule/UpdateFirewallRule in that group, then GetFirewallRuleGroup -> FirewallRuleGroup.StatusMessage must be 'Finished rule group update', not 'Created Firewall Rule Group' (cause 4).
