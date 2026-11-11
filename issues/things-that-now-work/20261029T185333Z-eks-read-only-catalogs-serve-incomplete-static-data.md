Issue-Source: trace-sweep-divergence
Issue-Trace: eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator eks-catalog-describe-addon-versions-eks-node-monitoring-agent-vpc-cni eks-eks-catalog-read-only-discovery eks-catalog-describe-cluster-versions eks-catalog-list-access-policies
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeAddonVersions|2xx->2xx|$.addons[*]
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*]
Issue-Key: eks|ListAccessPolicies|2xx->2xx|$.accessPolicies[arn="arn
Issue-Key: eks|DescribeAddonConfiguration|2xx->2xx|$.configurationSchema
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# eks read-only catalogs serve incomplete static data across five readers

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
FIVE net-new root causes in EKS's read-only catalogs. Every one answers 200 with the right envelope but with reference data the real service serves and the emulator's seed does not, so the divergence is in the static catalog each reader projects. All five must be closed before this issue is resubmitted. (Earlier catalog gaps were filed and closed as 'EKS read-only catalog operations serve an incomplete static catalog (3 causes)' and 'eks: 3 net-new root causes — the add-on catalog omits cert-manager and the HyperPod training operator, DescribeAddonConfiguration serves a placeholder'; those cover different catalog entries and do not cover the entries below.)

Cause 1 of 5: DescribeAddonVersions omits AWS Marketplace-owned add-ons entirely — the largest cluster in this replay (85 steps).
AWS: Smithy: DescribeAddonVersions GET /addons/supported-versions -> addons (list of AddonInfo), paginated, filterable by addonName/types/publishers/owners/kubernetesVersion. AWS's catalog contains Marketplace add-ons whose AddonInfo carries addonName, type, publisher, owner 'aws-marketplace', marketplaceInformation {productId, productUrl}, defaultNamespace, and an addonVersions list where each AddonVersionInfo carries addonVersion, architecture, computeTypes, compatibilities (clusterVersion / platformVersions / defaultVersion per supported Kubernetes minor), requiresConfiguration and requiresIamPermissions. The recording shows a filtered read for such a name (e.g. the AccuKnox KubeArmor and EG Innovations universal-agent-operator add-ons) returning exactly that one populated entry.
Emulator: answers 200 with an empty addons list ($.addons[0] missing) for those names, across all 85 affected steps of the fixture.
Why: a client discovering, version-pinning or installing a Marketplace add-on sees it as nonexistent, so every subsequent CreateAddon/UpdateAddon decision keyed to the catalog is impossible; the marketplaceInformation and defaultNamespace members a subscriber needs are never observable.
Fix: the static add-on catalog seed behind DescribeAddonVersions in the eks service (the data, plus its owner/publisher filter paths), not the serializer.
Traces: eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator#task#1 (85 steps in the cluster).

Cause 2 of 5: DescribeClusterVersions does not serve the newest Kubernetes versions, so the head of the list is absent.
AWS: Smithy: DescribeClusterVersions GET /cluster-versions -> clusterVersions (ClusterVersionList), paginated. Each ClusterVersion carries clusterVersion, clusterType, defaultPlatformVersion, defaultVersion, releaseDate, endOfStandardSupportDate, endOfExtendedSupportDate, status and versionStatus, kubernetesPatchVersion, and controlPlaneScalingTiers — one entry per tier with tierName, apiRequestConcurrency, podSchedulingRatePerSecond, clusterDatabaseSizeGb and controlPlaneComponentConfigOverrides (kubeApiServer / kubeScheduler / kubeControllerManager sub-configs with defaultValue plus min/max constraints).
Emulator: answers 200 with a shorter list — the first seven elements the recording returns ($.clusterVersions[0] through [6]) are missing, i.e. the newest supported minors and their scaling-tier descriptions are not in the seed at all.
Why: a client that picks a cluster version from this catalog (or reads a tier's concurrency limits) cannot see currently supported versions, so cluster creation and upgrade planning are driven off a stale version set.
Fix: the cluster-version catalog seed behind DescribeClusterVersions, including per-version controlPlaneScalingTiers.
Traces: eks-catalog-describe-cluster-versions#task#1.

Cause 3 of 5: ListAccessPolicies omits the Amazon SageMaker HyperPod cluster access policies.
AWS: Smithy: ListAccessPolicies GET /access-policies -> accessPolicies (AccessPolicy: name + arn), paginated. AWS publishes the managed cluster access policies as arn:aws:eks::aws:cluster-access-policy/<name> with name equal to the policy's name; the recording includes the HyperPod family (AmazonSagemakerHyperpodSystemNamespacePolicy, AmazonSagemakerHyperpodTrainingPolicy, AmazonSagemakerHyperpodUserClusterPolicy).
Emulator: answers 200 but those three entries are absent from accessPolicies.
Why: AssociateAccessPolicy takes a policy ARN from this catalog, so a HyperPod-style access entry cannot be discovered or validated against the published set.
Fix: the managed access-policy catalog seed behind ListAccessPolicies.
Traces: eks-catalog-list-access-policies#task#1.

Cause 4 of 5: the same cluster-version catalog is also wrong at the head of the list on the discovery path — clusterVersions[0] is not the version AWS serves there.
AWS: as cause 1's model citation; the recording's first element is the newest minor, carrying its platform version, support dates, patch version and full controlPlaneScalingTiers list.
Emulator: answers 200 with a different record at $.clusterVersions[0] (value_mismatch rather than a short list), so an unfiltered discovery read is served a different version at the head and without that entry's scaling tiers.
Why: callers that take the first element as the newest/default version select a version AWS does not offer there. Shares the catalog with cause 2 but is a distinct replay unit and must be re-replayed separately.
Fix: the cluster-version catalog seed and its ordering for the unfiltered read.
Traces: eks-eks-catalog-read-only-discovery#task#1.

Cause 5 of 5: DescribeAddonConfiguration serves a placeholder configurationSchema instead of the add-on's real JSON Schema.
AWS: Smithy: DescribeAddonConfiguration GET /addons/configuration-schemas -> configurationSchema (String). AWS returns the add-on's actual JSON Schema document — a draft-06 schema ($schema http://json-schema.org/draft-06/schema#) with a $ref into its own definitions and the add-on's real configurable surface (for the VPC CNI add-on: definitions for VpcCni, EniConfig, Affinity and the full Env property set with per-variable type/format/enum constraints).
Emulator: answers 200 with a configurationSchema that does not match that document, so the schema a client validates its add-on configuration against is not the add-on's schema.
Why: any client that validates or renders add-on configuration from this schema accepts configuration AWS rejects, or rejects configuration AWS accepts.
Fix: the per-add-on configuration-schema seed behind DescribeAddonConfiguration.
Traces: eks-eks-catalog-read-only-discovery#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (40 fixtures, 232 steps, 112 passed, 120 mismatched, score 48.3%).

1. DescribeAddonVersions, eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator#task#1 — expected 200 with $.addons[0] = the requested Marketplace add-on (addonName/type/publisher/owner 'aws-marketplace'/marketplaceInformation/defaultNamespace plus a populated addonVersions list); actual 200 with $.addons[0] missing. 85 steps in the cluster.
2. DescribeClusterVersions, eks-catalog-describe-cluster-versions#task#1 — expected 200 with clusterVersions[0..6] present (newest minors incl. defaultPlatformVersion, support dates, kubernetesPatchVersion, controlPlaneScalingTiers); actual 200 with all seven missing.
3. ListAccessPolicies, eks-catalog-list-access-policies#task#1 — expected 200 including arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpod{SystemNamespace,Training,UserCluster}Policy; actual 200 with all three missing.
4. DescribeClusterVersions, eks-eks-catalog-read-only-discovery#task#1 — expected 200 whose clusterVersions[0] is the newest minor with its controlPlaneScalingTiers; actual 200 with a different record at that index (value_mismatch).
5. DescribeAddonConfiguration, eks-eks-catalog-read-only-discovery#task#3 — expected 200 whose $.configurationSchema is the add-on's real draft-06 JSON Schema ($ref into definitions, VpcCni/EniConfig/Affinity/Env definitions); actual 200 with a non-matching schema document.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: GET /addons/supported-versions?addonName=<a Marketplace add-on such as accuknox_kubearmor> (fixture eks-catalog-describe-addon-versions-accuknox-kubearmor-eg-innovations-eg-universal-agent-operator, step task#1) and inspect $.addons.
3. Cause 2: GET /cluster-versions (eks-catalog-describe-cluster-versions#task#1) and compare the head of $.clusterVersions.
4. Cause 3: GET /access-policies (eks-catalog-list-access-policies#task#1) and look for the HyperPod cluster-access-policy ARNs.
5. Cause 4: GET /cluster-versions on the discovery fixture (eks-eks-catalog-read-only-discovery#task#1) and compare $.clusterVersions[0].
6. Cause 5: GET /addons/configuration-schemas?addonName=vpc-cni&addonVersion=<recorded version> (eks-eks-catalog-read-only-discovery#task#3) and compare $.configurationSchema with the add-on's published schema.
