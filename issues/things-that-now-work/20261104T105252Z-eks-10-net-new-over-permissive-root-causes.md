Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.access-entry-invalid-type-username-combo-rejected eks.create-cluster-auto-mode-requires-matching-config eks.create-cluster-service-ipv4-cidr-rejected eks.ekscc-create-cluster-with-both-endpoints-disabled-rejected eks.ekscc-create-cluster-with-invalid-public-access-cidr-rejected eks.ekscc-create-cluster-with-unsupported-version-rejected eks.ekscc-create-cluster-standard-support-on-extended-version-rejected eks.ekscc-create-cluster-with-vpc-overlapping-service-cidr-rejected eks.cluster-delete-blocked-by-dependents-rejected eks.delete-cluster-blocked-by-nodegroup-dependency eks.delete-cluster-with-active-nodegroup-rejected eks.e2e-cluster-nodegroup-pod-identity-teardown eks.e2e-cluster-nodegroup-podidentity-teardown-flow eks.concurrent-cluster-delete-conflict
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.access-entry-invalid-type-username-combo-rejected|_expect_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: eks.create-cluster-auto-mode-requires-matching-config|_stable_error#8|AssertionError|raise AssertionError("expected CreateCluster to be refused, but it succeeded")
Issue-Key: eks.create-cluster-service-ipv4-cidr-rejected|_stable_error#6|AssertionError|raise AssertionError("expected CreateCluster to be refused, but it succeeded")
Issue-Key: eks.ekscc-create-cluster-with-both-endpoints-disabled-rejected|_expect_error#5|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: eks.ekscc-create-cluster-with-invalid-public-access-cidr-rejected|_expect_error#5|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: eks.ekscc-create-cluster-with-unsupported-version-rejected|_error#5|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: eks.ekscc-create-cluster-standard-support-on-extended-version-rejected|_expect_error#9|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: eks.ekscc-create-cluster-with-vpc-overlapping-service-cidr-rejected|_error#6|AssertionError|raise AssertionError(f"the call SUCCEEDED with {result!r}; real AWS rejects it")
Issue-Key: eks.cluster-delete-blocked-by-dependents-rejected|_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: eks.delete-cluster-blocked-by-nodegroup-dependency|_error#8|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: eks.delete-cluster-with-active-nodegroup-rejected|_error#11|AssertionError|raise AssertionError("expected the call to fail, but it succeeded")
Issue-Key: eks.e2e-cluster-nodegroup-pod-identity-teardown|_error#18|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: eks.e2e-cluster-nodegroup-podidentity-teardown-flow|_error_code#19|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: eks.concurrent-cluster-delete-conflict|run#11|AssertionError|assert code == "ResourceInUseException", (
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# eks: 10 net-new over-permissive root causes — CreateCluster accepts eight request classes AWS refuses, and DeleteCluster destroys a cluster that still has nodegroups

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Ten net-new root causes where the emulator ACCEPTS (HTTP 200) a request real EKS refuses, or refuses a concurrent delete with the wrong class. Over-permissive control-plane behavior is the most damaging kind of divergence: a template or script validated against the emulator fails against AWS, and in Cause 9 the emulator actually tears down a cluster AWS protects. All names/ids/CIDRs below are template slots taken from the request; only the fixed text of each refusal is the requirement.

Cause 1 of 10: CreateAccessEntry accepts a username together with a non-STANDARD type.
  AWS: setting username is not allowed when type is EC2_LINUX; the call is refused InvalidParameterException 400 with 'The specified username is invalid: setting username is not allowed when the type is "EC2_LINUX".' — fixed text with the rejected type as the only variable part — and no access entry is left behind (DescribeAccessEntry then answers ResourceNotFoundException 'The requested resource does not exist.').
  Emulator: HTTP 200, returning an accessEntry with type 'EC2_LINUX' and username 'not-allowed'.
  Why: the caller stores an access entry AWS would never create, and the invalid username silently becomes part of cluster authentication state.
  Fix: CreateAccessEntry's validation — reject username for any server-managed type.
  Traces: eks.access-entry-invalid-type-username-combo-rejected.

