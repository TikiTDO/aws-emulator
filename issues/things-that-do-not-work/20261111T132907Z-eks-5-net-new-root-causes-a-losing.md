Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.concurrent-cluster-delete-conflict eks.create-cluster-auto-mode-requires-matching-config eks.delete-cluster-blocked-by-nodegroup-dependency eks.nodegroup-release-version-matches-optimized-ami-release-format eks.cluster-version-update-in-progress-keeps-old-version-and-refuses-second
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.concurrent-cluster-delete-conflict|run#11|AssertionError|assert code == "ResourceInUseException", (
Issue-Key: eks.create-cluster-auto-mode-requires-matching-config|run#10|AssertionError|assert msg == _AUTO_MODE_ALL_OR_NOTHING, msg
Issue-Key: eks.delete-cluster-blocked-by-nodegroup-dependency|run#16|AssertionError|assert fp_code is not None, (
Issue-Key: eks.nodegroup-release-version-matches-optimized-ami-release-format|run#15|AssertionError|assert match, f"releaseVersion must look like <major.minor.patch>-<YYYYMMDD>, got {release!r}"
Issue-Key: eks.cluster-version-update-in-progress-keeps-old-version-and-refuses-second|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: ffc9519ba35ae474158cb1300960bfc0fdc8c7a4

# eks: 5 net-new root causes — a losing concurrent DeleteCluster is 404 not 409, the Auto Mode all-or-nothing refusal is mis-worded, CreateFargateProfile still accepts a public subnet, DescribeNodegroup omits releaseVersion, and a VersionUpdate never leaves ACTIVE

