Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-collection-group-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-collection-group-lifecycle
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|ListCollectionGroups|2xx->2xx|$.collectionGroupSummaries[arn="arn:aws:aoss:us-west-2:000000000000:collection-group/frcezlkw8uno43umn8f4fx2i"].storageConfiguration
Issue-Commit: cf4b8496ba0c958ea150f5d2d9329060f4b043af

# aoss ListCollectionGroups omits storageConfiguration from every collection-group summary

> _Note: this issue was identified against the code base at commit `cf4b8496ba0c958ea150f5d2d9329060f4b043af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCollectionGroups

## Description
ONE net-new root cause from replaying the wire corpus simparity-wire-aws-v1 against this checkout: the collection-group LISTING drops the storageConfiguration block that the live service reports on every collection group, so a listing describes a group differently from a read of the same group.

=== Cause 1 of 1: ListCollectionGroups' collectionGroupSummaries entries carry no storageConfiguration ===

AWS: recorded behavior only for this member on the LISTING shape. Smithy: ListCollectionGroups is @readonly/@paginated and answers collectionGroupSummaries of CollectionGroupSummary {id, arn, name, numberOfCollections, createdDate, capacityLimits, generation} — the model as carried in this repo does NOT declare storageConfiguration on CollectionGroupSummary. The model DOES carry the shape CollectionGroupStorageConfiguration {v2Enabled: Boolean, "Whether the collection group's data is held on the next-generation (v2) storage stack"}, and its own documentation records why: "LOCAL ADDITION to this model, re-apply after a refresh from upstream: the shape and the storageConfiguration member on CreateCollectionGroupDetail and CollectionGroupDetail are not in the model as shipped, and are carried here because the live OpenSearch Serverless API returns the block on every collection-group detail (recorded behaviour) and this emulator can only serialize modelled members." The recording shows the live service returns that same block on the LISTING row too: the summary for the group created by this fixture carries storageConfiguration {"v2Enabled": false} (false is the value a CLASSIC-generation group reports; a NEXTGEN group reports true). The local addition was applied to the two detail shapes and not to the summary.

Emulator: HTTP 200 and the summary row is otherwise correct, but storageConfiguration is absent (missing, got None) — the member is neither modelled on CollectionGroupSummary nor rendered. The omission is deliberate in the code: CollectionGroupRecord::summary() builds the row from id/arn/name/numberOfCollections/createdDate/capacityLimits/generation only, and its doc comment states that storageConfiguration is "not in it" because the model's summary shape does not declare it, while CollectionGroupRecord::detail()/create_detail() both set storage_configuration: Some(self.storage_configuration()).

Why: a client that reads a group's storage mode (classic v1 vs next-generation v2) off a listing — an inventory sweep, a reconciliation loop, a console-style table, any code that lists groups instead of getting them one at a time — sees the member as absent rather than as false/true, so it cannot tell a CLASSIC group from a NEXTGEN one without a second per-group call, and code that treats an absent block as "unknown" or dereferences v2Enabled breaks. It also makes the emulator self-inconsistent: the same group reports storageConfiguration when read and not when listed.

Fix: the collection-group listing's serializer plus the local model addition it depends on — add storageConfiguration (target CollectionGroupStorageConfiguration) to CollectionGroupSummary in this service's Smithy model exactly as it is already carried on CreateCollectionGroupDetail and CollectionGroupDetail (re-apply after an upstream refresh, as that shape's doc instructs), and render it in CollectionGroupRecord::summary() from the same storage_configuration() helper detail() uses, so the listing row cannot report a group differently from a read of it.

Traces: opensearchserverless-collection-group-lifecycle#task#2 (ListCollectionGroups).

No open issue covers this: the replay reports the cluster as unowned, and the aoss issues already open concern other planes — 20261029T210149Z-aoss-state-plane-an-accepted-write-is-lost.md (lost writes and the form of minted ids/policy versions), 20261029T210149Z-aoss-tagging-plane-cannot-resolve-a-collection-group.md (tag-resource ARN resolution) and 20261109T201926Z-aoss-batchgetcollection-publishes-production-aoss-amazonaws-com-collection.md (published endpoint hosts).

## Evidence
From the simparity-wire-aws-v1 replay (emulator up; 150 steps, 134 passed).

opensearchserverless-collection-group-lifecycle#task#2 ListCollectionGroups, status 200 expected and 200 served. Divergence: $.collectionGroupSummaries[arn="arn:aws:aoss:<region>:<account>:collection-group/<group id>"].storageConfiguration: missing — expected {'v2Enabled': False}, got None. (The ARN's region/account and the group id are per-run values used only to select the row; the defect is that the storageConfiguration block is absent from the row at all. v2Enabled is false here because the fixture's group is CLASSIC generation.)

Source of the omission in this checkout: services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs — CollectionGroupRecord::summary() (lines 1103-1113) sets only id/arn/name/number_of_collections/created_date/capacity_limits/generation, with the doc comment at lines 1098-1102 recording that storageConfiguration is excluded because CollectionGroupSummary does not model it; CollectionGroupRecord::detail() (line 1089) and ::create_detail() (line 1069) both set storage_configuration: Some(self.storage_configuration()), computed at lines 1127-1130 as v2_enabled = (generation == NEXTGEN).

Model evidence: smithy_query(query="shape", shape="CollectionGroupSummary") lists no storageConfiguration member; smithy_query(query="doc", shape="CollectionGroupStorageConfiguration") carries the LOCAL ADDITION note quoted in the description, which names only CreateCollectionGroupDetail and CollectionGroupDetail.

## Steps to Reproduce
Boot the emulator from this checkout and replay the aoss wire fixture opensearchserverless-collection-group-lifecycle, inspecting step task#2; or with an SDK against the emulator:

1. aoss.create_collection_group(name=<n>, ...) (no generation named, i.e. a CLASSIC group) and note that the create response's createCollectionGroupDetail carries storageConfiguration {'v2Enabled': False}.
2. aoss.list_collection_groups() and inspect the collectionGroupSummaries row whose arn/name is that group -> expected the row to carry storageConfiguration {'v2Enabled': False} as the recorded AWS response does; observe the member absent from the row.
3. For contrast, aoss.get_collection_group / batch_get_collection_group style read of the same group reports storageConfiguration, so the listing and the read disagree about the same resource.
