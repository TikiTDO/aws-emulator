Issue-Source: trace-sweep-divergence
Issue-Trace: eks-ekscc-traj-create-request-shape-mistakes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-ekscc-traj-create-request-shape-mistakes eks-ekscc-traj-vpc-config-mistakes eks-ekscc-traj-cluster-lookup-mistakes eks-ekscc-traj-reference-create-retry-and-delete eks-ekscc-traj-version-and-support-mistakes
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateCluster|4xx->4xx|$.clusterName
Issue-Key: eks|CreateCluster|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateCluster|4xx->4xx|status
Issue-Key: eks|ListClusters|4xx->4xx|$.message
Issue-Key: eks|DescribeCluster|4xx->4xx|$.message
Issue-Key: eks|DescribeCluster|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|DescribeCluster|4xx->4xx|status
Issue-Key: eks|CreateCluster|4xx->2xx|$.clusterName
Issue-Key: eks|CreateCluster|4xx->2xx|$.message
Issue-Key: eks|CreateCluster|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateCluster|4xx->2xx|status
Issue-Key: eks|DescribeClusterVersions|4xx->2xx|$.message
Issue-Key: eks|DescribeClusterVersions|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: eks|DescribeClusterVersions|4xx->2xx|status
Issue-Key: eks|DescribeClusterVersions|4xx->4xx|$.message
Issue-Related: 20261029T185333Z-eks-refusal-messages-diverge-from-aws-wording-across.md
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EKS cluster-scope request validation: missing checks, wrong refusal class and generic framework wording

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Nine net-new root causes share one fault: the cluster-scope operations (CreateCluster, DescribeCluster, ListClusters, DescribeClusterVersions) do not implement EKS's own request validation. Where EKS refuses, the emulator either accepts the request with 200, or refuses it with the generic Smithy-constraint framework sentence instead of the EKS message, or classifies the refusal as the wrong exception.

The already-open issue 20261029T175608Z-eks-3-net-new-over-permissive-root-causes.md covers RegisterCluster accepting a duplicate cluster name and IAM authorization never being enforced; it owns those replay keys, not these. Cause 4 below is the same gap on CreateCluster, which that issue does not name.

Cause 1 of 9: an invalid cluster name is refused with the framework's constraint sentence instead of EKS's.
  AWS: CreateCluster.name is @length 1..100 @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$ (Smithy: CreateCluster.name -> ClusterName, required). A violation is refused with 400 and the message 'The parameter name contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.' — all fixed text except the member name 'name'. The error body also echoes the offending value in a clusterName member.
  Emulator: 400 with "1 validation error detected: Value '<value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z][A-Za-z0-9\\-_]*$" and no clusterName member.
  Why: a client matching on EKS's documented message (or surfacing it to an operator) sees an unrecognised string, and automation keyed on the clusterName member of the error body finds nothing there.
  Fix: CreateCluster's validation must emit the EKS-worded refusal rather than the generic constraint report, and its error serializer must populate clusterName.
  Traces: eks-ekscc-traj-create-request-shape-mistakes#task#6 (also #task#7, eks-ekscc-traj-vpc-config-mistakes#task#5).

Cause 2 of 9: a body-shape parse failure leaks the raw deserializer message as a ValidationException.
  AWS: for this request EKS refuses with HTTP 403 and 'User: <principalArn> is not authorized to perform: eks:CreateCluster' — fixed text 'User: ' ... ' is not authorized to perform: eks:CreateCluster' with the caller's principal ARN as the only variable part. Authorization is evaluated before the body is deserialized into shapes.
  Emulator: HTTP 400, x-amzn-errortype 'ValidationException', message 'expected array start, got string' — an internal JSON-deserializer diagnostic that is not an AWS message at all and names no member.
  Why: a client cannot act on 'expected array start, got string': it names neither the member at fault nor the permission it lacks, and ValidationException is not even a modeled CreateCluster error (Smithy: CreateCluster errors are ClientException, InvalidParameterException, ResourceInUseException, ResourceLimitExceededException, ServerException, ServiceUnavailableException, UnsupportedAvailabilityZoneException).
  Fix: CreateCluster's handler must evaluate authorization before deserialization, and its deserializer must never surface a raw parser diagnostic as a modeled fault.
  Traces: eks-ekscc-traj-create-request-shape-mistakes#task#10.

Cause 3 of 9: ListClusters' maxResults refusal uses the framework sentence instead of EKS's.
  AWS: ListClusters.maxResults is @range 1..100 (Smithy: ListClusters.maxResults -> ListClustersRequestMaxResults, httpQuery). An out-of-range value is refused with 400 and 'maxResults needs to be between 1 and 100' — fixed text whose bounds come from the model.
  Emulator: "1 validation error detected: Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
  Why: a paginating client that matches EKS's message to adjust its page size does not recognise the emulator's, so it cannot self-correct.
  Fix: ListClusters' validation — emit the EKS-worded range refusal.
  Traces: eks-ekscc-traj-cluster-lookup-mistakes#task#1 (also #task#2).

