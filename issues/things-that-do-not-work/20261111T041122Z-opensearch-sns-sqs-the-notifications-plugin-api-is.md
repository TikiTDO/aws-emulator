Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false
Issue-Key: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false|run#13|AssertionError|assert status in (200, 201), (
Issue-Commit: 181b4db8a49f9f99cf56c1b2f4aacf19bf82375e

# opensearch__sns__sqs: the notifications plugin API is unrouted — POST /_plugins/_notifications/configs answers 400 'no handler found', so no SNS notification channel exists and an alerting monitor can never publish (1 cause)

> _Note: this issue was identified against the code base at commit `181b4db8a49f9f99cf56c1b2f4aacf19bf82375e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OpenSearch domain REST notifications plugin API (POST /_plugins/_notifications/configs, es:ESHttpPost)

## Description
One cause.

Cause 1 of 1: the OpenSearch notifications plugin's configuration endpoint is not routed on a created domain's published endpoint, so the SNS notification channel an alerting monitor needs cannot be created, and the whole monitor-fires-to-SNS path (and the contrast case where the trigger condition is false and nothing is published) is unreachable.

AWS: recorded behavior only (the notifications plugin REST API is not part of the opensearch Smithy model; it reaches the domain through es:ESHttpPost). The recordings show POST /_plugins/_notifications/configs with an SNS channel body (config_id, name, config_type "sns", and the sns topic ARN plus role ARN) answering HTTP 200 or 201 with the created config, after which an alerting monitor whose trigger references that channel publishes its rendered message to the SNS topic when the trigger condition evaluates true, and publishes nothing when it evaluates false.
Emulator: HTTP 400 `{"error": "no handler found for uri [/_plugins/_notifications/configs] and method [POST]"}`. The fixed text is `no handler found for uri [` ... `] and method [` ... `]` with the request's own path and method filled in.
Why: a client cannot wire OpenSearch alerting to SNS at all, and the 400 reads as a malformed channel definition rather than as an unavailable plugin, so the caller blames its own body. Downstream, nothing is ever delivered to the SNS topic or its SQS subscriber, so a monitor that should alert looks identical to one whose condition was false — a silent loss of notifications.
Fix: the OpenSearch domain data plane's REST router plus a notifications-plugin handler — route /_plugins/_notifications/configs (create/read/update/delete), store the SNS channel configuration, and have the alerting monitor's trigger publish its rendered message to the configured SNS topic only when the trigger condition is true.
Traces: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false.

Note: open issues 20261024T055421Z-opensearch-sns-sqs-opensearch-alerting-monitor-cannot-be-exercised-the-domain.md (endpoint answered 404 UnknownService 'es') and 20261101T140814Z-opensearch-sns-sqs-the-alerting-monitor-trigger-check.md (POST /_bulk answered 405) describe earlier, different failures of this same scenario; those steps now succeed and the scenario dies later, on the unrouted notifications-plugin route. The replay marks this check as neither already-filed nor owned.

## Evidence
Expected (recorded AWS): POST /_plugins/_notifications/configs with an SNS channel body answers HTTP 200 or 201 with the created notification config.

Actual (emulator), verbatim from the replay:
  AssertionError: creating the SNS notification channel returned HTTP 400, expected 200/201: {'error': 'no handler found for uri [/_plugins/_notifications/configs] and method [POST]'}

The config id, topic ARN and domain name in the recording are per-run values, not part of the requirement.

## Steps to Reproduce
1. Create an SNS topic with an SQS queue subscribed to it, and an IAM role the domain can assume to publish.
2. CreateDomain (EngineVersion OpenSearch_2.11, single t3.small.search node, gp3 10 GiB); wait for Processing=false and read DomainStatus.Endpoint.
3. Index documents through the data plane and _refresh so a monitor query has something to count.
4. SigV4-sign and send POST https://<DomainStatus.Endpoint>/_plugins/_notifications/configs with an SNS channel body (config_type "sns", the topic ARN and role ARN).
5. Observe HTTP 400 {"error":"no handler found for uri [/_plugins/_notifications/configs] and method [POST]"} instead of 200/201, so the alerting monitor can never be given a channel and neither the publish nor the withheld-publish case can be observed on the SQS queue.
