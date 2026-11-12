Issue-Source: trace-sweep-divergence
Issue-Trace: eks-efsfips-traj-request-shape-mistakes-without-cluster
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-efsfips-traj-request-shape-mistakes-without-cluster eks-mbes-traj-create-addon-request-mistakes eks-efsfips-traj-addon-name-and-version-mistakes
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|ListAddons|4xx->4xx|$.clusterName
Issue-Key: eks|ListAddons|4xx->4xx|$.message
Issue-Key: eks|ListAddons|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|ListAddons|4xx->4xx|status
Issue-Related: 20261102T083656Z-eks-add-on-lifecycle-is-unreachable-clusters-never.md
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EKS add-on request validation runs in the wrong order and refuses with the wrong error class

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAddon

## Description
Seven net-new root causes share one fault: the add-on operations evaluate their preconditions in an order AWS does not use, so a request with several problems is refused for the wrong reason, with the wrong HTTP status and the wrong x-amzn-errortype. In particular the emulator resolves the cluster before validating the request's own shape in some paths and after it in others, and it reports a path-parameter pattern violation as a resource-not-found.

The already-open issue 20261029T185333Z-eks-refusal-messages-diverge-from-aws-wording-across.md covers wording on seven other operations and owns their replay keys; it does not cover these steps, which are about refusal ORDER and error CLASS on the add-on surface.

Cause 1 of 7: an invalid clusterName path label is not reported, and a different member's required-check fires first.
  AWS: CreateAddon's clusterName is an httpLabel constrained by @length 1..100 and @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$ (Smithy: CreateAddon.clusterName -> ClusterName, httpLabel, required). When the label does not match, AWS refuses with 400 and the message 'The parameter clusterName contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.' — all fixed text except the member name 'clusterName'. AWS also echoes the offending value in the error body's clusterName member.
  Emulator: refuses with the generic framework sentence "1 validation error detected: Value null at 'addonName' failed to satisfy constraint: Member must not be null" and omits $.clusterName from the body. The path-label pattern is never checked.
  Why: a client that passed a cluster ARN where a bare cluster name belongs is told its addonName is missing, so it fixes the wrong field and the request still fails.
  Fix: CreateAddon's validation — the httpLabel pattern/length check on clusterName must run and must be ordered ahead of body member presence checks — plus the error serializer that echoes clusterName.
  Traces: eks-efsfips-traj-request-shape-mistakes-without-cluster#task#1.

Cause 2 of 7: a cross-account PassRole refusal is reported as a missing cluster.
  AWS: CreateAddon refuses a serviceAccountRoleArn in another account with HTTP 403, x-amzn-errortype 'AccessDeniedException', message 'Cross-account pass role is not allowed.' (fixed text, no variable part), and no clusterName member in the body. AWS performs this authorization check before resolving the cluster.
  Emulator: HTTP 404, x-amzn-errortype 'ResourceNotFoundException', message 'No cluster found for name: <clusterName>.' and an extra clusterName member in the body.
  Why: a 403 is an identity/permission problem the client must fix in IAM; a 404 tells it to create the cluster instead. The client takes an action that cannot help.
  Fix: CreateAddon's validation — the role-ARN account check must be ordered ahead of cluster resolution and raise AccessDeniedException (403).
  Traces: eks-efsfips-traj-request-shape-mistakes-without-cluster#task#2.

Cause 3 of 7: an invalid serviceAccountRoleArn is reported as a missing cluster.
  AWS: CreateAddon refuses a malformed serviceAccountRoleArn with HTTP 400, x-amzn-errortype 'InvalidParameterException', message 'The provided addonServiceAccountRoleArn is invalid.' (fixed text, no variable part), with no clusterName member (Smithy: CreateAddon.serviceAccountRoleArn -> RoleArn @length 1..255; error InvalidParameterException 400).
  Emulator: HTTP 404 ResourceNotFoundException, 'No cluster found for name: <clusterName>.', plus an unmodeled clusterName member.
  Why: the client is pointed at the cluster rather than at the role ARN it actually got wrong.
  Fix: CreateAddon's validation — ARN shape check ordered ahead of cluster resolution, classified as InvalidParameterException.
  Traces: eks-efsfips-traj-request-shape-mistakes-without-cluster#task#3.

Cause 4 of 7: an out-of-range maxResults on ListAddons is reported as a missing cluster.
  AWS: ListAddons.maxResults is @range 1..100 (Smithy: ListAddons.maxResults -> ListAddonsRequestMaxResults, httpQuery, range min 1 max 100) and an out-of-range value is refused with HTTP 400, x-amzn-errortype 'InvalidParameterException', message 'maxResults needs to be between 1 and 100' — fixed text whose only variable parts are the bounds, which come from the model. No clusterName member appears in the body.
  Emulator: HTTP 404 ResourceNotFoundException, 'No cluster found for name: <clusterName>.', with an extra clusterName member.
  Why: a paginating client that passes a bad page size is told the cluster is gone and aborts the enumeration instead of correcting the page size.
  Fix: ListAddons' validation — query-parameter range checks must run before cluster resolution.
  Traces: eks-efsfips-traj-request-shape-mistakes-without-cluster#task#4.

Cause 5 of 7: the CreateAddon error body swaps clusterName and addonName.
  AWS: in the add-on refusal body, clusterName carries the cluster and addonName carries the add-on, each taken from the corresponding request parameter.
  Emulator: the two are transposed — the error body reports the add-on name in clusterName and the cluster name in addonName.
  Why: a client that logs or branches on the error body's clusterName attributes the failure to the wrong resource, and any automated remediation keyed on it targets a resource that does not exist.
  Fix: CreateAddon's error serializer, where the two members are populated.
  Traces: eks-efsfips-traj-addon-name-and-version-mistakes#task#8.