Cause 2 of 10: CreateCluster accepts a partial EKS Auto Mode configuration.
  AWS: Auto Mode requires computeConfig, storageConfig.blockStorage and kubernetesNetworkConfig.elasticLoadBalancing to be enabled together, and a computeConfig with nodePools requires nodeRoleArn; each mismatch is refused InvalidParameterException 400 and leaves no cluster (DescribeCluster then answers 'No cluster found for name: <name>.').
  Emulator: HTTP 200 for computeConfig={'enabled': True} alone, and for the other partial combinations.
  Why: a half-configured Auto Mode cluster cannot exist in AWS; the caller gets a cluster whose compute/storage/load-balancing settings are mutually inconsistent.
  Fix: CreateCluster's Auto Mode cross-member validation.
  Traces: eks.create-cluster-auto-mode-requires-matching-config.

Cause 3 of 10: CreateCluster accepts an invalid kubernetesNetworkConfig.serviceIpv4Cidr.
  AWS: three refusals, all InvalidParameterException 400 — a non-RFC1918 CIDR: 'Invalid Cluster Service Ipv4 CIDR <cidr> for the cluster <name>, must be within RFC1918 range'; a netmask outside /12../24: 'Invalid Cluster Service Ipv4 CIDR <cidr> for the cluster <name>, must have netmask size between /12 and /24 inclusive'; serviceIpv4Cidr with ipFamily=ipv6: 'serviceIpv4Cidr cannot be specified when creating an "ipv6" cluster'. The fixed text is everything but the submitted CIDR and the cluster name.
  Emulator: HTTP 200 for all three.
  Why: the cluster's service CIDR is unchangeable after create; accepting an illegal one produces a cluster whose networking cannot exist in AWS.
  Fix: CreateCluster's kubernetesNetworkConfig validation.
  Traces: eks.create-cluster-service-ipv4-cidr-rejected.

Cause 4 of 10: CreateCluster accepts both API endpoints disabled.
  AWS: endpointPublicAccess=false together with endpointPrivateAccess=false is refused InvalidParameterException 400 with 'Private and public endpoint access cannot be false' (all fixed text), leaving no cluster.
  Emulator: HTTP 200.
  Why: the emulator creates a cluster with no reachable Kubernetes API endpoint — a configuration AWS forbids precisely because it is unusable.
  Fix: CreateCluster's resourcesVpcConfig endpoint validation.
  Traces: eks.ekscc-create-cluster-with-both-endpoints-disabled-rejected.

Cause 5 of 10: CreateCluster accepts a malformed publicAccessCidrs entry.
  AWS: a syntactically invalid CIDR is refused InvalidParameterException 400 with 'The following CIDRs are invalid in publicAccessCidrs: [<cidr>, ...]' — fixed text 'The following CIDRs are invalid in publicAccessCidrs: [' plus the offending entries from the request.
  Emulator: HTTP 200 (e.g. publicAccessCidrs ['300.1.1.1/32']).
  Why: the caller believes an unparseable allow-list was applied to the public endpoint; against AWS the same template fails.
  Fix: CreateCluster's publicAccessCidrs validation.
  Traces: eks.ekscc-create-cluster-with-invalid-public-access-cidr-rejected.

Cause 6 of 10: CreateCluster accepts an unsupported Kubernetes version.
  AWS: an unsupported version (e.g. 1.10) is refused InvalidParameterException 400 with 'unsupported Kubernetes version <version>' — fixed lower-case text plus the rejected version — and leaves no cluster.
  Emulator: HTTP 200.
  Why: the emulator will serve a control plane at a version EKS does not offer, so version-pinning logic validated here is wrong everywhere else.
  Fix: CreateCluster's version validation against the supported-version catalogue.
  Traces: eks.ekscc-create-cluster-with-unsupported-version-rejected.

Cause 7 of 10: CreateCluster accepts upgradePolicy.supportType=STANDARD on a version that is only in extended support.
  AWS: refused InvalidParameterException 400 with 'The requested Kubernetes version "<version>" is only supported under extended support. Please change the supportType to extended support to create the cluster.' — fixed text with the requested version as the variable part.
  Emulator: HTTP 200.
  Why: support type drives both upgrade obligations and billing; accepting the combination hides a decision AWS forces the caller to make.
  Fix: CreateCluster's upgradePolicy/version cross-validation.
  Traces: eks.ekscc-create-cluster-standard-support-on-extended-version-rejected.

