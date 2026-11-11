Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-fleet-describe-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-fleet-describe-lifecycle ec2-describe-images-self-owned-pagination-and-params
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeFleets|2xx->2xx|$.DescribeFleetsResponse.fleetSet.item.excessCapacityTerminationPolicy
Issue-Key: ec2|DescribeFleets|2xx->2xx|$.DescribeFleetsResponse.fleetSet.item.onDemandOptions.allocationStrategy
Issue-Key: ec2|DescribeFleets|2xx->2xx|$.DescribeFleetsResponse.fleetSet.item.replaceUnhealthyInstances
Issue-Key: ec2|DescribeFleets|2xx->2xx|$.DescribeFleetsResponse.fleetSet.item.spotOptions.allocationStrategy
Issue-Key: ec2|DescribeFleets|2xx->2xx|$.DescribeFleetsResponse.fleetSet.item.spotOptions.instancePoolsToUseCount
Issue-Key: ec2|DescribeFleets|2xx->2xx|$.DescribeFleetsResponse.fleetSet.item.terminateInstancesWithExpiration
Issue-Key: ec2|DescribeFleets|2xx->2xx|$.DescribeFleetsResponse.fleetSet.item.fleetState
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[*].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[*].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-1"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-1"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-2"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-2"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-3"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-3"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-4"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-4"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-5"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-5"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-6"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-6"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-7"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-7"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-8"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5b2b585a-8"].freeTierEligible
Issue-Key: ec2|DeleteSnapshot|2xx->4xx|status
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# ec2 fleet/image read paths and snapshot delete diverge: omitted members, rewritten allocation strategy, HTTP 400 on DeleteSnapshot

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFleets

## Description
Six distinct causes, all on ec2, found by replaying the wire corpus (trace set simparity-wire-aws-v1). Each cause is numbered with its own fixture and step; all six must be closed before this issue is submitted.

Cause 1 of 6: DescribeFleets omits fleet members AWS always serializes.
AWS: Smithy com.amazonaws.ec2 FleetData carries ExcessCapacityTerminationPolicy (xmlName excessCapacityTerminationPolicy, enum FleetExcessCapacityTerminationPolicy = no-termination|termination), ReplaceUnhealthyInstances (replaceUnhealthyInstances, boolean), TerminateInstancesWithExpiration (terminateInstancesWithExpiration, boolean) and SpotOptions.InstancePoolsToUseCount (instancePoolsToUseCount, integer). The recording shows AWS serializing all four for an EC2 Fleet created with defaults: excessCapacityTerminationPolicy 'termination', replaceUnhealthyInstances 'false', terminateInstancesWithExpiration 'false', spotOptions.instancePoolsToUseCount '1'. The values are the request's effective settings (requested value, else the service default), not fixed literals.
Emulator: the four members are absent from the DescribeFleets fleetSet item entirely (replay: 'missing (expected ... got None)' for each of $.DescribeFleetsResponse.fleetSet.item.excessCapacityTerminationPolicy, .replaceUnhealthyInstances, .terminateInstancesWithExpiration and .spotOptions.instancePoolsToUseCount).
Why: a client that reads a fleet's capacity/health policy back from DescribeFleets (to display it, to diff it before ModifyFleet, or to assert it in a test) sees absent fields and must either guess defaults or treat the fleet as misconfigured; SDK getters return null where AWS returns a value.
Fix: the DescribeFleets fleet projection — the fleet record's state plus its output serializer must carry the effective policy/flag members instead of dropping those that were never explicitly requested.
Traces: ec2-fleet-describe-lifecycle#task#10 (also seen at ec2-fleet-describe-lifecycle#teardown#15).

Cause 2 of 6: DescribeFleets rewrites the allocation strategy the client supplied.
AWS: recorded behavior only. The recording shows AWS echoing the allocation strategy exactly as it was supplied on CreateFleet — onDemandOptions.allocationStrategy and spotOptions.allocationStrategy both read back as 'lowestPrice', the legacy spelling accepted by the API, even though the Smithy enums (FleetOnDemandAllocationStrategy = lowest-price|prioritized, SpotAllocationStrategy = lowest-price|diversified|capacity-optimized|capacity-optimized-prioritized|price-capacity-optimized) only list the hyphenated spelling. The requirement is that the read path returns the token the request carried, whichever accepted spelling that was.
Emulator: both members come back canonicalised to 'lowest-price' (replay: value_mismatch expected 'lowestPrice', got 'lowest-price' on $.DescribeFleetsResponse.fleetSet.item.onDemandOptions.allocationStrategy and .spotOptions.allocationStrategy).
Why: a client that round-trips a fleet definition (read the fleet, resubmit or compare it) sees a value it never sent; equality checks against its own configuration fail and config-drift detectors report a change that did not happen.
Fix: CreateFleet state capture plus the DescribeFleets serializer — store and return the caller's enum token verbatim rather than normalising it to the model's canonical value.
Traces: ec2-fleet-describe-lifecycle#task#10, ec2-fleet-describe-lifecycle#teardown#15.

