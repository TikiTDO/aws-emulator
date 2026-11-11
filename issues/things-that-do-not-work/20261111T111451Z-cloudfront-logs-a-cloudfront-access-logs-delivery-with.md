Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery
Issue-Key: cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# cloudfront__logs: a CloudFront ACCESS_LOGS delivery with two destinations carries nothing — the edge never answers the request to be logged, so the per-request record join and DeleteDelivery's severing contract are unreachable

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
One net-new root cause, grounded in the failing check below. Cause count: 1 — it must be closed before this issue is submitted.

RELATED OPEN ISSUES (neither covers this check in this corpus): 20261110T105638Z-cloudfront-logs-cloudfront-standard-logging-v2-delivers-no-fa23e8.md owns the sibling check cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group — one source, one destination, record presence only — and does not carry this check's replay key or grade the two-destination differential barrier or DeleteDelivery's cut-off. 20261025T174921Z-logs-cloudfront-cloudfront-vended-access-logs-are-never.md describes the same family but was filed against a DIFFERENT corpus (Issue-Corpus: simparity-behavioral-ga-prod-v1) under the service token logs__cloudfront, so it carries no replay key for this suite; this check is marked neither ALREADY FILED nor OWNED in the replay report. 20261109T143515Z-cloudfront-logs-a-cloudfront-distribution-s-domainname-is-published-as.md attributes an unserved edge to the published <id>.cloudfront.net DomainName; the check here resolves the edge through the harness's own endpoint route, so that attribution is not what is observed.

=== Cause 1 of 1: the vended ACCESS_LOGS pipeline is green at the control plane and delivers no record, because the distribution's edge never answers the request there would be a record for ===
AWS: recorded behavior only for the delivery leg — the CloudWatch Logs model describes the control-plane envelopes (PutDeliverySource with logType ACCESS_LOGS over a CloudFront distribution ARN, PutDeliveryDestination, CreateDelivery with recordFields, GetDelivery, DeleteDelivery) and there is no model surface for the CloudFront edge. Recorded AWS behavior: (i) once the distribution is Deployed, a GET of an object the attached OriginAccessControl can read is answered 200 and a GET of an absent key is answered 403; every response carries a per-request X-Amz-Cf-Id and an X-Cache whose value ends in " from cloudfront" and whose first word is Miss for a CachingDisabled 200 and Error for the OAC-denied 403. (ii) Each served request becomes ONE JSON record in EVERY live delivery's destination log group, with exactly that delivery's configured recordFields as keys and no others, x-edge-request-id equal to the X-Amz-Cf-Id the viewer got, cs-uri-stem equal to the requested path, sc-status equal to the status answered, x-edge-result-type equal to the first word of that X-Cache. (iii) Two CreateDelivery calls on the SAME delivery source both succeed and both carry every request. (iv) DeleteDelivery(id) severs exactly that one delivery: GetDelivery on the id is then ResourceNotFoundException with message "Requested Delivery does not exist in this account." (fixed text for the class), a repeat DeleteDelivery on the same id is the same ResourceNotFoundException rather than an idempotent 200, the sibling delivery on the same source is untouched and keeps carrying later requests, already-delivered records are not purged, and delivery into that group stops and stays stopped.
Emulator: every control-plane call succeeds — s3 CreateBucket/PutObject/PutBucketPolicy, cloudfront CreateOriginAccessControl/CreateDistribution/TagResource, logs CreateLogGroup x2, PutDeliverySource (its asserted envelope service == "cloudfront" and resourceArns == [the distribution ARN] both hold), PutDeliveryDestination x2 (both deliveryDestinationType == "CWL") and CreateDelivery x2. The check is then killed by the suite's 60 s cap while still inside the first edge GET, whose own budget is 240 s (the log-record polls are 900 s each and the Deployed poll 1500 s, so the kill lands in the edge fetch): no response with the expected status and an X-Amz-Cf-Id header ever arrives from the distribution's own edge address. So two independent pipelines read as live and nothing is ever delivered. The edge host is the emulator's own announced distribution endpoint, so this is the emulator refusing to serve, not a runner egress or name-resolution fault.
Why: the whole causal claim of CloudFront standard logging v2 is unverifiable and silently empty. A client gets HTTP 200 from all five control-plane calls, believes two log pipelines are live, and never receives a record or an error. Request-level correlation (X-Amz-Cf-Id joined to x-edge-request-id) — the only way to tie a viewer response to its log record — does not exist. Because nothing is ever delivered, DeleteDelivery's semantics are equally unobservable: a client cannot tell whether deleting one delivery severed only that one, whether a repeat delete errors or succeeds idempotently, or whether already-delivered records survive.
Fix: two components, both needed. (a) The CloudFront edge data plane for a distribution's served address: resolve the cache behavior and origin for the path, fetch through the attached OriginAccessControl, answer 200 for a readable object and 403 for one the OAC cannot read, and emit a per-response X-Amz-Cf-Id plus an X-Cache of the form "<Miss|Hit|Error> from cloudfront". (b) The delivery leg in the CloudWatch Logs service: for each served request, resolve the distribution's live ACCESS_LOGS deliveries and append one JSON event per delivery to its destination log group's stream, restricted to exactly that delivery's recordFields and populated from that request/response (x-edge-request-id == the X-Amz-Cf-Id returned to the viewer); DeleteDelivery must stop only that delivery's emission, leave sibling deliveries on the same source emitting, leave already-written events in place, and make GetDelivery and a repeat DeleteDelivery of the deleted id answer ResourceNotFoundException "Requested Delivery does not exist in this account."
Traces: cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery.

