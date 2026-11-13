Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-ops-item-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-ops-item-lifecycle ssm-update-opsitem-status-transition ssm-ops-item-operational-data ssm-ops-item-lifecycle-and-query ssm-ops-item-not-found-errors ssm-opsitem-create-describe-events-delete ssm-ops-item-related-items
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetOpsItem|2xx->2xx|$.OpsItem.Status
Issue-Key: ssm|GetOpsItem|2xx->2xx|$.OpsItem.Version
Issue-Key: ssm|GetOpsItem|4xx->4xx|$.Message
Issue-Key: ssm|GetOpsItem|2xx->2xx|$.OpsItem.OperationalData.probeKey3
Issue-Key: ssm|GetOpsItem|4xx->2xx|$
Issue-Key: ssm|GetOpsItem|4xx->2xx|$.__type
Issue-Key: ssm|GetOpsItem|4xx->2xx|status
Issue-Key: ssm|GetOpsItem|2xx->2xx|$.OpsItem.RelatedOpsItems[*]
Issue-Commit: 685d7dc1b36320cc963844050fe238af8df0451a

# SSM OpsCenter mutations are never applied to stored OpsItems and GetOpsItem's not-found refusal is emulator-authored (7 causes)

> _Note: this issue was identified against the code base at commit `685d7dc1b36320cc963844050fe238af8df0451a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpsItem

## Description
The emulator's OpsCenter surface stores an OpsItem at CreateOpsItem time and then never mutates it: UpdateOpsItem and DeleteOpsItem are accepted (2xx) but the item GetOpsItem serves afterwards is byte-for-byte the created one, and the one modelled refusal of the surface carries the emulator's own sentence instead of AWS's. Seven distinct causes, all in service `ssm`, all on the OpsItem read/write path.

Cause 1 of 7 — OpsItem.Version does not advance when the item is edited (fixture ssm-ops-item-lifecycle, step task#3).
AWS: Smithy: OpsItem$Version is a String documented as "The version of this OpsItem. Each time the OpsItem is edited the version number increments by one."; recorded behavior shows AWS minting a 13-digit, monotonically increasing version token (an epoch-millisecond-scale value) that changes on every successful UpdateOpsItem. The exact digits are per-run and are NOT the requirement; the requirement is that the member is a monotonically increasing token that differs before and after an edit.
Emulator: returns the constant string "1" for OpsItem.Version on every GetOpsItem, both for a freshly created item and after one or more UpdateOpsItem calls.
Why: a client that uses Version for optimistic concurrency or change detection (re-read, compare Version, act) can never observe that its edit landed and loops or overwrites concurrent edits.
Fix: the OpsItem state store — Version must be re-derived on each mutation of an OpsItem record, not fixed at insert.
Traces: ssm-ops-item-lifecycle#task#3, ssm-ops-item-operational-data#task#3.

Cause 2 of 7 — UpdateOpsItem Status is not persisted (representative; fixture ssm-ops-item-lifecycle, step task#5).
AWS: Smithy: UpdateOpsItem accepts Status (enum OpsItemStatus) and OpsItemId (required, pattern ^(oi)-[0-9a-f]{12}$); a subsequent GetOpsItem returns OpsItem.Status equal to the value just written (recorded: Status 'InProgress' after an UpdateOpsItem setting Status=InProgress).
Emulator: GetOpsItem still reports Status 'Open' after UpdateOpsItem accepted the transition — expected 'InProgress', got 'Open' (with Version also unchanged, Cause 1).
Why: an OpsCenter workflow that drives an item Open -> InProgress -> Resolved sees every transition silently discarded, so status-gated automation never advances and status filters/queries return the wrong set.
Fix: the UpdateOpsItem handler's state mutation (it must write the supplied members back into the stored OpsItem).
Traces: ssm-ops-item-lifecycle#task#5, ssm-ops-item-lifecycle#task#7, ssm-update-opsitem-status-transition#task#3, ssm-update-opsitem-status-transition#task#5.

Cause 3 of 7 — the Resolved transition is likewise discarded (fixture ssm-ops-item-lifecycle-and-query, step teardown#7).
AWS: Smithy: OpsItem$Status / UpdateOpsItem.Status; after UpdateOpsItem sets Status=Resolved, GetOpsItem reports OpsItem.Status 'Resolved'.
Emulator: GetOpsItem reports 'Open' — expected 'Resolved', got 'Open'.
Why: the terminal state of an OpsItem is unobservable, so a caller cannot tell a closed operational item from an open one.
Fix: same UpdateOpsItem handler/state write path as Cause 2, for every value of the OpsItemStatus enum.
Traces: ssm-ops-item-lifecycle-and-query#teardown#7.

Cause 4 of 7 — the OpsItemNotFoundException message is the emulator's own sentence (fixture ssm-ops-item-not-found-errors, step task#1).
AWS: Smithy: GetOpsItem errors include OpsItemNotFoundException (client error, HTTP 400, member Message). Recorded response for an OpsItemId that does not exist: status 400, __type 'OpsItemNotFoundException', Message of the form 'OpsItem not found. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemNotFoundException; Request ID: <request id>; Proxy: null)'. Fixed text: 'OpsItem not found.' followed by the parenthesised 'Service: AmazonSSM; Status Code: 400; Error Code: OpsItemNotFoundException; ... Proxy: null' suffix; the only variable part is the Request ID, which is taken from the serving request and is not asserted here.
Emulator: status 400 and __type are right, but Message is "The specified OpsItem ID doesn't exist. Verify the ID and try again." — which is the Smithy doc string of the error shape, not the message AWS returns.
Why: callers and test suites that match on the refusal text (or parse the trailing Error Code/Status Code fields the AWS SDKs emit) do not recognise the refusal.
Fix: the GetOpsItem error serializer / message construction for OpsItemNotFoundException.
Traces: ssm-ops-item-not-found-errors#task#1.

Cause 5 of 7 — OperationalData entries added by UpdateOpsItem are not stored (fixture ssm-ops-item-operational-data, step task#5).
AWS: Smithy: UpdateOpsItem.OperationalData (map OpsItemOperationalData) / OpsItem$OperationalData; after an update adding a key, GetOpsItem returns that key with its {Type, Value} pair alongside the pre-existing keys (recorded: a third key present with Type 'String' and the value supplied in the request).
Emulator: the newly added OperationalData key is absent from GetOpsItem (expected the key's {'Type','Value'} object, got None); only the keys supplied at create time are present.
Why: operational metadata written after creation (log pointers, related-resource keys such as /aws/resources) is lost, so downstream readers of the OpsItem never see it.
Fix: the UpdateOpsItem handler's OperationalData merge into the stored OpsItem (including OperationalDataToDelete).
Traces: ssm-ops-item-operational-data#task#5.

Cause 6 of 7 — DeleteOpsItem does not delete: GetOpsItem keeps answering 200 (fixture ssm-ops-item-operational-data, step teardown#7).
AWS: Smithy: DeleteOpsItem (input OpsItemId required) removes the OpsItem; GetOpsItem on the deleted id then fails with status 400 / __type 'OpsItemNotFoundException' and the message described in Cause 4. Recorded behavior: the delete is eventually consistent but settles to the 400 refusal.
Emulator: GetOpsItem keeps returning status 200 with the full OpsItem body for the deleted id for the whole poll window (poll capped at 60s, 7 attempts, error code None) — expected 400 OpsItemNotFoundException, got 200.
Why: a deleted operational item remains readable and listable forever, so cleanup/teardown can never be confirmed and deletion-gated flows hang.
Fix: the DeleteOpsItem handler / OpsItem state store (the record must be removed, and reads of a removed id must take the not-found path).
Traces: ssm-ops-item-operational-data#teardown#7, ssm-opsitem-create-describe-events-delete#teardown#4.

Cause 7 of 7 — RelatedOpsItems written by UpdateOpsItem are not stored (fixture ssm-ops-item-related-items, step task#5).
AWS: Smithy: UpdateOpsItem.RelatedOpsItems (list RelatedOpsItems) / OpsItem$RelatedOpsItems; after the update, GetOpsItem returns one entry per related item, each carrying the OpsItemId supplied in the request.
Emulator: OpsItem.RelatedOpsItems is empty after the update (expected an entry with the related OpsItemId, got None).
Why: the relation graph between OpsItems is dropped, so OpsCenter consumers cannot follow duplicates/parents from the item.
Fix: the UpdateOpsItem handler's RelatedOpsItems write into the stored OpsItem.
Traces: ssm-ops-item-related-items#task#5.

No open issue owns any of these: the replay reported owned_by empty for all seven clusters. The nearest open issues concern other SSM surfaces (20261025T020208Z-cloudwatch-ssm-a-cloudwatch-alarm-s-ssm-opsitem-action-never, which is about an alarm action never creating an OpsItem, and 20261103T173026Z-ssm-enum-validationexception-reports-stale-value-sets-and, which covers DescribeOpsItems filter-key enum text) and do not cover the OpsItem read/write state path above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (service ssm, 7 net-new clusters, stub_path empty, owned_by empty):
- ssm-ops-item-lifecycle#task#3 (GetOpsItem, 200->200): $.OpsItem.Version value_mismatch — expected a 13-digit version token, got '1'.
- ssm-ops-item-lifecycle#task#5 (GetOpsItem, 200->200): $.OpsItem.Status expected 'InProgress', got 'Open'; $.OpsItem.Version expected a new 13-digit token, got '1'. Also affects ssm-ops-item-lifecycle#task#7, ssm-update-opsitem-status-transition#task#3, ssm-update-opsitem-status-transition#task#5.
- ssm-ops-item-lifecycle-and-query#teardown#7 (GetOpsItem, 200->200): $.OpsItem.Status expected 'Resolved', got 'Open'.
- ssm-ops-item-not-found-errors#task#1 (GetOpsItem, 400->400): $.Message expected 'OpsItem not found. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemNotFoundException; Request ID: <id>; Proxy: null)', got "The specified OpsItem ID doesn't exist. Verify the ID and try again."
- ssm-ops-item-operational-data#task#5 (GetOpsItem, 200->200): $.OpsItem.OperationalData.probeKey3 missing — expected {'Type': 'String', 'Value': <request value>}, got None.
- ssm-ops-item-operational-data#teardown#7 (GetOpsItem, 400->200): expected status 400 with __type 'OpsItemNotFoundException'; got 200 with the full OpsItem body (poll capped at 60s, 7 attempts, error code None). Also affects ssm-opsitem-create-describe-events-delete#teardown#4.
- ssm-ops-item-related-items#task#5 (GetOpsItem, 200->200): $.OpsItem.RelatedOpsItems[0] missing — expected an entry carrying the related OpsItemId, got None.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Status/Version: replay fixture ssm-ops-item-lifecycle and inspect step task#5 — CreateOpsItem, UpdateOpsItem(Status=InProgress), GetOpsItem; the response still shows Status 'Open' and Version '1'.
4. OperationalData: replay ssm-ops-item-operational-data step task#5 — the key added by UpdateOpsItem is missing from GetOpsItem.
5. RelatedOpsItems: replay ssm-ops-item-related-items step task#5.
6. Delete: replay ssm-ops-item-operational-data step teardown#7 — DeleteOpsItem then GetOpsItem keeps answering 200 for the whole poll window instead of 400 OpsItemNotFoundException.
7. Error text: replay ssm-ops-item-not-found-errors step task#1 — GetOpsItem on a nonexistent oi-<12 hex> id and compare the Message.