Cause 3 of 6: a deleted fleet keeps reporting fleetState 'active'.
AWS: Smithy FleetStateCode = submitted|active|deleted|failed|deleted_running|deleted_terminating|modifying. After the fleet is deleted without terminating its instances, the recording shows DescribeFleets reporting fleetState 'deleted_running' — the state that says the fleet request is gone but its instances keep running.
Emulator: fleetState is still 'active' after the delete (replay: value_mismatch expected 'deleted_running', got 'active').
Why: a client polling DescribeFleets to confirm a delete never observes a terminal state, so delete-then-wait loops spin forever and teardown logic cannot tell a live fleet from a deleted one.
Fix: the fleet state machine behind DeleteFleets must move the record to deleted_running / deleted_terminating per the terminate-instances choice, and DescribeFleets must report it. Note this cause may be gated by the separate open stub issue 20261023T102725Z-ec2-deletefleets-stub.md (DeleteFleets answers 501); that issue covers implementing the operation, not the state value this read path must then publish, which is why it is filed here.
Traces: ec2-fleet-describe-lifecycle#teardown#15.

Cause 4 of 6: DescribeImages omits the block-device ebs.encrypted member.
AWS: Smithy EbsBlockDevice.Encrypted (xmlName encrypted, boolean). The recording shows AWS serializing encrypted for every blockDeviceMapping entry of a self-owned AMI — 'false' for an unencrypted snapshot-backed mapping; the value reflects the mapping's actual encryption state.
Emulator: the member is absent (replay: '$.DescribeImagesResponse.imagesSet.item[*].blockDeviceMapping.item.ebs.encrypted: missing (expected false, got None)' for every image in the page).
Why: a compliance or inventory client that scans AMIs for unencrypted root volumes reads null instead of a boolean and either skips the image or reports it as unknown.
Fix: the DescribeImages block-device-mapping serializer / the AMI state's ebs mapping record.
Traces: ec2-describe-images-self-owned-pagination-and-params#task#13, ec2-describe-images-self-owned-pagination-and-params#task#15.

Cause 5 of 6: DescribeImages reports freeTierEligible 'false' for images AWS reports as eligible.
AWS: Smithy Image.FreeTierEligible (xmlName freeTierEligible, boolean) — whether the image is free-tier eligible. The recording shows AWS returning 'true' for each of the account's own AMIs in this fixture. Hypothesis: the flag tracks the AMI's platform/usage-operation and the account's free-tier status rather than being a constant.
Emulator: every image is serialized with freeTierEligible 'false' (replay: value_mismatch expected 'true', got 'false' on $.DescribeImagesResponse.imagesSet.item[*].freeTierEligible, in both the index-ordered page and the name-keyed page).
Why: a client that filters candidate AMIs on free-tier eligibility finds none eligible and picks a different image (or refuses to launch), which is the opposite of the real answer.
Fix: the DescribeImages image projection — derive freeTierEligible from the image/account facts instead of hard-coding false.
Traces: ec2-describe-images-self-owned-pagination-and-params#task#13, ec2-describe-images-self-owned-pagination-and-params#task#15.

Cause 6 of 6: DeleteSnapshot refuses with HTTP 400 a snapshot the caller owns.
AWS: Smithy DeleteSnapshot input SnapshotId (required) + DryRun, output empty (Unit); the EC2 query model declares no per-operation errors. The recording shows AWS answering HTTP 200 with an empty DeleteSnapshotResponse when the caller deletes its own snapshot. AWS's 400 refusals for this call are InvalidSnapshot.NotFound (unknown snapshot id) and InvalidSnapshot.InUse (a registered AMI still references it) — neither applies to these steps.
Emulator: HTTP 400 (replay detail is exactly 'status: expected 200, got 400'; the cluster records no body, so the emulator's error code and message for this refusal are unverified and must themselves be checked against AWS's wording once the success path works — the fixed text of InvalidSnapshot.NotFound/InUse with the snapshot id taken from the request).
Why: teardown and any delete-after-use workflow fails: the snapshot leaks, the caller's error handling sees a client error for a legal request, and every subsequent step that assumes the snapshot is gone diverges.
Fix: DeleteSnapshot's handler/validation and the snapshot state it consults — a snapshot that exists and is owned by the caller (including one created implicitly by CreateImage) must delete and answer 200.
Traces: ec2-describe-images-self-owned-pagination-and-params#teardown#24, ec2-fleet-describe-lifecycle#teardown#18.

