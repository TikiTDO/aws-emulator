Issue-Source: trace-sweep-divergence
Issue-Trace: backup-gateway-tag-resource-lifecycle-round-trip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-gateway-tag-resource-lifecycle-round-trip backup-gateway-untag-nonexistent-key-noop
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway|ListTagsForResource|2xx->2xx|$.Tags[*].Value
Issue-Commit: 11c4079474be1ad931c1a1cc92224a1a5bcf318e

# ListTagsForResource renders the aws:createdDate system tag value in a shape the service does not emit

> _Note: this issue was identified against the code base at commit `11c4079474be1ad931c1a1cc92224a1a5bcf318e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
One cause.

Cause 1 of 1: the value this crate renders for the AWS-assigned creation-date tag of a tagged resource is not the form the service reports, so every successful ListTagsForResource of a resource this API created carries one malformed tag value. Fixture backup-gateway-tag-resource-lifecycle-round-trip step task#4 (cluster also covers that fixture's task#6 and backup-gateway-untag-nonexistent-key-noop task#5).

AWS: ListTagsForResource "Lists the tags applied to the resource identified by its Amazon Resource Name (ARN)" and answers 200 with ResourceArn + Tags. Smithy (services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/smithy-model/com-amazonaws-backupgateway-backuponpremises-v20210101.json): Tags is a list of Tag; Tag$Key targets TagKey (@length 1..128, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$, doc "The key can't start with aws:"), Tag$Value targets TagValue (@length 0..256, @pattern ^[^\x00]*$). The model says NOTHING about AWS-assigned system tags or about how their values are spelled, so for this member it is recorded behavior only: the recorded wire of a tag read of a resource this API created carries a second tag whose VALUE is per-run variable and is therefore asserted by SHAPE in the fixture ('<shaped-not-asserted>') rather than by literal, and that shape is what the service's rendering of a creation instant satisfies.

Emulator: the tag list is not read straight off the stored tag map. src/state.rs renders it through reported_tags(), which chains system_tag_list() ahead of the client's tags, minting two synthesized pairs 'aws:createdBy' and 'aws:createdDate'; the second one's value comes from created_date(), which — by the crate's own stated choice, "truncated to WHOLE SECONDS — the granularity a creation date is reported at" — converts the record's creation instant with aws_smithy_types::DateTime::from_secs(..) and formats it with Format::DateTime. Replay reports, on three steps of two fixtures, status 200 on both sides and exactly one diverging path: "$.Tags[1].Value: value_mismatch (expected '<shaped-not-asserted>', got '2026-10-30T21:47:26Z')". Every other asserted path of the same response matches — the list length, the ordering, Tags[1].Key and the shape-asserted Tags[0].Value (aws:createdBy) all pass — so the defect is isolated to how this one value is SPELLED, not to which tags are reported or in what order.

This is not a per-run clock difference being reported as a defect: the fixture does not assert the instant at all (that is what '<shaped-not-asserted>' means), so a merely different clock reading would have passed. What fails is the SHAPE of the rendered string, i.e. the whole-second RFC-3339 'YYYY-MM-DDTHH:MM:SSZ' form this crate chose does not match the form the recording carries for that value. '2026-10-30T21:47:26Z' is the run's own clock filling the template and is not itself the requirement; the requirement is the rendering rule, whose fixed part is the layout/precision of the date-time and whose variable part is the record's own creation instant taken from state.

Why: a client that reads the creation-date tag of a gateway, hypervisor or virtual machine gets a value it cannot handle the way it handles the real service's — a parser or a comparison written against the service's rendering (sub-second precision, or whatever precision/layout the recording carries) either rejects the string or silently reads a different instant, and any client-side round-trip (re-sending the same value, diffing two reads, or sorting resources by creation date against values obtained elsewhere) goes wrong. Because the pair is prepended to EVERY tag read of a resource this API created, the fault is on the success path of ListTagsForResource for gateways and hypervisors alike, not on an error path.

Fix: the state layer that renders the tag list, not the handler — src/state.rs's created_date() and the system_tag_list()/reported_tags() pair it feeds; src/ops/list_tags_for_resource.rs only forwards what state.resource_tags() hands back and needs no change. Re-derive the rendering (precision and layout) from the recorded wire of the fixture steps below rather than from the crate's own whole-second convention, and keep the one renderer shared so StoredGateway::tag_list and StoredHypervisor::tag_list cannot drift apart.

No open issue covers this: 20261029T000229Z-backup-gateway-tagresource-error-response.md is about TagResource's check ORDER on an unknown ARN, and 20261030T102305Z-backup-gateway-importhypervisorconfiguration-mints-a-hypervisorarn-whose-hypervisor-id-is.md is about a minted ARN's id form; neither touches the rendering of a reported tag value.

Traces: backup-gateway-tag-resource-lifecycle-round-trip#task#4 (representative), backup-gateway-tag-resource-lifecycle-round-trip#task#6, backup-gateway-untag-nonexistent-key-noop#task#5.

## Evidence
Replay cluster (service backup-gateway, operation ListTagsForResource, verdict mismatch, affected_count 3, stub_path empty, owned_by empty):

  expected_status 200, actual_status 200
  detail: $.Tags[1].Value: value_mismatch (expected '<shaped-not-asserted>', got '2026-10-30T21:47:26Z')
  divergence_key: service=backup-gateway|ListTagsForResource|status=200->200|keys=$.Tags[1].Value
  affected: backup-gateway-tag-resource-lifecycle-round-trip#task#4,
            backup-gateway-tag-resource-lifecycle-round-trip#task#6,
            backup-gateway-untag-nonexistent-key-noop#task#5

Expected (recorded wire): Tags[1].Value is a per-run value asserted by shape — the service's rendering of the resource's creation instant. The fixture asserts no literal instant, so a differing clock reading alone cannot produce this mismatch.

Actual (emulator): a whole-second RFC-3339 date-time, e.g. '2026-10-30T21:47:26Z'.

Source of the actual value, services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/state.rs:
  * :1067-1076 reported_tags() = system_tag_list(created_by, created_time) chained ahead of tag_list(tags)
  * :1084-1095 system_tag_list() mints { aws:createdBy -> created_by } and { aws:createdDate -> created_date(created_time) }
  * :1097-1111 created_date() = DateTime::from_secs(date_time(created_time).secs()).fmt(Format::DateTime), documented as "truncated to WHOLE SECONDS"
  * :1027-1029 StoredGateway::tag_list and :1274-1276 StoredHypervisor::tag_list both go through reported_tags, so both resource types carry the same rendering
  * :2448-2457 resource_tags(), the single lookup src/ops/list_tags_for_resource.rs:169-194 serves the response from

Model basis (smithy_query shape=Tag,TagValue on services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/smithy-model/com-amazonaws-backupgateway-backuponpremises-v20210101.json): Tag$Value -> TagValue, @length 0..256, @pattern ^[^\x00]*$; no trait or doc states a format for an AWS-assigned tag value, hence "recorded behavior only" for the rendering itself.

Everything else in the same responses matches: status 200, ResourceArn, list length and ordering, Tags[1].Key, and the shape-asserted Tags[0].Value.

## Steps to Reproduce
1. Start the emulator with the backup-gateway service mounted.
2. Perform the activation handshake (GET /?activationRegion=us-west-2&no_redirect) and call CreateGateway with the issued ActivationKey; note the returned GatewayArn.
3. Call TagResource on that GatewayArn with a couple of client tags.
4. Call ListTagsForResource with that GatewayArn -> HTTP 200. Read Tags[1]: its Key is 'aws:createdDate' and its Value is a whole-second RFC-3339 date-time such as '2026-10-30T21:47:26Z'.
5. Compare that value's form with the recorded wire of backup-gateway-tag-resource-lifecycle-round-trip step task#4: the fixture asserts this value by shape (the instant itself is per-run and not asserted), and the emulator's rendering does not satisfy that shape.
6. Call UntagResource for one of the client keys and call ListTagsForResource again (task#6), and repeat against a never-set key (backup-gateway-untag-nonexistent-key-noop task#5): the same malformed value is reported every time, because it is synthesized on every tag read of a resource this API created.