Cause 4 of 9: DescribeCluster looks up a malformed cluster name instead of rejecting it.
  AWS: although DescribeCluster's `name` label is an unconstrained String in the model (Smithy: DescribeCluster.name -> String, httpLabel, required), EKS enforces the cluster-name grammar on it and refuses a value that is not a bare cluster name with HTTP 400 and 'The parameter name contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.' (recorded behavior; the model does not state the constraint for this member).
  Emulator: HTTP 404, x-amzn-errortype 'ResourceNotFoundException', message 'No cluster found for name: <value>.'
  Why: a client that passed a cluster ARN where a name belongs is told the cluster does not exist, so it tries to create it instead of correcting the identifier.
  Fix: DescribeCluster's validation — reject a name that does not match the cluster-name grammar before any state lookup.
  Traces: eks-ekscc-traj-cluster-lookup-mistakes#task#3.

Cause 5 of 9: CreateCluster accepts a duplicate cluster name with 200 instead of refusing it.
  AWS: CreateCluster for a name already in use refuses with HTTP 409 ResourceInUseException and 'Cluster already exists with name: <name>' — fixed text 'Cluster already exists with name: ' plus the requested name. The error body also carries a clusterName member. Smithy: CreateCluster error ResourceInUseException, httpError 409.
  Emulator: HTTP 200 — the duplicate is created, with no message and no x-amzn-errortype.
  Why: a retrying client that would have treated 409 as 'already created, carry on' instead gets a second cluster record; two clusters now share a name and later lookups are ambiguous. This is a silent data-integrity break, not just a wording difference.
  Fix: CreateCluster's handler must detect the name collision against the EKS state store and raise ResourceInUseException.
  Traces: eks-ekscc-traj-reference-create-retry-and-delete#task#6.

Cause 6 of 9: CreateCluster accepts an unsupported Kubernetes version with 200.
  AWS: CreateCluster refuses a version string that is not a supported EKS control-plane version with HTTP 400 and 'unsupported Kubernetes version <version>' — fixed text 'unsupported Kubernetes version ' plus the requested version from the request. The error body also carries clusterName. Smithy: CreateCluster.version : String (the supported set is not enumerated in the model, so the value set is recorded behavior).
  Emulator: HTTP 200 — the cluster is created on a version EKS does not offer, and $.message / x-amzn-errortype are absent.
  Why: a client that mistypes a version (for example a three-part patch version where EKS takes major.minor) gets a cluster that cannot exist on AWS, so the emulator validates nothing and the mistake surfaces only in production.
  Fix: CreateCluster's validation — check version against the supported cluster-version catalog.
  Traces: eks-ekscc-traj-version-and-support-mistakes#task#3 (also #task#4, #task#5, #task#9, eks-ekscc-traj-vpc-config-mistakes#task#6, #task#7).

Cause 7 of 9: DescribeClusterVersions accepts a malformed clusterVersions list with 200.
  AWS: DescribeClusterVersions.clusterVersions is a httpQuery StringList (Smithy: DescribeClusterVersions.clusterVersions -> StringList, httpQuery). A value that is not a comma-separated list of versions is refused with HTTP 400 and 'Provided Cluster Versions are invalid, it must be comma separated versions ie 1.27,1.28' — entirely fixed text.
  Emulator: HTTP 200, with no message and no x-amzn-errortype.
  Why: the client's filter is silently ignored and it receives a result set it did not ask for, which it then treats as authoritative.
  Fix: DescribeClusterVersions' validation of the clusterVersions query parameter.
  Traces: eks-ekscc-traj-version-and-support-mistakes#task#6.

Cause 8 of 9: the DescribeClusterVersions status enum refusal uses the framework sentence instead of EKS's.
  AWS: DescribeClusterVersions.status targets ClusterVersionStatus with values unsupported, standard-support, extended-support (Smithy: ClusterVersionStatus enum). An out-of-set value is refused with HTTP 400 and 'Provided Cluster Status is invalid: <value>' — fixed text 'Provided Cluster Status is invalid: ' plus the rejected value from the request.
  Emulator: "1 validation error detected: Value '<value>' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [unsupported, standard-support, extended-support]".
  Why: a client matching EKS's message does not recognise the emulator's, and the emulator's framework sentence is a different error contract from the one EKS publishes for this parameter.
  Fix: DescribeClusterVersions' validation — emit the EKS-worded enum refusal.
  Traces: eks-ekscc-traj-version-and-support-mistakes#task#7.

