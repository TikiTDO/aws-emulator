Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-fleet-overrides-instance-type-and-requirements-conflict-rejected ec2.create-store-image-task-unsupported-bucket-region-rejected ec2.run-instances-refuses-a-nonexistent-image-and-launches-nothing ec2.run-instances-refuses-an-eight-hex-image-id-as-not-found ec2.op-contract-disable-fast-launch ec2.op-contract-enableFastLaunch ec2.op-contract-export-image ec2.op-contract-list-images-in-recycle-bin
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateFleet|InvalidParameterCombinationException
Issue-Key: ec2.create-store-image-task-unsupported-bucket-region-rejected|_error#23|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2.run-instances-refuses-a-nonexistent-image-and-launches-nothing|_client_error#9|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. Result: {result!r}")
Issue-Key: ec2.run-instances-refuses-an-eight-hex-image-id-as-not-found|run#10|AssertionError|assert got == _ABSENT, (
Issue-Key: ec2.op-contract-disable-fast-launch|run#11|AssertionError|assert err["Code"] == "InvalidRequest", f"expected InvalidRequest for a non-Windows AMI, got {err['Code']}"
Issue-Key: ec2.op-contract-enableFastLaunch|run#8|AssertionError|assert code == "InvalidRequest", f"expected InvalidRequest for a non-Windows AMI, got {code}"
Issue-Key: ec2.op-contract-export-image|run#38|AssertionError|assert code == "InvalidParameter", f"expected error code 'InvalidParameter', got {code!r}"
Issue-Key: ec2.op-contract-list-images-in-recycle-bin|run#21|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Commit: ffc9519ba35ae474158cb1300960bfc0fdc8c7a4

# ec2: 6 net-new refusal-classification causes — CreateFleet, CreateStoreImageTask, RunInstances AMI ids, Enable/DisableFastLaunch, ExportImage and ListImagesInRecycleBin DryRun

