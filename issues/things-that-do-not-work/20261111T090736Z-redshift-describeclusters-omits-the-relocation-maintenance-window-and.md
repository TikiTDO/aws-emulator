Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-aqua-configuration-modify-transition
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-aqua-configuration-modify-transition redshift-create-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.Endpoint.Address
Issue-Related: 20261029T192501Z-redshift-read-payloads-omit-modeled-members-and-tagging.md
Issue-Commit: 34a59b28db3b6549e829977bc135282939da6978

# Redshift DescribeClusters omits the relocation, maintenance-window and AZ-DR cluster members

> _Note: this issue was identified against the code base at commit `34a59b28db3b6549e829977bc135282939da6978`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
1 root cause. DescribeClusters' serialized Cluster element drops three members that AWS emits on every cluster it describes.

Cause 1 of 1: DescribeClusters omits Cluster.AvailabilityZoneRelocation, Cluster.IsMaintenanceWindowActivated and Cluster.AzDisasterRecoveryStatus.
AWS: recorded behavior only — the on-disk Smithy model (services/service-com-amazonaws-redshift-redshiftserviceversion20121201, shape com.amazonaws.redshift#Cluster) models AvailabilityZoneRelocationStatus but does NOT declare AvailabilityZoneRelocation, IsMaintenanceWindowActivated or AzDisasterRecoveryStatus, so the requirement comes from the recording: the live awsQuery/XML DescribeClusters response carries all three elements inside each DescribeClustersResult.Clusters.Cluster, with HTTP 200. Their values reflect the cluster's own configured state — the relocation setting the cluster was created/modified with ('enabled' when availability-zone relocation is on), whether the cluster is currently inside its preferred maintenance window ('true'/'false'), and the cluster's AZ disaster-recovery state ('enabled' when it is on). The specific strings in the recording are that cluster's state filling those slots, not literals to hard-code.
Emulator: the three elements are absent from the response document entirely (replay reports each as missing, expected 'enabled' / 'true' / 'enabled', got None) while the status matches at 200.
Why: a client that reads relocation capability, maintenance-window activity or AZ disaster-recovery state off DescribeClusters gets nothing back and cannot tell "the feature is off" from "the field was not reported"; XML consumers that bind the response to AWS's document shape see three fewer elements than AWS sends, and a client that just enabled AZ relocation (as this fixture does) cannot observe it on the cluster it configured.
Fix: the DescribeClusters Cluster serializer, plus the cluster state that must carry the relocation setting, the maintenance-window-active computation and the AZ-DR status so the serializer has values to emit.
Traces: redshift-aqua-configuration-modify-transition#task#3; the same cluster also covers redshift-aqua-configuration-modify-transition#task#5 and redshift-create-cluster-lifecycle#task#2.

Related open issue, which does NOT own this replay key: 20261029T192501Z-redshift-read-payloads-omit-modeled-members-and-tagging.md describes the same three omitted members as its Cause 1, but its recorded keys for this cluster do not include $.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.Endpoint.Address, so replay no longer matches this cluster to it and reports it unowned. Fixing either should be verified by re-replaying the step named above.

Not part of this finding: the same step also reports Cluster.Endpoint.Address differing from the recording ('sim2real-conv-aqua-85f4122a-...cklgxdrirwon.us-west-2.redshift.amazonaws.com' vs 'sim2real-conv-aqua-99dcf901-...30c6517df75b.us-west-2.redshift.amazonaws.com'). Both are well-formed <cluster-name>.<generated-subdomain>.<region>.redshift.amazonaws.com endpoint hostnames; they differ only in the per-run generated cluster identifier and the generated subdomain, which are environment values and not a requirement.

## Evidence
Replay of simparity-wire-aws-v1, fixture redshift-aqua-configuration-modify-transition step task#3, operation DescribeClusters, verdict=mismatch, expected_status 200 / actual_status 200 (the status agrees; only the body diverges):

  $.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.AvailabilityZoneRelocation: missing (expected 'enabled', got None)
  $.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.IsMaintenanceWindowActivated: missing (expected 'true', got None)
  $.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.AzDisasterRecoveryStatus: missing (expected 'enabled', got None)

Cluster members in the same cluster of affected steps: redshift-aqua-configuration-modify-transition#task#5, redshift-create-cluster-lifecycle#task#2 (affected_count 3).

The requirement is the PRESENCE of the three elements and that each reports the cluster's own configured state; 'enabled'/'true'/'enabled' are this cluster's state values filling those slots. Smithy check used: query=shape, shape=Cluster on services/service-com-amazonaws-redshift-redshiftserviceversion20121201/smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json — the model declares AvailabilityZoneRelocationStatus only, so these three members are recorded-behavior requirements rather than modeled ones.

Also observed in the step and deliberately excluded: Cluster.Endpoint.Address (host_not_own) differs only in the per-run generated cluster id and generated endpoint subdomain.

## Steps to Reproduce
1. Replay fixture redshift-aqua-configuration-modify-transition from trace set simparity-wire-aws-v1 up to step task#3: the fixture creates a cluster, modifies its AQUA/availability-zone-relocation configuration, then calls DescribeClusters.
2. Inspect the DescribeClusters 200 response body at $.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster and look for the AvailabilityZoneRelocation, IsMaintenanceWindowActivated and AzDisasterRecoveryStatus elements. The emulator emits none of them; the recording has all three.
3. Repeat on redshift-aqua-configuration-modify-transition#task#5 and redshift-create-cluster-lifecycle#task#2, which are the other members of the same divergence cluster.
4. Ignore Cluster.Endpoint.Address when comparing: it carries per-run generated host components.
