Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare
Issue-Key: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Commit: adb8f986b66b684ed38690a8bd355d4f46d0ebf5

# SES configuration-set CloudWatch event destinations deliver nothing: no AWS/SES Send datapoint with resolved dimensions ever appears, and both cloudwatch__ses checks consume their whole budget polling

> _Note: this issue was identified against the code base at commit `adb8f986b66b684ed38690a8bd355d4f46d0ebf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSetEventDestination

## Description
One net-new root cause, grounded in both failing cloudwatch__ses checks. Neither is marked "ALREADY FILED" nor "OWNED by" in the replay report.

Cause 1 of 1: a configuration set's CloudWatchDestination never delivers a datapoint to the AWS/SES namespace, so no Send metric with resolved dimensions is ever readable and neither check can reach a verdict.
Failing check ids: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions, cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare.

AWS: recorded behavior only for the cross-service effect (the Smithy models cover the envelopes, not the delivery). sesv2:CreateConfigurationSetEventDestination accepts an EventDestination with Enabled=true, MatchingEventTypes=["SEND"] and a CloudWatchDestination carrying a DimensionConfigurations list, each entry a DimensionName with a DimensionValueSource of MESSAGE_TAG or EMAIL_HEADER plus a DefaultDimensionValue. The recorded behavior these two checks were calibrated against is: after sesv2:SendEmail through that configuration set (EmailTags naming the message tag, Content.Simple.Headers naming the MIME header), SES's asynchronous publishing pipeline delivers exactly ONE datapoint to CloudWatch namespace AWS/SES, MetricName 'Send', dimensioned by every dimension the destination DECLARED and resolved from that send — ses:configuration-set = the configuration set name, the custom message tag = its EmailTags value, the custom header dimension = its header value — readable via cloudwatch:GetMetricStatistics(Namespace='AWS/SES', MetricName='Send', Dimensions=<that exact combination>, Period=60, Statistics=['Sum']) with Sum == 1. The paired negative is equally recorded: a dimension the destination did NOT declare is never resolved onto the datapoint at all — not even filled with its DefaultDimensionValue — so SES attaching ses:configuration-set as an implicit message tag to every send is NOT by itself enough to make it a published dimension.

Emulator: neither check reaches any of its assertions. Both are reported [mismatch] with the detail 'Check timed out after 60.0s' — the harness kills them while they are still blocked, with their own metric-poll budget (_METRIC_POLL_TIMEOUT_S = 300.0, interval 15.0) and absence hold (_ABSENCE_HOLD_S = 60.0) far from exhausted. The condition they block on never becomes true on this emulator: _metric_sum() calls cloudwatch:GetMetricStatistics for AWS/SES 'Send' over that dimension combination and keeps getting an empty Datapoints list (the helper returns None for an empty list, and _poll_until waits for `v is not None`). So the SES->CloudWatch metric-delivery path is inert: a CloudWatchDestination is accepted on create and then publishes nothing. Hypothesis (neither the model nor the recording states where the stall lands first): the same run may also be blocked earlier in _sending_domain/_verified_stand_in, which creates a Route 53 public hosted zone for a stand-in domain, publishes the Easy DKIM CNAMEs sesv2:CreateEmailIdentity returned, and then polls sesv2:GetEmailIdentity until VerifiedForSendingStatus is true and DkimAttributes.Status == 'SUCCESS' (budget 600.0s) — if the emulator's SES never promotes an identity whose DKIM CNAMEs exist in its own Route 53 to verified, the checks never even reach SendEmail. Both stall points are the emulator's: a target that serves its own Route 53 zone is expected to answer the same requests as AWS.

Why: a client that wires SES observability the documented way — one configuration set, one CloudWatch event destination, dimensions declared for its message tags and MIME headers — reads an empty AWS/SES namespace on this emulator and concludes its sends failed or its destination is misconfigured, when SES accepted both. Dashboards, per-campaign attribution and any alarm over AWS/SES Send are silently dead; worse, the negative contract is unobservable too, so an emulator that later publishes a Send datapoint carrying UNDECLARED dimensions (or declared ones filled with DefaultDimensionValue) would look equally correct to a client while over-reporting. Because both checks time out rather than assert, the resolved-dimension and never-resolved-dimension halves of the contract are entirely ungraded.

Fix: the SES event-publishing path — the CloudWatchDestination branch of the configuration-set event-destination handler and its state in the sesv2 crate (services/service-com-amazonaws-ses-simpleemailservice / the sesv2 crate), which must, on an accepted SendEmail through a configuration set with an Enabled destination matching event type SEND, publish one AWS/SES 'Send' datapoint into the cloudwatch crate's metric store dimensioned by exactly the destination's declared DimensionConfigurations resolved from that send's EmailTags and MIME headers (and by nothing else). Secondarily, the sesv2 email-identity verification state machine, so GetEmailIdentity reports VerifiedForSendingStatus/DKIM SUCCESS once the returned DKIM CNAMEs exist in the target's own Route 53.

Traces: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions (blocks at its _poll_until over _metric_sum, check body lines 63588-63601 of the replay report), cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare (same helper, asserting the absence half).

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the built emulator: 125/412 passed (30.3%). Per-service line: `cloudwatch__ses 0/2 (0%) 2 failed`. Both check ids are in the failing list, both [mismatch], and NEITHER is marked "ALREADY FILED" or "OWNED by".

Failing detail for both: `Error: Check timed out after 60.0s` — no assertion was reached, so the emulator produced no datapoint for the condition either check polls on.

Expected (cloudwatch-destination-registers-send-metric-with-resolved-dimensions):
  1. sesv2:CreateConfigurationSet ConfigurationSetName=<cs>.
  2. sesv2:CreateConfigurationSetEventDestination EventDestinationName='sim2real-cw', EventDestination={Enabled: true, MatchingEventTypes: ['SEND'], CloudWatchDestination.DimensionConfigurations: [ {ses:configuration-set, MESSAGE_TAG, default 'none'}, {campaign, MESSAGE_TAG, default 'none'}, {X-Custom-Header, EMAIL_HEADER, default 'none'} ]}.
  3. sesv2:SendEmail FromEmailAddress=<sender@verified-domain>, ConfigurationSetName=<cs>, EmailTags=[{campaign: <campaign-value>}], Content.Simple.Headers=[{X-Custom-Header: <header-value>}] -> a non-empty MessageId.
  4. cloudwatch:GetMetricStatistics Namespace='AWS/SES', MetricName='Send', Dimensions=[{ses:configuration-set=<cs>}, {campaign=<campaign-value>}, {X-Custom-Header=<header-value>}], Period=60, Statistics=['Sum'] -> Datapoints non-empty, Sum == 1 (exactly one Send for this unique dimension combination).
Actual: GetMetricStatistics returns an empty Datapoints list for the whole run; the check's _poll_until (timeout 300.0s, interval 15.0s, description "AWS/SES Send metric resolved for dimensions ...") never satisfies `v is not None`, and the harness terminates the check at 60.0s. No AWS/SES datapoint is produced by the SES send path at all.

Expected (cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare): the mirror image — a Send datapoint IS published for the dimensions the destination declared, while the undeclared dimension produces NO series at all (asserted as an absence held for _ABSENCE_HOLD_S = 60.0s, not as a zero).
Actual: same timeout; neither the positive series nor the asserted absence could be evaluated, because the positive series never exists.

Status/code/message: no divergence in any error payload is involved — both operations in the setup (CreateConfigurationSetEventDestination, SendEmail) are accepted. The divergence is a missing cross-service EFFECT: zero AWS/SES datapoints where AWS delivers one.

Hypothesis (not stated by the model or the recording): the run may stall earlier, in _verified_stand_in, which polls sesv2:GetEmailIdentity for VerifiedForSendingStatus true and DkimAttributes.Status=='SUCCESS' (budget 600.0s) after publishing the returned Easy DKIM CNAMEs into a Route 53 public hosted zone it created in the same target. Either stall point is emulator-side.

## Steps to Reproduce
1. Point an SES/SESv2 client, a Route 53 client and a CloudWatch client at the emulator.
2. Establish a sending domain the way the check does: sesv2:CreateEmailIdentity EmailIdentity=<domain>, take DkimAttributes.Tokens, route53:CreateHostedZone Name=<domain>, then route53:ChangeResourceRecordSets UPSERT a CNAME <token>._domainkey.<domain> -> <token>.dkim.amazonses.com for each token.
3. Poll sesv2:GetEmailIdentity EmailIdentity=<domain> until VerifiedForSendingStatus is true and DkimAttributes.Status == 'SUCCESS'.
   Expected: it becomes verified. Observed: if it never does, the check stalls here (one of the two candidate stall points).
4. sesv2:CreateConfigurationSet ConfigurationSetName=<cs>.
5. sesv2:CreateConfigurationSetEventDestination ConfigurationSetName=<cs>, EventDestinationName='sim2real-cw', EventDestination={"Enabled": true, "MatchingEventTypes": ["SEND"], "CloudWatchDestination": {"DimensionConfigurations": [{"DimensionName": "ses:configuration-set", "DimensionValueSource": "MESSAGE_TAG", "DefaultDimensionValue": "none"}, {"DimensionName": "campaign", "DimensionValueSource": "MESSAGE_TAG", "DefaultDimensionValue": "none"}, {"DimensionName": "X-Custom-Header", "DimensionValueSource": "EMAIL_HEADER", "DefaultDimensionValue": "none"}]}}.
6. sesv2:SendEmail FromEmailAddress=<anything>@<domain>, Destination.ToAddresses=['success@simulator.amazonses.com'], ConfigurationSetName=<cs>, EmailTags=[{"Name": "campaign", "Value": <campaign-value>}], Content.Simple.Headers=[{"Name": "X-Custom-Header", "Value": <header-value>}]. Record the returned MessageId (must be non-empty).
7. Poll cloudwatch:GetMetricStatistics Namespace='AWS/SES', MetricName='Send', Dimensions=[{"Name": "ses:configuration-set", "Value": <cs>}, {"Name": "campaign", "Value": <campaign-value>}, {"Name": "X-Custom-Header", "Value": <header-value>}], StartTime=<send time - 60s>, EndTime=<now + 60s>, Period=60, Statistics=['Sum'].
   Expected: within SES's publishing latency, Datapoints is non-empty and the Sum over the window is exactly 1.
   Observed: Datapoints stays empty for the whole budget; the check is killed at 60.0s.
8. Negative half: repeat steps 4-7 with a destination that declares ONLY the custom dimensions (omitting ses:configuration-set). Expected: a Send datapoint exists for the declared dimensions, and querying with an ses:configuration-set dimension added returns NO series at all (not a series defaulted to 'none'), held for at least 60s. Observed: unverifiable — no AWS/SES series exists either way.

Re-replay both check ids in trace_ids to confirm the fix.