> _Note: this issue was identified against the code base at commit `ffc9519ba35ae474158cb1300960bfc0fdc8c7a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Six net-new root causes in how EC2 classifies refusals. All six must be closed before submitting. The EC2 Smithy model declares no modeled errors for any of these operations (errors: [] for CreateFleet, CreateStoreImageTask, RunInstances, EnableFastLaunch, DisableFastLaunch, ExportImage, ListImagesInRecycleBin), so each AWS requirement below is recorded behavior: the status is 400 and the code and message are as quoted.

Cause 1 of 6: CreateFleet with both InstanceRequirements and InstanceType in a launch-template override is refused under the wrong code, and the message is re-worded.
AWS: error code `InvalidParameterValue` with the exact message "You can specify either InstanceRequirements or InstanceType, but not both. Remove either InstanceRequirements or InstanceType, and try again." — the whole message is fixed text for this error class; it carries no request-derived values.
Emulator: code `InvalidParameterCombinationException` and the fixed text is altered ("Instance Type" instead of "InstanceType").
Why: a client branching on the error code never matches `InvalidParameterValue`, and `InvalidParameterCombinationException` is not an ec2Query code at all, so generic EC2 error handling treats it as unknown.
Fix: CreateFleet's validation of launch-template overrides and the refusal it raises.
Traces: ec2.create-fleet-overrides-instance-type-and-requirements-conflict-rejected.

Cause 2 of 6: CreateStoreImageTask accepts a bucket that is not in the request's Region.
AWS: the call is refused with code `InvalidRequest`; the message's fixed text is "The bucket is in this region: <bucket-region>. Please use this region to retry the request (Service: Amazon S3; Status Code: 301; Error Code: PermanentRedirect; Request ID: <s3-request-id>; S3 Extended Request ID: <id>; Proxy: null)" — the bucket's region comes from S3's state and the request ids are per-call values. No store-image task is recorded and no object is written.
Emulator: the call SUCCEEDS with HTTP 200 and returns an ObjectKey.
Why: a client gets a store task that claims to be storing an AMI into a bucket it can never write, and the negative branch AWS guarantees is unreachable.
Fix: CreateStoreImageTask's handler — resolve the bucket's region from S3 state and refuse a mismatch before a task is created.
Traces: ec2.create-store-image-task-unsupported-bucket-region-rejected.

Cause 3 of 6: RunInstances does not refuse an AMI id that names no image.
AWS: a well-formed 17-hex AMI id that no account owns is refused with code `InvalidAMIID.NotFound` and the message "The image id '[<ami-id>]' does not exist" — fixed text with the requested id interpolated — and nothing is launched. A short (8-hex) `ami-` id is likewise refused `InvalidAMIID.NotFound` with the same message template, because AWS treats the legacy-length form as absent rather than malformed.
Emulator: for the 17-hex absent id the launch SUCCEEDS and an instance is created; for the 8-hex id it answers InvalidAMIID.Malformed with a format message.
Why: a client launching against a mistyped or deregistered AMI gets a running, billable instance built from nothing instead of a refusal, and id-length handling makes the absent case look like a client format error.
Fix: RunInstances' image resolution and validation — look the ImageId up and raise InvalidAMIID.NotFound with AWS's message when it is absent; accept the legacy 8-hex `ami-` form as a lookup rather than a format error.
Traces: ec2.run-instances-refuses-a-nonexistent-image-and-launches-nothing, ec2.run-instances-refuses-an-eight-hex-image-id-as-not-found.

Cause 4 of 6: the fast-launch operations misclassify a non-Windows AMI.
AWS: EnableFastLaunch and DisableFastLaunch on an AMI that is not Windows are refused with code `InvalidRequest`; the message's fixed text is AWS's fast-launch-unsupported-platform wording with the image id from the request.
Emulator: DisableFastLaunch answers `IncorrectState` and EnableFastLaunch answers `InvalidParameterValue`.
Why: a client cannot distinguish "this AMI can never use fast launch" from a transient state or a bad parameter, so retry logic retries a permanent refusal.
Fix: the shared platform check behind the EnableFastLaunch and DisableFastLaunch handlers and the error class it raises.
Traces: ec2.op-contract-disable-fast-launch, ec2.op-contract-enableFastLaunch.

Cause 5 of 6: ExportImage misclassifies an absent ImageId.
AWS: ExportImage validates ImageId with the generic parameter-validation class, not the resource-specific one: code `InvalidParameter` with the message 'Invalid id: "<ami-id>"' (fixed text plus the requested id).
Emulator: code `InvalidAMIID.NotFound`.
Why: a client that distinguishes a malformed/unknown export argument from a missing AMI resource branches the wrong way, and the message template differs too.
Fix: ExportImage's ImageId validation and the refusal it raises.
Traces: ec2.op-contract-export-image.

Cause 6 of 6: ListImagesInRecycleBin ignores the DryRun contract.
AWS: a DryRun=true request from an authorized caller is answered with code `DryRunOperation` (HTTP 412) and AWS's fixed dry-run message; nothing is listed.
Emulator: code `NotImplemented`, so the dry-run contract is not honoured at all.
Why: a client probing permissions with DryRun reads a 501-class refusal and cannot tell an authorization answer from an unimplemented operation.
Fix: ListImagesInRecycleBin's handler — handle DryRun before the body, answering DryRunOperation for an authorized caller.
Traces: ec2.op-contract-list-images-in-recycle-bin.

## Evidence
Cause 1 — expected ('InvalidParameterValue', 'You can specify either InstanceRequirements or InstanceType, but not both. Remove either InstanceRequirements or InstanceType, and try again.'); got ('InvalidParameterCombinationException', 'You can specify either InstanceRequirements or InstanceType, but not both. Remove either InstanceRequirements or Instance Type, and try again.').

Cause 2 — expected ('InvalidRequest', 'The bucket is in this region: <other-region>. Please use this region to retry the request (Service: Amazon S3; Status Code: 301; Error Code: PermanentRedirect; Request ID: ...; Proxy: null)') and no store task recorded. Actual: HTTP 200 — "real AWS rejects this call, but it SUCCEEDED and returned {'ObjectKey': 'ami-00000000000000080.bin', ...}".

Cause 3 — expected ('InvalidAMIID.NotFound', "The image id '[ami-0b7d1c2e3f4a5b6c7]' does not exist") and no instance launched. Actual: HTTP 200 with a full RunInstances reservation — "the call succeeded; real AWS rejects it. Result: {'ReservationId': 'r-00000000000000001', ... 'Instances': [...]}". For the 8-hex form: "RunInstances with the absent image 'ami-12345678' returned ('InvalidAMIID.Malformed', 'Invalid id: \"ami-12345678\" (expecting \"ami-...\")'), expected ('InvalidAMIID.NotFound', \"The image id '[ami-12345678]' does not exist\")".

Cause 4 — "expected InvalidRequest for a non-Windows AMI, got IncorrectState" (DisableFastLaunch) and "expected InvalidRequest for a non-Windows AMI, got InvalidParameterValue" (EnableFastLaunch).

Cause 5 — "expected error code 'InvalidParameter', got 'InvalidAMIID.NotFound'" for ExportImage(ImageId="ami-00000000000000000", DiskImageFormat="VMDK", S3ExportLocation=..., RoleName=...); the expected message is 'Invalid id: "ami-00000000000000000"'.

Cause 6 — "expected DryRunOperation, got 'NotImplemented'" for ListImagesInRecycleBin with DryRun=true.

## Steps to Reproduce
Replay the behavioral suite and re-run the eight checks listed in trace_ids. Individually:
1. CreateFleet with a LaunchTemplateConfig override that sets BOTH InstanceRequirements and InstanceType; read the error code and message.
2. Create an S3 bucket in a Region other than the client's (CreateBucketConfiguration.LocationConstraint), register an AMI in the client's Region, then call CreateStoreImageTask(ImageId=<ami>, Bucket=<cross-region bucket>).
3. RunInstances(ImageId="ami-0b7d1c2e3f4a5b6c7", InstanceType="t3.micro", MinCount=1, MaxCount=1, SubnetId=<subnet>) and then RunInstances(ImageId="ami-12345678", ...); compare code and message against InvalidAMIID.NotFound.
4. EnableFastLaunch / DisableFastLaunch against a Linux (non-Windows) AMI.
5. ExportImage with ImageId="ami-00000000000000000", DiskImageFormat="VMDK", a valid S3ExportLocation and RoleName.
6. ListImagesInRecycleBin(DryRun=True) as an authorized caller.