Cause 6 of 7: a resolveConflicts refusal pre-empts cluster resolution.
  AWS: when the named cluster does not exist, CreateAddon refuses with HTTP 404 and 'No cluster found for name: <clusterName>.' — fixed text 'No cluster found for name: ' plus the cluster name from the request and a trailing period — regardless of other request problems, and with no addonName member in the body. OVERWRITE is a modeled ResolveConflicts value (Smithy: ResolveConflicts enum = OVERWRITE, NONE, PRESERVE).
  Emulator: HTTP 400, x-amzn-errortype 'InvalidParameterException', message 'Resolve conflicts option overwrite is not supported for operation CreateAddon', plus an unmodeled addonName member. Cluster existence is never checked.
  Why: the client is told to change a parameter that AWS accepts, and never learns the cluster is absent — the actual blocker.
  Fix: CreateAddon's validation ordering — cluster resolution must precede the resolveConflicts check.
  Traces: eks-mbes-traj-create-addon-request-mistakes#task#1.

Cause 7 of 7: an invalid clusterName path label is reported as a missing cluster rather than as a pattern violation.
  AWS: a clusterName that violates @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$ is refused with HTTP 400 and 'The parameter clusterName contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.' — fixed text apart from the member name.
  Emulator: HTTP 404, x-amzn-errortype 'ResourceNotFoundException', 'No cluster found for name: <clusterName>.' — it looks the malformed name up in state instead of rejecting it.
  Why: 404 invites the client to create the cluster under a name AWS will never accept, so the retry loop cannot terminate.
  Fix: CreateAddon's validation — enforce the clusterName pattern on the path label before any state lookup.
  Traces: eks-mbes-traj-create-addon-request-mistakes#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — eks-efsfips-traj-request-shape-mistakes-without-cluster#task#1, CreateAddon, status 400->400: expected $.clusterName 'arn:aws:eks:us-west-2:000000000000:cluster/sim2real-probe-efsfips-none-0f804167' (got None) and $.message 'The parameter clusterName contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.'; got "1 validation error detected: Value null at 'addonName' failed to satisfy constraint: Member must not be null".
Cause 2 — eks-efsfips-traj-request-shape-mistakes-without-cluster#task#2, CreateAddon: expected status 403, x-amzn-errortype 'AccessDeniedException', $.message 'Cross-account pass role is not allowed.', $.clusterName absent; got status 404, x-amzn-errortype 'ResourceNotFoundException', $.message 'No cluster found for name: sim2real-probe-efsfips-none-0f804167.', $.clusterName populated.
Cause 3 — eks-efsfips-traj-request-shape-mistakes-without-cluster#task#3, CreateAddon: expected status 400, x-amzn-errortype 'InvalidParameterException', $.message 'The provided addonServiceAccountRoleArn is invalid.'; got 404, 'ResourceNotFoundException', 'No cluster found for name: sim2real-probe-efsfips-none-0f804167.' 2 affected steps.
Cause 4 — eks-efsfips-traj-request-shape-mistakes-without-cluster#task#4, ListAddons: expected status 400, x-amzn-errortype 'InvalidParameterException', $.message 'maxResults needs to be between 1 and 100'; got 404, 'ResourceNotFoundException', 'No cluster found for name: sim2real-probe-efsfips-none-0f804167.' 2 affected steps.
Cause 5 — eks-efsfips-traj-addon-name-and-version-mistakes#task#8, CreateAddon, status 400->400: expected $.clusterName 'aws-efs-csi-driver'... i.e. the recording has $.clusterName == the add-on-shaped value and $.addonName == the cluster-shaped value, while the emulator returned them transposed ($.clusterName 'sim2real-probe-efsfips-ver-db6e6015', $.addonName 'aws-efs-csi-driver').
Cause 6 — eks-mbes-traj-create-addon-request-mistakes#task#1, CreateAddon: expected status 404 and $.message 'No cluster found for name: sim2real-probe-mbes-cluster-28440ddc.' with $.addonName absent; got status 400, x-amzn-errortype 'InvalidParameterException', $.message 'Resolve conflicts option overwrite is not supported for operation CreateAddon', $.addonName 'cert-manager'.
Cause 7 — eks-mbes-traj-create-addon-request-mistakes#task#4, CreateAddon: expected status 400 and the clusterName-invalid-characters message; got status 404, x-amzn-errortype 'ResourceNotFoundException', $.message 'No cluster found for name: sim2real probe bad!.'

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreateAddon.clusterName (ClusterName, httpLabel, required, length 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$), CreateAddon.serviceAccountRoleArn (RoleArn, length 1..255), CreateAddon errors (InvalidParameterException 400, ResourceNotFoundException 404), ListAddons.maxResults (range 1..100), ResolveConflicts enum (OVERWRITE/NONE/PRESERVE).

## Steps to Reproduce
1. git clone the project repo and build the emulator from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Representative divergence: fixture eks-efsfips-traj-request-shape-mistakes-without-cluster, step task#1. The other six are at the fixture#step pairs listed per cause.
4. Manually: POST /clusters/<a-cluster-ARN-used-as-the-path-label>/addons with no addonName — the emulator reports addonName null instead of the clusterName pattern violation. POST /clusters/<nonexistent-cluster>/addons?... with resolveConflicts=OVERWRITE — the emulator reports the resolveConflicts value instead of 404 for the absent cluster. GET /clusters/<nonexistent-cluster>/addons?maxResults=0 — the emulator reports 404 instead of 400 'maxResults needs to be between 1 and 100'.