Cause 9 of 9: CreateCluster accepts a malformed security group id with 200.
  AWS: CreateCluster validates resourcesVpcConfig.securityGroupIds and refuses an id that is not a security-group identifier with HTTP 400 and 'The request contains an invalid security group Id: <id>' — fixed text 'The request contains an invalid security group Id: ' plus the offending id from the request.
  Emulator: HTTP 200, with no message and no x-amzn-errortype — the cluster is created referencing a security group that cannot exist.
  Why: a client whose VPC wiring is wrong gets a cluster that looks healthy in the emulator and fails only against AWS; the emulator cannot be used to validate VPC configuration at all.
  Fix: CreateCluster's validation of resourcesVpcConfig.securityGroupIds.
  Traces: eks-ekscc-traj-vpc-config-mistakes#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — eks-ekscc-traj-create-request-shape-mistakes#task#6, CreateCluster, 400->400: expected $.clusterName 'sim2real probe ekscc e3d7c8ef' (got None) and $.message 'The parameter name contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.'; got "1 validation error detected: Value 'sim2real probe ekscc e3d7c8ef' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z][A-Za-z0-9\\-_]*$". 3 affected steps.
Cause 2 — eks-ekscc-traj-create-request-shape-mistakes#task#10, CreateCluster: expected status 403 and $.message 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: eks:CreateCluster'; got status 400, x-amzn-errortype 'ValidationException', $.message 'expected array start, got string'.
Cause 3 — eks-ekscc-traj-cluster-lookup-mistakes#task#1, ListClusters, 400->400: expected $.message 'maxResults needs to be between 1 and 100'; got "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". 2 affected steps.
Cause 4 — eks-ekscc-traj-cluster-lookup-mistakes#task#3, DescribeCluster: expected status 400 and the name-invalid-characters message; got status 404, x-amzn-errortype 'ResourceNotFoundException', $.message 'No cluster found for name: arn:aws:eks:us-west-2:000000000000:cluster/sim2real-probe-ekscc-look-byarn-9b065e1d.'
Cause 5 — eks-ekscc-traj-reference-create-retry-and-delete#task#6, CreateCluster: expected status 409, $.message 'Cluster already exists with name: sim2real-probe-ekscc-ref-cluster-ac5ef024', $.clusterName populated and x-amzn-errortype present; got status 200 with all three absent.
Cause 6 — eks-ekscc-traj-version-and-support-mistakes#task#3, CreateCluster: expected status 400, $.message 'unsupported Kubernetes version 1.33.0', $.clusterName 'sim2real-probe-ekscc-ver-dotpatch-d4fac84c', x-amzn-errortype present; got status 200 with all absent. 6 affected steps.
Cause 7 — eks-ekscc-traj-version-and-support-mistakes#task#6, DescribeClusterVersions: expected status 400 and $.message 'Provided Cluster Versions are invalid, it must be comma separated versions ie 1.27,1.28'; got status 200, no message.
Cause 8 — eks-ekscc-traj-version-and-support-mistakes#task#7, DescribeClusterVersions, 400->400: expected $.message 'Provided Cluster Status is invalid: standard_support'; got "1 validation error detected: Value 'standard_support' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [unsupported, standard-support, extended-support]".
Cause 9 — eks-ekscc-traj-vpc-config-mistakes#task#8, CreateCluster: expected status 400 and $.message 'The request contains an invalid security group Id: not-a-security-group-id'; got status 200, no message.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreateCluster.name (ClusterName, required, length 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$), CreateCluster errors (ClientException/InvalidParameterException/ResourceLimitExceededException/UnsupportedAvailabilityZoneException 400, ResourceInUseException 409, ServerException 500, ServiceUnavailableException 503 — no ValidationException), DescribeCluster.name (String, httpLabel, required), ListClusters.maxResults (range 1..100), DescribeClusterVersions.status -> ClusterVersionStatus enum [unsupported, standard-support, extended-support], DescribeClusterVersions.clusterVersions (StringList, httpQuery).

## Steps to Reproduce
1. git clone the project repo and build the emulator from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Representative divergence: fixture eks-ekscc-traj-create-request-shape-mistakes, step task#6; the other eight at the fixture#step pairs listed per cause.
4. Manually: POST /clusters with name 'sim2real probe ekscc' (spaces) -> the emulator returns the generic constraint sentence, not EKS's. POST /clusters twice with the same name -> both return 200. POST /clusters with version '1.33.0' -> 200 instead of 400 'unsupported Kubernetes version 1.33.0'. POST /clusters with resourcesVpcConfig.securityGroupIds=['not-a-security-group-id'] -> 200 instead of 400. GET /clusters?maxResults=101 and GET /cluster-versions?status=standard_support -> framework constraint sentences. GET /clusters/<a-cluster-ARN> -> 404 instead of 400.
