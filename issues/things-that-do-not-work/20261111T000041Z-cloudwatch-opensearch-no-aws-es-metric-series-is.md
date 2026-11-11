Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__opensearch.obs-opensearch-domain-metrics-listed-under-client-id-and-domain-name-only
Issue-Key: cloudwatch__opensearch.obs-opensearch-domain-metrics-listed-under-client-id-and-domain-name-only|run#25|TimeoutError|raise _not_graded(cat_start, POLL_DEADLINE_S, "the AWS/ES metric catalogue")
Issue-Commit: cf4b8496ba0c958ea150f5d2d9329060f4b043af

# cloudwatch__opensearch: no AWS/ES metric series is published for a domain, so the ClientId+DomainName metric catalogue never appears

> _Note: this issue was identified against the code base at commit `cf4b8496ba0c958ea150f5d2d9329060f4b043af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMetrics

## Description
Cause 1 of 1: an OpenSearch domain the emulator reports Active publishes no AWS/ES CloudWatch metrics, so the catalogue the check waits for never materialises and the check is cut off before it can grade anything.
AWS: recorded behavior only for the emulator side; on AWS the namespace and dimensioning are documented (Amazon OpenSearch Service developer guide, managedomains-cloudwatchmetrics): an active domain publishes cluster metrics into namespace AWS/ES — ClusterStatus.green, ClusterStatus.yellow, ClusterStatus.red, Nodes and SearchableDocuments among them — dimensioned by ClientId (the account id) and DomainName, and ListMetrics(Namespace='AWS/ES') reports exactly those series with that dimension set. The calls are 200; nothing is refused.
Emulator: ListMetrics never reports an AWS/ES series for the domain. The check waits for the catalogue and is cut off: 'TimeoutError: the check's time budget ran out after 50.1s, before the 600s wait for the AWS/ES metric catalogue ended; not graded'. Hypothesis: the opensearch domain lifecycle has no CloudWatch publishing path at all, so the wait can never end rather than ending late.
Why: a client that alarms on ClusterStatus.red or scales on SearchableDocuments gets an empty metric list and empty Datapoints forever; monitoring an emulated domain is impossible, and the dimension contract (series listed under ClientId+DomainName only) cannot be checked.
Fix: the opensearch domain state/lifecycle in the opensearch service crate must publish the AWS/ES series (cluster status gauges, Nodes, SearchableDocuments) into CloudWatch with ClientId + DomainName dimensions as the domain settles and as documents are indexed. Numerous sibling issues already record the same class for other namespaces (e.g. 20261109T183935Z-cloudwatch-ec2-2-net-new-causes-no-aws.md, 20261109T183935Z-cloudwatch-sqs-no-aws-sqs-metric-series-is.md); none of them covers AWS/ES.
Traces: cloudwatch__opensearch.obs-opensearch-domain-metrics-listed-under-client-id-and-domain-name-only.

## Evidence
Replay output: File check_cloudwatch__opensearch_obs_opensearch_domain_metrics_listed_under_client_id_and_domain_name_only.py line 239 -> TimeoutError: the check's time budget ran out after 50.1s, before the 600s wait for the AWS/ES metric catalogue ended; not graded. Expected: ListMetrics(Namespace='AWS/ES') reports ClusterStatus.green/.yellow/.red, Nodes and SearchableDocuments for the domain, each dimensioned [ClientId=<account>, DomainName=<domain>]. Actual: no AWS/ES series is ever listed.

## Steps to Reproduce
1. CreateDomain (OpenSearch_2.11, t3.small.search x1, EBS gp3 10 GiB) and wait for Processing=false with a published Endpoint.
2. Call cloudwatch ListMetrics(Namespace='AWS/ES') and poll it: AWS lists ClusterStatus.green/.yellow/.red, Nodes and SearchableDocuments under [ClientId, DomainName]; the emulator lists nothing, indefinitely.
3. Call GetMetricStatistics for AWS/ES Nodes with those dimensions — Datapoints stays empty.
