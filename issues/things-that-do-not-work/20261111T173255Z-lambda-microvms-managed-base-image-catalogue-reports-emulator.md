Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-microvms-catalog-list-managed-microvm-images
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-microvms-catalog-list-managed-microvm-images
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|ListManagedMicrovmImages|2xx->2xx|$.items[*]
Issue-Commit: 5776b1d0bb94f52c23a7c9dceb3e487542215018

# lambda-microvms managed base-image catalogue reports emulator-chosen publication timestamps

> _Note: this issue was identified against the code base at commit `5776b1d0bb94f52c23a7c9dceb3e487542215018`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListManagedMicrovmImages

## Description
Cause 1 of 1: ListManagedMicrovmImages reports the AWS-managed base-image catalogue entry for `al2023-1` with publication instants this emulator invented, so the entry's `createdAt`/`updatedAt` do not match the catalogue metadata AWS publishes.

AWS: recorded behavior only. Smithy (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms): ListManagedMicrovmImages GET /2025-09-09/managed-microvm-images -> 200, output member `items` (ManagedMicrovmImageSummaryList) @required; ManagedMicrovmImageSummary members `imageArn` (@required), `createdAt` (Timestamp, @required, "The timestamp when the managed MicroVM image was created") and `updatedAt` (Timestamp, "The timestamp when the managed MicroVM image was last updated"). The model fixes no values, but the managed catalogue is AWS-owned published metadata, not per-account or per-run state: the recorded 200 reports the single catalogue entry `arn:aws:lambda:<region>:aws:microvm-image:al2023-1` with a `createdAt` and an `updatedAt` that are properties of AWS's catalogue and are stable across calls, and it serializes them at sub-second (millisecond) precision as epoch-seconds-with-fraction (recorded: createdAt 1781715610.862, updatedAt 1790702978.629 — the catalogue's fixed publication instants, each later than mid-2026 and updatedAt strictly after createdAt).

Emulator: the same step answers 200 with the right single entry and the right ARN, but with whole-second constants the emulator chose for itself: createdAt 1757376000 (2025-09-09T00:00:00Z, the service's API-version date) and updatedAt 1760400000 (2025-10-14T00:00:00Z). The replay reports `$.items[0]: missing`/`extra` because the item compares unequal on exactly those two members. The values are not read from the catalogue at all: they are derived from the hard-coded generation table in the service's state (MANAGED_BASE_IMAGE_VERSIONS, whose own comment says "The instants below are chosen (this emulator does not hold AWS's own publication dates)"), folded into an entry by `managed_base_image_publication` and projected by `ComAmazonawsLambdamicrovmsLambdamicrovmsState::managed_base_images`. Because the table is also what ListMicrovmImageVersions and base-image resolution read, every catalogue read reports the same invented pair with no fractional component.

Why: a client that uses the managed catalogue's timestamps to decide anything gets the wrong answer. Code that keys a cache or a local record on the entry's `updatedAt`, or that rebuilds its own image version when the base image it builds on has been republished since the last build, sees a base image frozen at 2025-10-14 and skips rebuilds it must perform; code that compares the base image's `createdAt`/`updatedAt` against its OWN image-version timestamps sees the base image as predating everything it owns by over a year, inverting an ordering AWS reports the other way round. Serializing the instants with second-only granularity also drops the millisecond component AWS returns, so a client that round-trips the value back into a comparison or an ETag-style key cannot reproduce AWS's value.

Fix: the managed-image catalogue data in the service's state, not the handler or the serializer — ListManagedMicrovmImages itself correctly projects whatever the catalogue hands it. The generation table the catalogue's publication instants are read from must carry the instants AWS's catalogue publishes, at the sub-second precision the recording shows, so that the entry's `createdAt` (oldest generation's publication instant) and `updatedAt` (newest generation's) match AWS; the per-version reads that share that table (ListMicrovmImageVersions / managed_base_image_version_published_at) must stay consistent with it.

Traces: lambda-microvms-catalog-list-managed-microvm-images#task#1 (ListManagedMicrovmImages, expected 200 / actual 200, `$.items[0]` unequal on createdAt and updatedAt). Code: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs lines 496-502 (MANAGED_BASE_IMAGE_VERSIONS), 578-590 (managed_base_image_publication), 1731-1745 (managed_base_images); services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/list_managed_microvm_images.rs lines 202-213 (projection of createdAt/updatedAt).

## Evidence
Fixture lambda-microvms-catalog-list-managed-microvm-images, step task#1, operation ListManagedMicrovmImages, verdict mismatch, expected_status 200, actual_status 200.

Expected (recorded AWS) $.items[0]:
  {"imageArn": "arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1", "createdAt": 1781715610.862, "updatedAt": 1790702978.629}

Actual (emulator) $.items[0]:
  {"imageArn": "arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1", "createdAt": 1757376000, "updatedAt": 1760400000}

Replay detail: "$.items[0]: missing" + "$.items[0]: extra" — the single catalogue item differs on createdAt and updatedAt only; imageArn, the item count and the HTTP status all match. The emulator's two values are the constants in MANAGED_BASE_IMAGE_VERSIONS (1_760_400_000 for generation "1", 1_757_376_000 for generation "0"), i.e. invented catalogue metadata rather than AWS's, and both lack the fractional-second component AWS serializes. Neither value is per-run: the emulator returns the same pair on every call, and so does AWS — the pair is simply the wrong pair.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture lambda-microvms-catalog-list-managed-microvm-images, step task#1: GET /2025-09-09/managed-microvm-images against the lambda-microvms endpoint with no maxResults/nextToken.
3. The call answers 200 with one item for arn:aws:lambda:<region>:aws:microvm-image:al2023-1.
4. Compare $.items[0].createdAt and $.items[0].updatedAt with the recorded response: the emulator returns whole-second constants anchored on 2025-09-09 and 2025-10-14 instead of the catalogue's published instants (recorded 1781715610.862 / 1790702978.629, millisecond precision), so the item comparison fails even though the status, the item count and the ARN all match.