Cause 8 of 10: CreateCluster accepts a serviceIpv4Cidr that overlaps the cluster VPC's CIDR.
  AWS: refused InvalidParameterException 400 with 'Invalid Cluster Service Ipv4 CIDR <cidr> for the cluster <name>, overlaps with VPC range <vpcCidr>' — fixed text with the submitted CIDR, the cluster name and the VPC range as variables.
  Emulator: HTTP 200 (returned a CREATING cluster with kubernetesNetworkConfig.serviceIpv4Cidr equal to the VPC CIDR).
  Why: pod and service address space would collide; AWS refuses the cluster rather than create something unroutable.
  Fix: CreateCluster's kubernetesNetworkConfig validation — compare against the resolved VPC CIDR.
  Traces: eks.ekscc-create-cluster-with-vpc-overlapping-service-cidr-rejected.

Cause 9 of 10: DeleteCluster succeeds while the cluster still has nodegroups.
  AWS: Smithy: DeleteCluster declares ResourceInUseException (httpError 409). With a nodegroup attached the call is refused ResourceInUseException with 'Cluster has nodegroups attached' — the entire sentence is fixed text — and the cluster stays ACTIVE. Only after the nodegroups are gone is DeleteCluster accepted.
  Emulator: HTTP 200, returning the cluster with status 'DELETING' while its nodegroups still exist.
  Why: this is destructive. The dependency guard that protects a cluster with live nodes is absent, so a teardown ordering bug that AWS refuses silently destroys the cluster here, and the orphaned nodegroups' state becomes unreachable.
  Fix: DeleteCluster's dependent-resource guard (nodegroups, and the same guard path for other cluster children).
  Traces: eks.cluster-delete-blocked-by-dependents-rejected, eks.delete-cluster-blocked-by-nodegroup-dependency, eks.delete-cluster-with-active-nodegroup-rejected, eks.e2e-cluster-nodegroup-pod-identity-teardown, eks.e2e-cluster-nodegroup-podidentity-teardown-flow.
  
Cause 10 of 10: the losing side of two concurrent DeleteCluster calls gets ResourceNotFoundException instead of ResourceInUseException.
  AWS: when two DeleteCluster calls race, one wins and the other is refused ResourceInUseException (409) because the cluster is already being deleted — the resource still exists, it is busy.
  Emulator: ResourceNotFoundException (404).
  Why: a 404 tells the caller the cluster was never there, so idempotent teardown logic that distinguishes 'already deleting' from 'never existed' records the wrong outcome.
  Fix: DeleteCluster's state transition — a cluster already in DELETING is in-use, not absent.
  Traces: eks.concurrent-cluster-delete-conflict.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (service eks 142/306 passed).

