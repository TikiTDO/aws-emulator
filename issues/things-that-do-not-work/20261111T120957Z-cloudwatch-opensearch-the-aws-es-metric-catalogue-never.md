Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__opensearch.obs-opensearch-1-domain-metrics-track-cluster-health-and-documents
Issue-Key: cloudwatch__opensearch.obs-opensearch-1-domain-metrics-track-cluster-health-and-documents|run#28|TimeoutError|raise _not_graded(cat_start, POLL_DEADLINE_S, "the AWS/ES metric catalogue")
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# cloudwatch__opensearch: the AWS/ES metric catalogue never appears for a domain, so the cluster-health/document metric check exhausts its budget ungraded

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMetrics

## Description
Cause 1 of 1: an active OpenSearch domain publishes no AWS/ES metric series, so a caller waiting for the namespace's metric catalogue to appear never gets it and the run is terminated ungraded.

AWS: recorded behavior only (metric emission is not expressible in the Smithy model). A domain created with OpenSearch_2.11 on a single t3.small.search node with EBS gp3 publishes AWS/ES metrics that CloudWatch serves through ListMetrics/GetMetricStatistics within minutes of the domain becoming active — at minimum ClusterStatus.green, ClusterStatus.yellow, ClusterStatus.red, Nodes and SearchableDocuments — dimensioned by ClientId and DomainName, and SearchableDocuments tracks the documents indexed into the domain. The catalogue appears well inside the recording's 600 s wait.

Emulator: ListMetrics for Namespace='AWS/ES' never lists those metrics; the check's time budget runs out after 583.4 s of a 600 s wait and reports 'the check's time budget ran out ... before the 600s wait for the AWS/ES metric catalogue ended; not graded'. No call is refused — the series simply never exists, so the wait cannot end.

Why: nothing that reads an OpenSearch domain through CloudWatch works — cluster-health dashboards, ClusterStatus.red/yellow alarms, node-count and SearchableDocuments monitoring — and a client cannot distinguish 'domain idle' from 'metrics not emitted'. A poller waiting for the catalogue hangs rather than receiving an answer or a modeled error.

Fix: the OpenSearch (es) domain's metric emission into the CloudWatch metric store — the emitter must publish AWS/ES datapoints under the {ClientId, DomainName} dimension set for an active domain; CloudWatch's own read path serves other namespaces correctly.

Traces: cloudwatch__opensearch.obs-opensearch-1-domain-metrics-track-cluster-health-and-documents.

Related open issue that does NOT carry this check's replay key and therefore does not cover it; a fixer should merge with it: 20261111T000041Z-cloudwatch-opensearch-no-aws-es-metric-series-is.md.

## Evidence
Check cloudwatch__opensearch.obs-opensearch-1-domain-metrics-track-cluster-health-and-documents [error]:

TimeoutError: the check's time budget ran out after 583.4s, before the 600s wait for the AWS/ES metric catalogue ended; not graded

Expected: ListMetrics/GetMetricStatistics for Namespace='AWS/ES' list ClusterStatus.green, ClusterStatus.yellow, ClusterStatus.red, Nodes and SearchableDocuments for the domain under the {ClientId, DomainName} dimensions, with SearchableDocuments reflecting the 25 indexed documents.
Actual: the AWS/ES catalogue never appears; the wait cannot complete and no call returns a status, code or message to compare.

## Steps to Reproduce
1. opensearch CreateDomain with EngineVersion OpenSearch_2.11, ClusterConfig {InstanceType: t3.small.search, InstanceCount: 1}, EBSOptions {EBSEnabled: true, VolumeType: gp3, VolumeSize: 10}; wait until it is active.
2. Index ~25 documents into the domain.
3. cloudwatch ListMetrics(Namespace='AWS/ES') and GetMetricStatistics for ClusterStatus.green / Nodes / SearchableDocuments with Dimensions [{ClientId}, {DomainName}].
4. Expect the metric catalogue and datapoints; observe neither, and the wait runs to the budget.