## Evidence
Replay output for cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery:

  Error: Check timed out after 60.0s

Where the cap lands, from the check body: all setup is single non-polling calls whose envelopes are asserted immediately and are NOT reported as the failure —
  assert src["service"] == "cloudfront" and src["resourceArns"] == [dist_arn]
  assert dest["deliveryDestinationType"] == "CWL"
so PutDeliverySource, both PutDeliveryDestination calls and both CreateDelivery calls succeeded. The next steps are polls with far larger budgets than the 60 s cap:
  _wait_deployed(cf, dist_id)                       # 1500 s
  _request(base + path, route_headers, status)       # 240 s, predicate: r[0] == expect_status and "x-amz-cf-id" in r[1]
  _poll_until(lambda: _records(logs, group), ...)    # 900 s per group
The kill therefore lands in the edge fetch against the distribution's own endpoint: no response with the expected status and an X-Amz-Cf-Id ever arrives.

Expected (never reached): GET /<present key> -> 200, again -> 200, GET /<absent key> -> 403; three distinct X-Amz-Cf-Id values; X-Cache ending in " from cloudfront" with first words Miss, Miss, Error; then in BOTH destination log groups one JSON record per request whose keys are exactly ['date','time','cs-uri-stem','sc-status','x-edge-result-type','x-edge-request-id'], with x-edge-request-id == that response's X-Amz-Cf-Id, cs-uri-stem == the requested path, sc-status == the answered status and x-edge-result-type == the first word of X-Cache.
Actual: the edge answers nothing matching, so no record is ever written to either group and the check is killed at the harness cap.

Assertions that consequently never ran:
  assert len(phase1) == 3                                  # three requests mint three distinct X-Amz-Cf-Id values
  assert {v[2] for v in phase1.values()} == {"Miss", "Error"}
  assert set(rec) == set(RECORD_FIELDS)
  GetDelivery(deleted id)        == ("ResourceNotFoundException", "Requested Delivery does not exist in this account.")
  DeleteDelivery(same id) again  == the same ResourceNotFoundException, not an idempotent 200
  the sibling delivery on the same source still returns its deliveryDestinationArn and keeps carrying post-delete batches
  once the delete takes effect the primary group stays dark, and records already delivered to it are not purged

Bucket/key/log-group names, the distribution id and domain, and the minted X-Amz-Cf-Id values are per-run; the requirement is the join between the response the edge gave and the record each live delivery wrote, and the one-delivery scope of DeleteDelivery.

## Steps to Reproduce
Delivery plane pinned to us-east-1 (CloudFront's vended-log control-plane region).
1. s3 CreateBucket(<new bucket>); PutObject(Key=<present>.txt). Choose a second key <absent>.txt that is never uploaded.
2. cloudfront CreateOriginAccessControl(OriginAccessControlOriginType="s3", SigningBehavior="always", SigningProtocol="sigv4").
3. cloudfront CreateDistribution: Enabled=True, one origin DomainName="<bucket>.s3.<region>.amazonaws.com" with S3OriginConfig.OriginAccessIdentity="" and OriginAccessControlId=<step 2>; DefaultCacheBehavior with ViewerProtocolPolicy="allow-all" and the managed CachingDisabled CachePolicyId. Note Id, ARN, DomainName.
4. s3 PutBucketPolicy allowing cloudfront.amazonaws.com s3:GetObject with Condition AWS:SourceArn == the distribution ARN.
5. logs CreateLogGroup twice in us-east-1: a "primary" and a "barrier" group.
6. logs PutDeliverySource(name=<new>, resourceArn=<distribution ARN>, logType="ACCESS_LOGS") -> succeeds (service == "cloudfront").
7. logs PutDeliveryDestination twice (outputFormat="json", destinationResourceArn="arn:aws:logs:us-east-1:<account>:log-group:<group>") -> both "CWL".
8. logs CreateDelivery twice on the SAME deliverySourceName, one per destination, recordFields=["date","time","cs-uri-stem","sc-status","x-edge-result-type","x-edge-request-id"] -> both succeed.
9. cloudfront GetDistribution until Status == "Deployed".
10. GET <edge>/<present>.txt twice and <edge>/<absent>.txt once.
    Observed: no response with the expected status and an X-Amz-Cf-Id header ever arrives; the poll never terminates and the check is killed at the 60 s harness cap.
    Expected: 200, 200, 403, each with a distinct X-Amz-Cf-Id and an X-Cache ending in " from cloudfront" whose first words are Miss, Miss, Error.
11. logs DescribeLogStreams + GetLogEvents over BOTH groups -> expected one JSON record per request in each group, keys exactly the configured recordFields, joined on x-edge-request-id == the X-Amz-Cf-Id from step 10. Unreachable today.
12. logs DeleteDelivery(id=<primary delivery>), then GetDelivery(same id) and DeleteDelivery(same id) again -> expected ResourceNotFoundException "Requested Delivery does not exist in this account." both times; the sibling delivery still resolves; later requests keep landing in the barrier group and stop landing in the primary group while the step-11 records remain. Unreachable today.
13. Or replay cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery.
