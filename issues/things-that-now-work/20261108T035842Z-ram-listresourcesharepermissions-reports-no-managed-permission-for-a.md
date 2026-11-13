Issue-Source: trace-sweep-divergence
Issue-Trace: ram-resource-share-permission-association
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-resource-share-permission-association
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|ListResourceSharePermissions|2xx->2xx|$.permissions[*]
Issue-Key: ram|ListResourceSharePermissions|2xx->2xx|$.permissions[*].status
Issue-Key: ram|ListResourceSharePermissions|2xx->2xx|$
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# RAM ListResourceSharePermissions reports no managed permission for a share it was associated with

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceSharePermissions

## Description
The emulator's resource-share permission set is write-blind: a managed permission that was associated with a resource share is never part of that share's permission set, so every read of the share's permissions answers 200 with an EMPTY `permissions` list — both immediately and after polling. Two distinct read paths in the recorded association lifecycle diverge.

Cause 1 of 2: ListResourceSharePermissions answers an empty permissions list for a share that has a customer-managed permission associated with it (immediate read-back).
  AWS: ListResourceSharePermissions "lists the RAM permissions that are associated with a resource share" and answers 200 with one `ResourceSharePermissionSummary` per managed permission attached to the share named by the required `resourceShareArn`. Smithy: ListResourceSharePermissions input {resourceShareArn required, nextToken, maxResults @range 1..500}, output {permissions: ResourceSharePermissionList, nextToken}, errors InvalidNextTokenException / InvalidParameterException / MalformedArnException / OperationNotPermittedException / UnknownResourceException (400), ServerInternalException (500), ServiceUnavailableException (503); AssociateResourceSharePermission input {resourceShareArn required, permissionArn required, replace, clientToken, permissionVersion} "Adds or replaces the RAM permission for a resource type included in a resource share", output {returnValue, clientToken}. After a successful AssociateResourceSharePermission for a customer-managed permission of resource type `imagebuilder:Component`, AWS reports that permission in the share's permission list with its `arn`, `resourceType`, the attached `version` (the permission's default version, rendered as a string), `defaultVersion: true` for that version, `featureSet: STANDARD`, a `lastUpdatedTime`, and `status` reported as `ASSOCIATED` for a permission currently attached to the share (recorded behavior only — the model documents ResourceSharePermissionSummary$status as only "The current status of the permission").
  Emulator: the same read answers HTTP 200 with `{"permissions": []}` — `$.permissions[0]` is missing entirely, so the associated permission is unobservable. The share's permission set is derived only from the ARNs the CREATE request carried plus the default AWS managed permission of each shared resource type (`ShareRecord::permission_arns`, consumed by `Store::share_permissions`); nothing ever adds an associated permission to it, and AssociateResourceSharePermission itself is still an unimplemented scaffold answering 501 NotImplemented (its own stub issue is auto-filed; this issue is about the permission set and the read, which answer 200 and are wrong rather than unimplemented).
  Why: a client that associates a managed permission with a resource share and then reads the share back concludes the share is governed by no permission at all. Any workflow that waits for the permission to appear (or that enumerates a share's permissions to decide whether to associate, replace or disassociate one) takes the wrong branch forever, and a permission whose attachment cannot be observed cannot be audited or replaced.
  Fix: the share/permission association state (the share's managed-permission set must record permissions attached after creation, not only those named at create time) and the ListResourceSharePermissions handler that projects it, including the `status` it reports for an attached permission.
  Traces: ram-resource-share-permission-association#task#5.

Cause 2 of 2: the permission never becomes observable on the share even after waiting — a polled read of the share's permissions stays empty until the poll cap.
  AWS: attaching a managed permission to a resource share settles; the share's permission list converges to the attached permission with `status: ASSOCIATED` within the lifecycle's polling window (recorded behavior only — AWS publishes no duration). The same ListResourceSharePermissions contract as above applies: 200 with the permission summary for the share's attached permission.
  Emulator: the polled read is capped at 60s (21 attempts) and every attempt answers 200 with `{"permissions": []}` — `$.permissions[0]` is missing and its `status` is never `ASSOCIATED`. There is no deferred/settling path that ever adds the permission: the set is computed from create-time ARNs only, so no amount of waiting changes the answer.
  Why: a client that polls the share's permissions for the attachment to complete never terminates (or times out and reports a failed association), even though the association request was accepted-looking from the caller's perspective. Downstream steps that gate on `status == ASSOCIATED` can never run.
  Fix: the share-permission association state and its settling path (the owed-move/settle mechanism the emulator already uses for resource and principal associations), plus the status the ListResourceSharePermissions handler projects for an attached permission.
  Traces: ram-resource-share-permission-association#task#8.

Related open RAM issues that do NOT cover this (they are about GetPermission staleness, CreatePermission refusal classes, and permission tagging, not the share's permission set): 20261103T042821Z-ram-getpermission-reports-stale-permission-state-after-version.md, 20261103T042821Z-ram-createpermission-refusals-use-the-wrong-exception-class.md, 20261103T042821Z-ram-tagging-operations-reject-managed-permission-arns-so.md.

## Evidence
Cause 1 — fixture ram-resource-share-permission-association, step task#5, ListResourceSharePermissions:
  expected status 200, actual status 200 (status agrees; the body does not)
  expected $.permissions[0] = {"arn": "<the associated customer-managed permission ARN>", "defaultVersion": true, "featureSet": "STANDARD", "lastUpdatedTime": <shaped-not-asserted>, "resourceType": "imagebuilder:Component", "status": "ASSOCIATED", "version": "1"}
  actual   $.permissions[0] = None (body is {"permissions": []})
  expected $.permissions[0].status in ['ASSOCIATED']; actual None after 10 poll attempt(s)
  (The ARN, the resource type and the version are values taken from the request and the emulator's state — the requirement is that the permission the request associated is reported, with the version that was attached and `status` ASSOCIATED; the recorded ARN itself is one run's value, not part of the contract.)

Cause 2 — fixture ram-resource-share-permission-association, step task#8, ListResourceSharePermissions:
  expected status 200, actual status 200
  $: poll capped at 60s — expected {"permissions": [{"arn": "<managed permission ARN>", "defaultVersion": true, "featureSet": "STANDARD", "lastUpdatedTime": <shaped-not-asserted>, "resourceType": "imagebuilder:Component", "status": "ASSOCIATED", "version": "1"}]}, got {"permissions": []}
  expected $.permissions[0].status in ['ASSOCIATED']; actual None after 21 poll attempt(s)

Code the behavior comes from (reference only):
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:488 — ShareRecord::permission_arns, documented as "the managed-permission ARNs the create request named"
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:968 — Store::share_permissions, which returns only those create-time ARNs plus each shared resource type's default AWS managed permission
  services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/list_resource_share_permissions.rs:232 — the handler reads share_permissions and reports version.status (ATTACHABLE for a customer-managed permission) rather than the share-association status
  services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/associate_resource_share_permission.rs:204 — still Scaffolded::not_implemented("AssociateResourceSharePermission")

## Steps to Reproduce
1. Replay the wire fixture corpus (trace set simparity-wire-aws-v1) against the emulator built from this checkout.
2. Fixture ram-resource-share-permission-association drives the managed-permission association lifecycle for a resource share: create a customer-managed permission for resource type imagebuilder:Component, create a resource share, associate the permission with the share, then read the share's permissions.
3. Step task#5 — POST /listresourcesharepermissions with {"resourceShareArn": "<the created share ARN>"}: the emulator answers 200 {"permissions": []} instead of 200 with the associated permission summary (status ASSOCIATED, version 1, resourceType imagebuilder:Component).
4. Step task#8 — the same read, polled to the 60s cap: every one of the 21 attempts answers 200 {"permissions": []}, so $.permissions[0].status never reaches ASSOCIATED.
5. Re-replay those two steps after the fix to confirm the share reports its attached managed permission.
