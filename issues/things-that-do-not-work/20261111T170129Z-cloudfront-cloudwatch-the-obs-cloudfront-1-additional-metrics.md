Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes
Issue-Key: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes|run#16|Timeout|time.sleep(max(1.0, min(_METRIC_INTERVAL_S, float(until_next_round))))
Issue-Commit: 3163fad6f190575ee3da7920eeb1a958f0b14066

# cloudfront__cloudwatch: the obs-cloudfront-1 additional-metrics check never reaches a verdict — no AWS/CloudFront additional metric series is ever published for served edge traffic

> _Note: this issue was identified against the code base at commit `3163fad6f190575ee3da7920eeb1a958f0b14066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMonitoringSubscription

## Description
Cause 1 of 1: a distribution with an enabled monitoring subscription publishes no AWS/CloudFront additional metric series, so a check that drives edge traffic and then reconciles the metrics against the outcomes it actually recorded never reaches a verdict.

AWS: recorded behavior only for the per-datapoint values (the Smithy model describes only the control-plane shapes). CloudFront's CreateMonitoringSubscription with RealtimeMetricsSubscriptionConfig.RealtimeMetricsSubscriptionStatus=Enabled turns on the additional metrics (CacheHitRate, OriginLatency, 4xxErrorRate/403ErrorRate/503ErrorRate, …) for the distribution; after viewer requests are served at the distribution's edge domain those series exist in the AWS/CloudFront namespace under the DistributionId + Region=Global dimension pair and GetMetricStatistics/GetMetricData return datapoints for them within the check's polling budget. The recording this check was calibrated against served every request it issued (3 repeats on a present key, 2 on an absent key) and saw the derived metric values follow those outcomes.

Emulator: the check exhausts the whole 600 s budget and is reported as `Error: Check timed out after 600.0s` — it never gets past waiting for the awaited effect. Hypothesis (grounded in the already-open cloudfront/cloudfront__cloudwatch issues named below, which report the same two shapes): either the distribution's published DomainName is a host the emulator does not answer on, so no edge request is served at all, or the edge serves but the AWS/CloudFront additional-metric series is never published; in both cases no datapoint ever appears. The check marks the first shape with DATAPLANE_UNIMPLEMENTED so that a never-serving edge reads as a fidelity failure rather than an environment error, so neither shape is the runner's fault.

Why: a client that enables a monitoring subscription and then dashboards or alarms on CacheHitRate / OriginLatency / *ErrorRate sees permanently empty series. Alarms over them sit in INSUFFICIENT_DATA forever instead of tracking the edge, and any autoscaling or paging built on them never fires.

Fix: the CloudFront edge data plane and the AWS/CloudFront metric-publication path that the monitoring subscription enables (the component that records per-request edge outcomes and publishes them into CloudWatch), not the check.

Related open issues that do NOT cover this check (the suite did not mark it owned by any of them, so it is filed separately and they are named here only as context): 20261110T162302Z-cloudfront-cloudwatch-cloudfront-additional-monitoring-subscription-metrics-check-never-completes.md, 20261110T171004Z-cloudfront-cloudwatch-the-additional-metrics-gating-check-never.md, 20261110T090020Z-cloudfront-cloudwatch-a-served-edge-produces-no-aws.md.

Traces: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes.

## Evidence
Expected: after CreateMonitoringSubscription(Enabled) and the 6 viewer requests the check issues, AWS/CloudFront additional-metric datapoints (CacheHitRate, OriginLatency, error-rate series) exist under the distribution's dimensions and agree, within the check's documented slack (±1 request on Requests), with the outcomes the check recorded from the edge responses.

Actual: `[mismatch] cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes` → `Error: Check timed out after 600.0s`. The check polls every 30 s for up to its metric deadline and never observes a datapoint, so it never evaluates any of its comparisons.

## Steps to Reproduce
1. CreateDistribution with the managed caching policy and wait for it to be Deployed.
2. CreateMonitoringSubscription on it with RealtimeMetricsSubscriptionConfig.RealtimeMetricsSubscriptionStatus=Enabled.
3. Issue viewer requests to the distribution's DomainName: one plus 3 repeats for a key the origin has, and 2 for a key it does not.
4. Poll cloudwatch GetMetricStatistics/GetMetricData in namespace AWS/CloudFront for CacheHitRate and OriginLatency under DistributionId=<id>, Region=Global.
Observed: no datapoints for the whole budget (and, per the related issues, the edge requests in step 3 may not be served at all). Expected: datapoints that reproduce the hit/miss and error outcomes recorded in step 3.