Cause 1: eks.access-entry-invalid-type-username-combo-rejected — 'expected the call to be rejected, but it SUCCEEDED and returned {... HTTPStatusCode: 200, accessEntry: {... username: not-allowed, type: EC2_LINUX}}'; the check requires ('InvalidParameterException', 'The specified username is invalid: setting username is not allowed when the type is "EC2_LINUX".').
Cause 2: eks.create-cluster-auto-mode-requires-matching-config — 'expected CreateCluster to be refused, but it succeeded' for computeConfig={'enabled': True}; the check requires InvalidParameterException for each partial combination.
Cause 3: eks.create-cluster-service-ipv4-cidr-rejected — 'expected CreateCluster to be refused, but it succeeded'; the check requires InvalidParameterException with 'Invalid Cluster Service Ipv4 CIDR 8.8.0.0/16 for the cluster <name>, must be within RFC1918 range', '... must have netmask size between /12 and /24 inclusive' and 'serviceIpv4Cidr cannot be specified when creating an "ipv6" cluster'.
Cause 4: eks.ekscc-create-cluster-with-both-endpoints-disabled-rejected — 'the call SUCCEEDED; real AWS rejects it'; required ('InvalidParameterException', 'Private and public endpoint access cannot be false').
Cause 5: eks.ekscc-create-cluster-with-invalid-public-access-cidr-rejected — 'the call SUCCEEDED; real AWS rejects it'; required ('InvalidParameterException', 'The following CIDRs are invalid in publicAccessCidrs: [300.1.1.1/32]').
Cause 6: eks.ekscc-create-cluster-with-unsupported-version-rejected — 'the call SUCCEEDED; real AWS rejects it, so there is no error code'; required InvalidParameterException 'unsupported Kubernetes version 1.10'.
Cause 7: eks.ekscc-create-cluster-standard-support-on-extended-version-rejected — 'the call SUCCEEDED; real AWS rejects it'; required InvalidParameterException 'The requested Kubernetes version "<version>" is only supported under extended support. Please change the supportType to extended support to create the cluster.'
Cause 8: eks.ekscc-create-cluster-with-vpc-overlapping-service-cidr-rejected — 'the call SUCCEEDED with {... serviceIpv4Cidr: 172.31.0.0/16 ... status: CREATING}'; required InvalidParameterException 'Invalid Cluster Service Ipv4 CIDR <cidr> for the cluster <name>, overlaps with VPC range <cidr>'.
Cause 9: eks.cluster-delete-blocked-by-dependents-rejected — 'expected a ClientError, but the call succeeded and returned {... HTTPStatusCode: 200 ... "status": "DELETING"}'; the sibling checks assert code ResourceInUseException and message 'Cluster has nodegroups attached' with the cluster remaining ACTIVE.
Cause 10: eks.concurrent-cluster-delete-conflict — 'a losing concurrent DeleteCluster must be ResourceInUseException, got ResourceNotFoundException'.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreateCluster (POST /clusters) errors ClientException 400, InvalidParameterException 400, ResourceInUseException 409, ResourceLimitExceededException 400, UnsupportedAvailabilityZoneException 400; DeleteCluster (DELETE /clusters/{name}) errors ClientException 400, InvalidRequestException 400, ResourceInUseException 409, ResourceNotFoundException 404; CreateAccessEntry errors include InvalidParameterException 400.

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run the check ids in trace_ids.
3. Manually, against the emulator's EKS endpoint, every one of these must be refused and must leave no cluster/entry behind:
   a. CreateAccessEntry(type='EC2_LINUX', username='not-allowed').
   b. CreateCluster(computeConfig={'enabled': true}) with no storageConfig and no kubernetesNetworkConfig.elasticLoadBalancing.
   c. CreateCluster(kubernetesNetworkConfig={'serviceIpv4Cidr': '8.8.0.0/16'}), then '10.100.0.0/28', then serviceIpv4Cidr together with ipFamily='ipv6'.
   d. CreateCluster(resourcesVpcConfig={'endpointPublicAccess': false, 'endpointPrivateAccess': false, ...}).
   e. CreateCluster(resourcesVpcConfig={'publicAccessCidrs': ['300.1.1.1/32'], ...}).
   f. CreateCluster(version='1.10'); CreateCluster(version=<extended-support-only version>, upgradePolicy={'supportType': 'STANDARD'}).
   g. CreateCluster(kubernetesNetworkConfig={'serviceIpv4Cidr': <the VPC's own CIDR>}).
   h. Create a cluster with a nodegroup, then DeleteCluster -> expect 409 ResourceInUseException 'Cluster has nodegroups attached' and the cluster still ACTIVE.
   i. Issue two DeleteCluster calls concurrently for one ACTIVE cluster -> the loser must be 409 ResourceInUseException.

## Resolution note

All ten causes are implemented in CreateCluster, UpdateClusterConfig, CreateAccessEntry,
UpdateAccessEntry and DeleteCluster, and 11 of this report's 14 behavioural keys pass.
Three legs of the replay stay open and are NOT closed by this report:

* Cause 2's MESSAGE. The refusal (400 InvalidParameterException, no cluster written) is in
  place for every partial EKS Auto Mode combination, but the sentence the check compares
  against is not reported by the replay and is not quoted in this report, so the sentence
  the service states ("All of computeConfig.enabled,
  kubernetesNetworkConfig.elasticLoadBalancing.enabled, and storageConfig.blockStorage.enabled
  must be set to either true or false.") still differs from it. `eks.create-cluster-auto-mode-requires-matching-config`
  remains failing on that comparison alone and should be re-sourced when a recording of the
  message is available.
* `eks.delete-cluster-blocked-by-nodegroup-dependency` now fails EARLIER than the delete this
  report is about, on `CreateFargateProfile` accepting public subnets — Cause 3 of
  `issues/things-that-do-not-work/20261104T105252Z-eks-10-net-new-root-causes-unvalidated-pod.md`,
  which owns that validation.
* Cause 10's unit, `eks.concurrent-cluster-delete-conflict`. A delete that OVERLAPS an
  accepted deletion is now refused 409 ResourceInUseException, and one that arrives after
  that deletion finished is 404 (which two other units of this report require). The replay's
  two "concurrent" deletes do not overlap in this emulator — the losing call reaches the
  handler only after the winning one finished its teardown — so that unit still records 404
  for its loser, and the guard is pinned by `tests/all/cluster_admission_rules.rs` instead.