Adjacent open issues that do NOT carry these replay keys (the replay reported owned_by empty for all six causes, so none of them covers these steps): 20261031T181451Z-ec2-instance-volume-snapshot-and-iam-profile-projections.md (other read paths omitting modeled members), 20261108T044413Z-ec2-cancel-fleet-and-detach-trust-provider-responses.md (cancel-fleet previous-state), 20261104T221115Z-ec2-events-sqs-deletesnapshot-answers-501-notimplemented-and.md (DeleteSnapshot answering 501, a different status than the 400 seen here).

## Evidence
Replay of trace set simparity-wire-aws-v1 (429 fixtures, 3941 steps, 1055 mismatched):

1. ec2-fleet-describe-lifecycle#task#10 — DescribeFleets, expected 200 / got 200:
   $.DescribeFleetsResponse.fleetSet.item.excessCapacityTerminationPolicy: missing (expected 'termination', got None)
   $.DescribeFleetsResponse.fleetSet.item.onDemandOptions.allocationStrategy: value_mismatch (expected 'lowestPrice', got 'lowest-price')
   $.DescribeFleetsResponse.fleetSet.item.replaceUnhealthyInstances: missing (expected 'false', got None)
   $.DescribeFleetsResponse.fleetSet.item.spotOptions.allocationStrategy: value_mismatch (expected 'lowestPrice', got 'lowest-price')
   $.DescribeFleetsResponse.fleetSet.item.spotOptions.instancePoolsToUseCount: missing (expected '1', got None)
   $.DescribeFleetsResponse.fleetSet.item.terminateInstancesWithExpiration: missing (expected 'false', got None)

2. ec2-fleet-describe-lifecycle#teardown#15 — DescribeFleets, expected 200 / got 200: all of the above plus
   $.DescribeFleetsResponse.fleetSet.item.fleetState: value_mismatch (expected 'deleted_running', got 'active')

3. ec2-describe-images-self-owned-pagination-and-params#task#13 — DescribeImages, expected 200 / got 200, for items [0]..[5]:
   $.DescribeImagesResponse.imagesSet.item[N].blockDeviceMapping.item.ebs.encrypted: missing (expected 'false', got None)
   $.DescribeImagesResponse.imagesSet.item[N].freeTierEligible: value_mismatch (expected 'true', got 'false')

4. ec2-describe-images-self-owned-pagination-and-params#task#15 — DescribeImages, same two defects keyed by image name instead of index (8 images).

5. ec2-describe-images-self-owned-pagination-and-params#teardown#24 — DeleteSnapshot: 'status: expected 200, got 400'. Same cluster: ec2-fleet-describe-lifecycle#teardown#18.

Every cluster above was reported with stub_path empty (no unimplemented operation blocks it) and owned_by empty (no open issue carries its replay key).

## Steps to Reproduce
1. Replay fixture ec2-fleet-describe-lifecycle against the emulator. At step task#10 (DescribeFleets on the fleet created earlier in the fixture) compare the fleetSet item against the recording: excessCapacityTerminationPolicy, replaceUnhealthyInstances, terminateInstancesWithExpiration and spotOptions.instancePoolsToUseCount are absent, and both allocationStrategy members read 'lowest-price' where the recording has the caller's 'lowestPrice'.
2. Continue the same fixture to teardown#15: after the fleet is deleted without terminating instances, DescribeFleets still reports fleetState 'active' instead of 'deleted_running'.
3. Replay fixture ec2-describe-images-self-owned-pagination-and-params. At task#13 and task#15 (DescribeImages over the account's own AMIs) every image omits blockDeviceMapping.item.ebs.encrypted and reports freeTierEligible 'false' instead of 'true'.
4. Continue that fixture to teardown#24 (DeleteSnapshot of a snapshot the fixture itself created): the emulator answers HTTP 400 where the recording has HTTP 200 with an empty DeleteSnapshotResponse. The same refusal reproduces at ec2-fleet-describe-lifecycle#teardown#18.
