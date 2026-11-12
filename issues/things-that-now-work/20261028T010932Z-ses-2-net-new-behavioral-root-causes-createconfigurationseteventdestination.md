Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.event-destination-unowned-sns-topic-fails ses.list-identities-pagination-exactness-v1-and-v2
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.event-destination-unowned-sns-topic-fails|_err#16|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ses|ListEmailIdentities|BadRequestException
Issue-Commit: f5d2818b51255ceb308b62fff342c02a5f2a7669

# ses: 2 net-new behavioral root causes — CreateConfigurationSetEventDestination accepts an SNSDestination whose SNS topic does not exist (200 instead of InvalidSNSDestination), and the SESv2 ListEmailIdentities forged-NextToken BadRequestException message is emulator-authored

> _Note: this issue was identified against the code base at commit `f5d2818b51255ceb308b62fff342c02a5f2a7669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSetEventDestination

## Description
Two independent, net-new root causes in the SES surface. Both are grounded in failing behavioral checks the replay report marked neither ALREADY FILED nor OWNED. Neither is covered by an open issue: 20261026T065304Z-ses-6-net-new-behavioral-root-causes-sesv2.md covers the identity-notification plane (SetIdentityNotificationTopic's bogus-ARN refusal), not the configuration-set event-destination SNS existence check; 20261027T184055Z-ses-2-net-new-behavioral-root-causes-the-275170.md covers the forged-cursor message of ListDedicatedIpPools, a different operation with a different sentence, and says nothing about ListEmailIdentities; 20261025T214303Z-ses-identity-verification-is-auto-completed-and-the.md owns the identity-verification store and the v1 ListIdentities forged-token refusal, not the SESv2 ListEmailIdentities one.

=== Cause 1 of 2: CreateConfigurationSetEventDestination (SES v1) accepts an SNSDestination pointing at an SNS topic that does not exist, answering HTTP 200, instead of refusing it with InvalidSNSDestination ===

AWS: SES validates an event destination's target at create time. For an SNSDestination whose TopicARN is well formed and in the caller's own account and region but names no existing topic, the call is refused and the destination is NOT attached to the configuration set. Smithy: CreateConfigurationSetEventDestination (input ConfigurationSetName required, EventDestination required) models the client error InvalidSNSDestinationException, httpError 400, awsQueryError code "InvalidSNSDestination" ("Indicates that the Amazon Simple Notification Service (Amazon SNS) destination is invalid. See the error message for details."). Recorded behavior for the message: the fixed form "SNS topic <" + TopicARN + "> not found." — fixed text "SNS topic <" and "> not found.", with the single variable part taken verbatim from the request's EventDestination.SNSDestination.TopicARN. The same request becomes legal once the topic exists: creating the topic the ARN names and re-issuing the identical call must succeed and DescribeConfigurationSet(ConfigurationSetAttributeNames=["eventDestinations"]) must then report that destination with the same TopicARN.

Emulator: the call SUCCEEDS. With a configuration set that already carries a working destination on an owned topic (the check's positive control, which passed), a second destination whose SNSDestination.TopicARN is arn:aws:sns:<region>:<account>:<name-of-a-topic-that-was-never-created> is answered HTTP 200 with an empty body (ResponseMetadata HTTPStatusCode 200, content-type text/xml) and no error at all. The replay assertion is "expected a ClientError, but the call SUCCEEDED and returned {... 'HTTPStatusCode': 200 ...}". So the handler never resolves the TopicARN against SNS state; the destination is attached to a dangling target.

Why: a client gets no signal that its event pipeline is broken. On AWS, a typo in a topic ARN, or a topic deleted before the destination is wired up, fails loudly at configuration time with InvalidSNSDestination and the destination stays unattached, so the caller fixes it before any mail is sent. On the emulator the mis-wired destination is persisted and every matching event (send, bounce, complaint...) is published into nothing; the failure only surfaces much later as missing notifications with no error to trace. It also makes the restore leg untestable: a caller cannot distinguish "topic missing, create it and retry" from "accepted", because both are 200. Validation-order and negative-class tests that pin InvalidSNSDestination see a success instead of a 400.

Fix: the CreateConfigurationSetEventDestination handler's EventDestination validation in the ses crate (services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_configuration_set_event_destination.rs) — it must resolve the SNSDestination.TopicARN against the SNS topic store for the caller's account/region before mutating the configuration set, and when no such topic exists refuse with 400 / InvalidSNSDestination and the message above, leaving the configuration set's event-destination list unchanged. The same resolution belongs on the update path so a destination cannot be repointed at a dangling topic.

Failing check: ses.event-destination-unowned-sns-topic-fails

=== Cause 2 of 2: SESv2 ListEmailIdentities refuses a forged NextToken with an emulator-authored message ("NextToken is not a token returned by ListEmailIdentities.") instead of AWS's "Invalid NextToken <token>." ===

AWS: a NextToken this service never issued is refused with BadRequestException. Smithy: ListEmailIdentities (POST /v2/email/list-identities, input NextToken + PageSize, paginated inputToken/outputToken NextToken) models exactly BadRequestException (httpError 400) and TooManyRequestsException (429) — no InvalidNextTokenException. Recorded behavior for the message: the fixed form "Invalid NextToken <" + the rejected token + ">." — fixed text "Invalid NextToken <" and ">.", with the one variable part being the NextToken value copied from the request (so the sentence echoes the caller's token, whatever it is).

Emulator: the HTTP status and error code are right (400 BadRequestException), but the message is the emulator's own sentence "NextToken is not a token returned by ListEmailIdentities." — a different wording that also drops the echoed token. Replay assertion: expected message 'Invalid NextToken <c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=>.', got 'NextToken is not a token returned by ListEmailIdentities.' (the base64 string there is the per-run forged cursor the check sent; it is the variable slot, not part of the requirement). The earlier legs of the same check — PageSize=1 walk, no duplicates, paginated union equals the single-shot read — passed, so only the refusal wording diverges, and the v1 ListIdentities legs after it were never reached.

Why: the error message is part of the contract. A client, test suite, or operator that branches on, logs, or asserts the bad-cursor sentence sees text AWS never sends, so a corrupt/stale cursor is not recognizable as the same condition it is in production; and because the emulator's sentence omits the rejected token, a caller debugging a cursor it cannot see in its own logs loses the one value AWS hands back. Any parity test pinned to AWS's wording fails here even though the status and code are correct.

Fix: the ListEmailIdentities handler's NextToken validation / its BadRequestException construction in the sesv2 crate (the page-cursor refusal path used by the email-identity listing), so the message is AWS's "Invalid NextToken <...>." with the request's token interpolated. Note the sibling defect already filed for ListDedicatedIpPools (20261027T184055Z-...-275170.md) is a DIFFERENT sentence ("Invalid next token.", no token echo) for a different operation — fixing one does not fix the other, and the two must not be collapsed onto one shared string.

Failing check: ses.list-identities-pagination-exactness-v1-and-v2

=== Traces ===
Cause 1: ses.event-destination-unowned-sns-topic-fails
Cause 2: ses.list-identities-pagination-exactness-v1-and-v2

## Evidence
Cause 1 — check ses.event-destination-unowned-sns-topic-fails (mismatch):
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'RequestId': '...', 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '335', ...}, 'RetryAttempts': 0}}
  Expected (AWS): HTTP 400, Code=InvalidSNSDestination, Message="SNS topic <arn:aws:sns:<region>:<account>:<topic-name>> not found." (fixed text "SNS topic <" / "> not found."; the ARN is taken from the request's EventDestination.SNSDestination.TopicARN), and the destination must NOT appear in DescribeConfigurationSet(ConfigurationSetAttributeNames=["eventDestinations"]).
  Actual (emulator): HTTP 200, empty CreateConfigurationSetEventDestinationResponse, no error — the dangling destination is accepted.
  Positive control in the same check passed: a destination whose SNSDestination.TopicARN names a topic that really was created attaches fine and is reported by DescribeConfigurationSet, so SNS event destinations are not broken wholesale — only the existence check is missing.
  Blocked downstream by the same cause: the restore leg (create the topic the ARN names, then re-issue the identical call and expect it to succeed with that TopicARN) is unobservable, since the first call already succeeded.

Cause 2 — check ses.list-identities-pagination-exactness-v1-and-v2 (mismatch):
  AssertionError: expected message 'Invalid NextToken <c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=>.', got 'NextToken is not a token returned by ListEmailIdentities.'
  Expected (AWS): 400 BadRequestException, Message="Invalid NextToken <" + <the NextToken sent> + ">."
  Actual (emulator): 400 BadRequestException, Message="NextToken is not a token returned by ListEmailIdentities." (status and code correct; wording diverges and the echoed token is dropped).
  Earlier legs that passed: PageSize=1 produced one item per page, no duplicates across pages, every created identity present, and the paginated union equalled the PageSize=1000 single-shot read — so pagination itself is exact and only the forged-cursor message diverges.

## Steps to Reproduce
Cause 1 (ses.event-destination-unowned-sns-topic-fails):
1. ses.create_configuration_set(ConfigurationSet={"Name": <cfg>}).
2. Positive control: sns.create_topic(Name=<live>) then ses.create_configuration_set_event_destination(ConfigurationSetName=<cfg>, EventDestination={"Name": <dest-live>, "Enabled": true, "MatchingEventTypes": ["send"], "SNSDestination": {"TopicARN": <live topic arn>}}) and confirm <dest-live> is listed by ses.describe_configuration_set(ConfigurationSetName=<cfg>, ConfigurationSetAttributeNames=["eventDestinations"]). This already works.
3. Build a well-formed ARN in the caller's own account and region for a topic that was never created: arn:aws:sns:<region>:<account>:<ghost>. Call ses.create_configuration_set_event_destination(ConfigurationSetName=<cfg>, EventDestination={"Name": <dest>, "Enabled": true, "MatchingEventTypes": ["send"], "SNSDestination": {"TopicARN": <ghost arn>}}).
   Expected: ClientError 400 InvalidSNSDestination, Message "SNS topic <<ghost arn>> not found."; and <dest> absent from describe_configuration_set's EventDestinations.
   Actual: HTTP 200, no error, destination accepted.
4. (Restore leg, blocked by the same cause) sns.create_topic(Name=<ghost>) then re-issue the identical create call: it must now succeed and describe_configuration_set must report <dest> with SNSDestination.TopicARN == <ghost arn>.

Cause 2 (ses.list-identities-pagination-exactness-v1-and-v2):
1. Create a few SESv2 email identities: sesv2.create_email_identity for three fresh domains plus one email address.
2. Walk sesv2.list_email_identities(PageSize=1) to exhaustion, then sesv2.list_email_identities(PageSize=1000) — both agree (these legs pass).
3. sesv2.list_email_identities(PageSize=1, NextToken="c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=") — a cursor this service never issued.
   Expected: ClientError 400 BadRequestException, Message "Invalid NextToken <c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=>." (the token echoed from the request).
   Actual: 400 BadRequestException, Message "NextToken is not a token returned by ListEmailIdentities."
4. (Blocked downstream) the v1 legs — ses.list_identities(MaxItems=1) walk over the same store and the IdentityType=Domain / EmailAddress partition — were never reached.
