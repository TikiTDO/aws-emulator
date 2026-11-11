Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription
Issue-Key: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# cloudfront__cloudwatch: no edge traffic is ever served, so AWS/CloudFront additional metrics can neither be produced nor shown to be withheld

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
ONE root cause, covering both failing checks of this service.

Cause 1 of 1: a distribution's edge domain serves nothing, so the CloudWatch side of the additional-metrics contract is unreachable.
AWS: recorded behavior only. After CreateMonitoringSubscription enables additional metrics on a distribution and real viewer requests are served at its edge domain, AWS/CloudFront publishes the additional metrics (e.g. CacheHitRate, OriginLatency, error-rate metrics) under {DistributionId, Region=Global} for the subscribed distribution, and publishes NOTHING for an unsubscribed control distribution that received the same traffic.
Emulator: both checks warm the edge first — `_fetch_ok(session, f"{ctx.endpoint_url(dist['DomainName'])}/{warmup_key}")` — and that GET never answers 200. Each check polls its own 300 s budget and is killed by the suite's 60 s cap, so GetMetricStatistics is never reached. The control plane is fine (CreateMonitoringSubscription succeeds, distribution Status reaches Deployed with PROPAGATION_DELAY = 500 ms in src/state.rs); the edge is what answers nothing.
Why: a caller cannot observe CloudFront metrics at all, and cannot verify the negative half of the contract (metrics withheld without a subscription) either — an observability trajectory written against the emulator sees an empty namespace whether or not it subscribed.
Fix: the CloudFront data-plane listener for distribution domain names (missing; see the same cause filed for service `cloudfront`), plus the AWS/CloudFront metric emission that edge requests must drive into the cloudwatch service's metric store.
Traces: cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes, cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription.

## Evidence
Replay output, verbatim tails:
- cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-match-recorded-edge-outcomes: Error: Check timed out after 60.0s. The check is tagged "concern:data-plane" and its first edge call is `warm = _fetch_ok(sessions..., f"{base}/{warmup_key}")` with `base = ctx.endpoint_url(dist["DomainName"])`; `_fetch_ok` GETs until 200 with timeout_s=300.
- cloudfront__cloudwatch.obs-cloudfront-1-additional-metrics-withheld-without-monitoring-subscription: Error: Check timed out after 60.0s, same `_fetch_ok` warm-up across both the subscribed subject and the unsubscribed control.
Expected (AWS): the warm-up GET answers 200 with the warm-up body, then cw.get_metric_statistics(Namespace='AWS/CloudFront', Dimensions=[{DistributionId},{Region: Global}]) returns datapoints for the subscribed distribution and [] for the control. Actual (emulator): the edge GET never answers, so neither assertion is reached.

## Steps to Reproduce
1. Create a distribution over an S3 origin with a CachingDisabled-style cache policy; wait for Status Deployed.
2. CreateMonitoringSubscription for it (succeeds).
3. GET https://<DomainName>/<warmup_key> — never answers 200.
4. Consequently GetMetricStatistics in AWS/CloudFront can never be driven by edge traffic.
Or replay the two check ids in trace_ids.