> _Note: this issue was identified against the code base at commit `ffc9519ba35ae474158cb1300960bfc0fdc8c7a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFargateProfile

## Description
Five net-new root causes surviving in the eks service after the already-filed/owned clusters are accounted for. All names, ids, ARNs, subnet ids and version strings quoted below are template slots filled from the request or from the emulator's own state — never literal requirements.

Cause 1 of 5: a losing concurrent DeleteCluster is refused ResourceNotFoundException instead of ResourceInUseException.
  AWS: Smithy: DeleteCluster (DELETE /clusters/{name}) models ResourceInUseException (409) and ResourceNotFoundException (404). Two DeleteCluster calls racing on the same ACTIVE cluster resolve to ONE deletion: at least one racer is accepted with HTTP 200 and cluster.status DELETING on the one cluster ARN, and a racer that loses the transition is refused with 409 ResourceInUseException — the cluster still exists and is DELETING, so it is in-use, not absent. The name also stays claimed: CreateCluster on it is refused 409 ResourceInUseException 'Cluster already exists with name: <name>' (fixed text 'Cluster already exists with name: ' plus the requested cluster name) both before and during the deletion, and a repeat DeleteCluster is idempotent (200, DELETING, same ARN).
  Emulator: the losing racer is refused ResourceNotFoundException (404) — 'a losing concurrent DeleteCluster must be ResourceInUseException, got ResourceNotFoundException'.
  Why: a 404 tells a client the cluster never existed or is already gone, so idempotent/retrying deleters (CDK, Terraform, controllers) treat the resource as fully removed while it is still DELETING and proceed to recreate or to drop their state, whereas AWS's 409 tells them to wait for the in-flight transition. Status-code class also differs (404 vs 409), so retry classification differs.
  Fix: DeleteCluster's handler/state transition — serialize the delete on the cluster record and refuse a concurrent loser with ResourceInUseException rather than resolving it against a record already removed.
  Traces: eks.concurrent-cluster-delete-conflict.

Cause 2 of 5: the EKS Auto Mode all-or-nothing refusal carries emulator-authored wording instead of EKS's sentence.
  AWS: Smithy: CreateCluster models InvalidParameterException (400) and takes computeConfig (ComputeConfigRequest), storageConfig (StorageConfigRequest) and kubernetesNetworkConfig (KubernetesNetworkConfigRequest). EKS Auto Mode is all-or-nothing: enabling computeConfig without block storage and elastic load balancing (or any partial combination of the three) is refused 400 InvalidParameterException with the message 'For EKS Auto Mode, please ensure that all required configs, including computeConfig, kubernetesNetworkConfig, and blockStorage are all either fully enabled or fully disabled.' — the whole sentence is fixed text, with no variable parts.
  Emulator: the error CLASS and status are right (InvalidParameterException), but the message is 'All of computeConfig.enabled, kubernetesNetworkConfig.elasticLoadBalancing.enabled, and storageConfig.blockStorage.enabled must be set to either true or false.'
  Why: the message is the only machine-readable discriminator between the several InvalidParameterException causes CreateCluster can raise; tooling and operators that match on EKS's sentence (and docs/runbooks quoting it) do not recognise the emulator's, so the Auto Mode misconfiguration cannot be told apart from an unrelated parameter rejection.
  Fix: CreateCluster's Auto Mode validation — emit EKS's fixed sentence for the partial-Auto-Mode refusal.
  Traces: eks.create-cluster-auto-mode-requires-matching-config.

Cause 3 of 5: CreateFargateProfile still accepts a public subnet, so the Fargate dependency class cannot be exercised.
  AWS: Smithy: CreateFargateProfile (POST /clusters/{clusterName}/fargate-profiles) models InvalidParameterException, InvalidRequestException and ClientException, all 400. A Fargate profile may use only private subnets; a public subnet is refused with a 400 in that set — 'Subnet <subnetId> provided in Fargate Profile is not a private subnet' (fixed text 'Subnet ' / ' provided in Fargate Profile is not a private subnet', the subnet id taken from the request) — and no profile is left behind (ListFargateProfiles returns an empty fargateProfileNames).
  Emulator: CreateFargateProfile on the cluster's public subnets succeeds — 'CreateFargateProfile on public subnets must be rejected (Fargate requires private subnets), but it was accepted'. (This check first exercises DeleteCluster with an attached nodegroup, which now answers correctly, and fails only at this Fargate step.)
  Why: a profile AWS would never create is recorded, so the deploy-time failure is deferred and a caller cannot discover that its Fargate subnets are wrong until it tries to run pods on real AWS. Open issue 20261104T105252Z-eks-10-net-new-root-causes-unvalidated-pod.md names the same subnet validation as its Cause 3 (grounded in eks.fargate-profile-public-subnet-rejected); this check is reported UNOWNED under the eks heading, so it is filed here and should be closed together with that cause.
  Fix: CreateFargateProfile's subnet validation — resolve each subnet and refuse any that is public.
  Traces: eks.delete-cluster-blocked-by-nodegroup-dependency.

Cause 4 of 5: DescribeNodegroup omits releaseVersion for an ACTIVE managed nodegroup.
  AWS: Smithy: Nodegroup.releaseVersion is documented as 'If the node group was deployed using a launch template with a custom AMI, then this is the AMI ID that was specified in the launch template. For node groups that weren't deployed using a launch template, this is the version of the Amazon EKS optimized AMI that the node group was deployed with.' For an amiType of the EKS-optimized Amazon Linux family created without a launch template, an ACTIVE nodegroup reports a releaseVersion of the form <major>.<minor>.<patch>-<YYYYMMDD>, whose <major>.<minor> equals the nodegroup's Kubernetes version (which itself defaults to the cluster version) and whose trailing component is a real calendar build date.
  Emulator: the nodegroup object returned by DescribeNodegroup for an ACTIVE nodegroup has no releaseVersion value at all — 'releaseVersion must look like <major.minor.patch>-<YYYYMMDD>, got None'. amiType and version round-trip correctly, so only this member is missing.
  Why: releaseVersion is the only way a client can tell which EKS-optimized AMI build its nodes are running, which is exactly what AMI-drift detection, pinning and nodegroup upgrade decisions key on; its absence makes every such client either crash on the missing key or silently conclude the nodegroup has no AMI identity.
  Fix: the Nodegroup state/serializer for CreateNodegroup/DescribeNodegroup — derive and persist releaseVersion for the resolved amiType and Kubernetes version (and echo the launch template's AMI id when one is used).
  Traces: eks.nodegroup-release-version-matches-optimized-ami-release-format.

Cause 5 of 5: after UpdateClusterVersion the cluster never leaves ACTIVE, so the VersionUpdate in-progress behaviour is unreachable.
  AWS: Smithy: UpdateClusterVersion (POST /clusters/{name}/updates) returns an Update and models ResourceInUseException (409). AWS accepts it with an update of type VersionUpdate and status InProgress whose params carry Version=<new version>; the cluster then leaves ACTIVE for UPDATING while DescribeCluster still reports the OLD version, a second UpdateClusterVersion during that window is refused 409 ResourceInUseException 'Cannot VersionUpdate because cluster <name> currently has update <updateId> in progress' (fixed text 'Cannot VersionUpdate because cluster ' / ' currently has update ' / ' in progress', with the cluster name and the in-flight update id as the variables), the refused attempt is NOT recorded in ListUpdates, and once the update reaches Successful DescribeCluster reports the new version and ACTIVE again.
  Emulator: the check makes no assertion past the update poll — it exhausts the suite's 60s budget ('Check timed out after 60.0s') while polling DescribeCluster for the cluster to leave ACTIVE after an accepted UpdateClusterVersion. Hypothesis: the emulator either applies the version change without ever publishing an UPDATING state or leaves the update parked, so DescribeCluster stays ACTIVE and the in-progress window — including the second-update ResourceInUseException and the ListUpdates invariant — can never be observed. (The check's own poll deadlines are minutes, so the 60s budget only bounds the symptom; cluster creation reaching ACTIVE works, since the check gets past it.)
  Why: a client that drives an EKS upgrade waits for UPDATING and then for the update to be Successful; against the emulator it either never sees the transition it is told to wait for, or it is free to launch a second conflicting upgrade that AWS refuses. Open issues 20261104T105252Z-eks-2-net-new-root-causes-the-metrics.md (Cause 2, 'asynchronous EKS resources never reach a terminal state') and 20261029T175608Z-eks-3-net-new-root-causes-startinsightsrefresh-response.md describe the same family for other operations; this check is reported UNOWNED and names the cluster VersionUpdate state machine specifically.
  Fix: the cluster version-update state machine behind UpdateClusterVersion — publish cluster.status UPDATING with the old version while the VersionUpdate is InProgress, gate a second VersionUpdate with ResourceInUseException, and settle the update to Successful with the new version and ACTIVE.
  Traces: eks.cluster-version-update-in-progress-keeps-old-version-and-refuses-second.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout. Overall 217/352 passed; service eks 202/311 passed (109 failed). Every failure below is reported UNOWNED and not marked ALREADY FILED in that report.

Cause 1 — eks.concurrent-cluster-delete-conflict [mismatch]:
  check_eks_concurrent_cluster_delete_conflict.py, line 193, in run
  assert code == "ResourceInUseException"
  AssertionError: a losing concurrent DeleteCluster must be ResourceInUseException, got ResourceNotFoundException
  (the check first confirms the duplicate-name refusal 409 ResourceInUseException 'Cluster already exists with name: <name>' passes, then races two DeleteCluster calls on one ACTIVE cluster.)

Cause 2 — eks.create-cluster-auto-mode-requires-matching-config [mismatch]:
  check_eks_create_cluster_auto_mode_requires_matching_config.py, line 246, in run
  assert msg == _AUTO_MODE_ALL_OR_NOTHING, msg
  AssertionError: All of computeConfig.enabled, kubernetesNetworkConfig.elasticLoadBalancing.enabled, and storageConfig.blockStorage.enabled must be set to either true or false.
  Required (_AUTO_MODE_ALL_OR_NOTHING): 'For EKS Auto Mode, please ensure that all required configs, including computeConfig, kubernetesNetworkConfig, and blockStorage are all either fully enabled or fully disabled.'
  Triggering request: CreateCluster(name=<name>, roleArn=<role>, resourcesVpcConfig={subnetIds:[<subnet-a>,<subnet-b>]}, computeConfig={enabled:true}) with no storageConfig and no kubernetesNetworkConfig.elasticLoadBalancing. The error code asserted just above (InvalidParameterException) passed, so only the message diverges.

Cause 3 — eks.delete-cluster-blocked-by-nodegroup-dependency [mismatch]:
  check_eks_delete_cluster_blocked_by_nodegroup_dependency.py, line 286, in run
  assert fp_code is not None
  AssertionError: CreateFargateProfile on public subnets must be rejected (Fargate requires private subnets), but it was accepted
  The check accepts any of {InvalidParameterException, InvalidRequestException, ClientException} as the refusal code and then requires ListFargateProfiles to be empty. The earlier assertions in the same check pass: DeleteCluster with an attached nodegroup is refused ResourceInUseException 'Cluster has nodegroups attached' and the cluster stays ACTIVE.

Cause 4 — eks.nodegroup-release-version-matches-optimized-ami-release-format [mismatch]:
  check_eks_nodegroup_release_version_matches_optimized_ami_release_format.py, line 271, in run
  assert match, f"releaseVersion must look like <major.minor.patch>-<YYYYMMDD>, got {release!r}"
  AssertionError: releaseVersion must look like <major.minor.patch>-<YYYYMMDD>, got None
  Nodegroup created with amiType=AL2023_x86_64_STANDARD, instanceTypes=['t3.small'], scalingConfig={minSize:0,maxSize:1,desiredSize:0} and no launch template; it reached ACTIVE and its amiType and version (== the cluster version) both round-tripped, so only releaseVersion is absent.

Cause 5 — eks.cluster-version-update-in-progress-keeps-old-version-and-refuses-second [mismatch]:
  Error: Check timed out after 60.0s — no assertion failure. The check creates a cluster at the older of two catalogued versions, waits for ACTIVE (reached), calls UpdateClusterVersion to the newer version, and then polls DescribeCluster for status != ACTIVE (its own bound is _STATUS_SETTLE_S = 180). It never observes a non-ACTIVE status, so the subsequent assertions — DescribeCluster reporting the OLD version with status UPDATING, the second UpdateClusterVersion being refused ('ResourceInUseException', 'Cannot VersionUpdate because cluster {cluster} currently has update {update_id} in progress'), ListUpdates == [the one update id], and the final Successful/new-version/ACTIVE reads — are never reached.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — DeleteCluster DELETE /clusters/{name}, errors ClientException 400, InvalidRequestException 400, ResourceInUseException 409, ResourceNotFoundException 404, ServerException 500, ServiceUnavailableException 503; CreateCluster POST /clusters with computeConfig -> ComputeConfigRequest, storageConfig -> StorageConfigRequest, kubernetesNetworkConfig -> KubernetesNetworkConfigRequest, errors incl. InvalidParameterException 400 and ResourceInUseException 409; CreateFargateProfile POST /clusters/{clusterName}/fargate-profiles, errors ClientException/InvalidParameterException/InvalidRequestException/ResourceLimitExceededException/UnsupportedAvailabilityZoneException 400; DescribeNodegroup GET /clusters/{clusterName}/node-groups/{nodegroupName} -> Nodegroup, whose releaseVersion is documented as the EKS-optimized AMI version the node group was deployed with (or the launch template's AMI id); UpdateClusterVersion POST /clusters/{name}/updates -> Update, errors incl. InvalidStateException 400 and ResourceInUseException 409.

## Steps to Reproduce
1. git_clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run the five check ids in trace_ids.
3. Manually, against the emulator's EKS endpoint:
   a. Cause 1: CreateCluster and wait for ACTIVE, then issue two concurrent DeleteCluster calls for the same name. Expect one 200 with cluster.status DELETING and the loser refused 409 ResourceInUseException (not 404 ResourceNotFoundException); then expect a repeat DeleteCluster to be idempotent (200, DELETING, same ARN) and CreateCluster on the name to stay refused 409 ResourceInUseException 'Cluster already exists with name: <name>'.
   b. Cause 2: CreateCluster(computeConfig={enabled:true}) with no storageConfig and no kubernetesNetworkConfig.elasticLoadBalancing (and again with only two of the three enabled). Expect 400 InvalidParameterException with exactly 'For EKS Auto Mode, please ensure that all required configs, including computeConfig, kubernetesNetworkConfig, and blockStorage are all either fully enabled or fully disabled.'
   c. Cause 3: on an ACTIVE cluster whose subnets are public, CreateFargateProfile with a pod execution role trusting eks-fargate-pods.amazonaws.com. Expect a 400 refusal (InvalidParameterException 'Subnet <subnetId> provided in Fargate Profile is not a private subnet') and ListFargateProfiles to stay empty — not a 200.
   d. Cause 4: CreateNodegroup with amiType=AL2023_x86_64_STANDARD and no launch template, wait for ACTIVE, then DescribeNodegroup. Expect releaseVersion present and shaped <major>.<minor>.<patch>-<YYYYMMDD>, with <major>.<minor> equal to the nodegroup's Kubernetes version and the trailing component a valid calendar date.
   e. Cause 5: on an ACTIVE cluster at version N, call UpdateClusterVersion to version N+1. Expect the returned update to be type VersionUpdate / status InProgress with params Version=N+1; then DescribeCluster must report status UPDATING while still reporting version N, a second UpdateClusterVersion must be refused 409 ResourceInUseException 'Cannot VersionUpdate because cluster <name> currently has update <updateId> in progress', ListUpdates must contain only the first update id, and after the update reaches Successful DescribeCluster must report version N+1 and ACTIVE.
