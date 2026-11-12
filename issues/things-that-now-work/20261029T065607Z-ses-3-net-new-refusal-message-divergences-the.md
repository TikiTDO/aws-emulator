Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.custom-verification-template-unverified-from-fails ses.delete-email-identity-cascades-policies-and-tags ses.event-destination-lifecycle
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.custom-verification-template-unverified-from-fails|run#9|AssertionError|assert message == expected_message, (
Issue-Key: ses.delete-email-identity-cascades-policies-and-tags|run#20|AssertionError|assert (err_tags.response['Error']['Code'], err_tags.response['Error'].get('Message')) == (
Issue-Key: ses.event-destination-lifecycle|run#25|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Commit: 5d001f011ed28660c188b78afabc22e4b6c273f2

# ses: 3 net-new refusal-message divergences — the v1 FromEmailAddressNotVerified sentence brackets the address, the SESv2 tagging not-found sentence names the ARN instead of the identity, and the v1 EventDestinationDoesNotExist sentence names the destination instead of the configuration set

> _Note: this issue was identified against the code base at commit `5d001f011ed28660c188b78afabc22e4b6c273f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConfigurationSetEventDestination

## Description
Three independent, net-new root causes in the SES refusal surface. All three calls are refused with the CORRECT error class and status; in each case the emulator renders an emulator-authored sentence where AWS renders a different one, so a client that reads the message (or parses an identifier out of it) is misled. None of these three is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 3: ses (v1) CreateCustomVerificationEmailTemplate's FromEmailAddressNotVerified refusal message is re-worded and brackets the address.
AWS: Smithy com.amazonaws.ses#SimpleEmailService (awsQuery) CreateCustomVerificationEmailTemplate, input {TemplateName, FromEmailAddress: FromAddress, TemplateSubject, TemplateContent, SuccessRedirectionURL, FailureRedirectionURL — all required}, output Unit; modelled error FromEmailAddressNotVerifiedException with awsQueryError code "FromEmailAddressNotVerified", httpResponseCode 400. The model does not carry the sentence, so recorded behavior only: Error.Message is "The from email address <FromEmailAddress> is not verified." — the fixed text is "The from email address " ... " is not verified." and the single variable part is the request's FromEmailAddress, interpolated BARE (no angle brackets around it).
Emulator: same 400 and same code "FromEmailAddressNotVerified", but Error.Message is "From email address <ADDR> is not verified." — fixed text "From email address <" + the request's FromEmailAddress + "> is not verified." Two divergences in one sentence: the leading "The from email address" is replaced by "From email address", and the address is wrapped in angle brackets AWS does not emit.
Why: a client (or an SDK-level test, or a retry/branching rule) that matches the documented sentence does not recognise this refusal, and anything that lifts the offending address out of the message gets "<addr>" instead of "addr".
Fix: the refusal construction in that operation's handler, services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_custom_verification_email_template.rs — the message template only, not the gate's condition. Note: open issue 20261029T023243Z-ses-createcustomverificationemailtemplate-s-new-fromemailaddressnotverified-gate-refuses-every.md covers a DIFFERENT cause on this same operation (the gate refuses every sender, so the 200 path is unreachable) and explicitly states "The defect is the status code/condition ..., not the message wording"; this cause is the wording and is not covered there.
Traces: ses.custom-verification-template-unverified-from-fails (the negative half now reaches and fails the message assertion).

Cause 2 of 3: SESv2 ListTagsForResource's NotFoundException for an email-identity ARN that resolves to nothing renders a generic resource sentence naming the whole ARN, where AWS names the identity.
AWS: Smithy com.amazonaws.sesv2#SimpleEmailService_v2 ListTagsForResource, GET /v2/email/tags with ResourceArn as a required httpQuery (@length min 1); modelled errors BadRequestException (400), NotFoundException (404), TooManyRequestsException (429). The model does not carry the sentence, so recorded behavior only: for a ResourceArn of the form arn:aws:ses:<region>:<account>:identity/<identity> whose identity does not exist, the response is 404 NotFoundException with Error.Message "No EmailIdentity present with name: <identity>" — fixed text "No EmailIdentity present with name: " and the variable part is the IDENTITY NAME parsed out of the request's ResourceArn (the segment after "identity/"), with no trailing period and no brackets.
Emulator: the status and code are right (404 NotFoundException) but Error.Message is "Resource arn:aws:ses:<region>:<account>:identity/<identity> does not exist." — fixed text "Resource " + the request's whole ResourceArn verbatim + " does not exist." AWS's sentence is resource-type specific and carries only the name; the emulator's is a generic ARN sentence. The sibling read on the same deleted identity, GetEmailIdentityPolicies, already renders AWS's wording ("Email identity <identity> does not exist.") and passes, so the divergence is confined to the tagging plane's not-found refusal.
Why: a client distinguishing "this ARN is not an SES resource" (BadRequestException wording) from "this email identity is gone" reads the message, and tooling that extracts the missing identity's name from the sentence gets a full ARN or nothing.
Fix: the not-found refusal of the SESv2 tagging read — services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_tags_for_resource.rs and whatever shared ARN-to-resource resolution it calls, which should raise the identity-specific sentence for an identity ARN (and keep the same sentence on the matching UntagResource/TagResource not-found path).
Traces: ses.delete-email-identity-cascades-policies-and-tags. Related but distinct open issue: 20261028T171153Z-ses-2-net-new-refusal-fidelity-root-causes.md (SESv2 TagResource RESERVED-PREFIX refusal wording) — a different operation and a different refusal class.

Cause 3 of 3: ses (v1) UpdateConfigurationSetEventDestination's EventDestinationDoesNotExist refusal message names the destination and the configuration set, where AWS names only the configuration set.
AWS: Smithy com.amazonaws.ses#SimpleEmailService (awsQuery) UpdateConfigurationSetEventDestination, input {ConfigurationSetName, EventDestination — both required}; modelled errors ConfigurationSetDoesNotExistException, EventDestinationDoesNotExistException (awsQueryError code "EventDestinationDoesNotExist", httpResponseCode 400), InvalidCloudWatchDestinationException, InvalidFirehoseDestinationException, InvalidSNSDestinationException. The model does not carry the sentence, so recorded behavior only: updating a destination that was deleted (or never existed) on an existing configuration set is refused 400 EventDestinationDoesNotExist with Error.Message "No EventDestination found for <ConfigurationSetName>" — fixed text "No EventDestination found for " and the single variable part is the request's ConfigurationSetName, with NO trailing period and with the EventDestination.Name absent from the sentence.
Emulator: same 400 and code, but Error.Message is "Event destination <NAME> does not exist for configuration set <CFGSET>." — fixed text "Event destination <" + EventDestination.Name + "> does not exist for configuration set <" + ConfigurationSetName + ">.", i.e. an emulator-authored sentence with an extra interpolated value, brackets and a trailing period.
Why: the message is a requirement of this error class; a client asserting on, or logging/branching from, AWS's sentence sees an unrecognised one, and the sentence shape implies a per-destination lookup AWS does not report.
Fix: the refusal construction in services/service-com-amazonaws-ses-simpleemailservice/src/ops/update_configuration_set_event_destination.rs (and the shared destination-lookup helper it uses, so the sibling delete_configuration_set_event_destination.rs renders the same AWS sentence).
Traces: ses.event-destination-lifecycle (the create/describe/update/delete lifecycle and the post-delete refusal's CODE all pass; only the message assertion fails).

## Evidence
Cause 1 — ses.custom-verification-template-unverified-from-fails, check line 54:
  Expected: Error.Code "FromEmailAddressNotVerified" with Error.Message "The from email address <addr> is not verified."
  Actual:   Error.Code "FromEmailAddressNotVerified" with Error.Message "From email address <<addr>> is not verified."
  Replay: AssertionError: expected message 'The from email address sim2real-behav-cvfrom-39a2ef52ffe0@example.com is not verified.', got 'From email address <sim2real-behav-cvfrom-39a2ef52ffe0@example.com> is not verified.'
  (The address is a per-run generated value from the request; the requirement is the surrounding fixed text and the bare, unbracketed interpolation.)

Cause 2 — ses.delete-email-identity-cascades-policies-and-tags, check line 99, after sesv2 DeleteEmailIdentity on the subject identity:
  Expected: sesv2 ListTagsForResource(ResourceArn=arn:aws:ses:<region>:<account>:identity/<identity>) -> 404 NotFoundException, Error.Message "No EmailIdentity present with name: <identity>"
  Actual:   404 NotFoundException, Error.Message "Resource arn:aws:ses:<region>:<account>:identity/<identity> does not exist."
  Replay: AssertionError: reading tags of a deleted identity's ARN must be NotFoundException, got {'Message': 'Resource arn:aws:ses:us-west-2:000000000000:identity/sim2real-behav-cascade-323153aeee72.example.com does not exist.', 'Code': 'NotFoundException'}
  The immediately preceding assertion on GetEmailIdentityPolicies for the SAME deleted identity passed with AWS's wording "Email identity <identity> does not exist.", which localises the defect to the tagging read. (region/account/identity are per-run values.)

Cause 3 — ses.event-destination-lifecycle, check line 113, after DeleteConfigurationSetEventDestination succeeded and DescribeConfigurationSet confirmed the destination is gone:
  Expected: UpdateConfigurationSetEventDestination -> 400 EventDestinationDoesNotExist, Error.Message "No EventDestination found for <ConfigurationSetName>"
  Actual:   400 EventDestinationDoesNotExist, Error.Message "Event destination <<EventDestination.Name>> does not exist for configuration set <<ConfigurationSetName>>."
  Replay: AssertionError: expected message 'No EventDestination found for sim2real-behav-cfgset-027b26268c8c', got 'Event destination <sim2real-behav-dest-58ed188f8639> does not exist for configuration set <sim2real-behav-cfgset-027b26268c8c>.'
  (The two names are per-run generated values; the requirement is that the sentence is "No EventDestination found for " + the request's ConfigurationSetName, with no destination name and no trailing period.)

## Steps to Reproduce
Boot the emulator and point boto3 at it.

Cause 1 (ses v1, awsQuery):
1. ses.verify_email_identity(EmailAddress='probe-<unique>@example.com').
2. ses.create_custom_verification_email_template(TemplateName='<unique>', FromEmailAddress='probe-<unique>@example.com', TemplateSubject='Please verify', TemplateContent='<html><body>Please verify your address.</body></html>', SuccessRedirectionURL='https://example.com/success', FailureRedirectionURL='https://example.com/failure').
3. Inspect the ClientError: Code is FromEmailAddressNotVerified (correct) but Message is 'From email address <probe-<unique>@example.com> is not verified.' instead of 'The from email address probe-<unique>@example.com is not verified.'

Cause 2 (sesv2):
1. sesv2.create_email_identity(EmailIdentity='cascade-<unique>.example.com', Tags=[{'Key':'sim2real-role','Value':'cascade'}]).
2. sesv2.delete_email_identity(EmailIdentity='cascade-<unique>.example.com').
3. sesv2.list_tags_for_resource(ResourceArn='arn:aws:ses:<region>:<account>:identity/cascade-<unique>.example.com') — observe 404 NotFoundException with Message 'Resource arn:... does not exist.' instead of 'No EmailIdentity present with name: cascade-<unique>.example.com'.
4. For contrast, sesv2.get_email_identity_policies(EmailIdentity='cascade-<unique>.example.com') on the same deleted identity returns the correct sentence 'Email identity <cascade-<unique>.example.com> does not exist.'

Cause 3 (ses v1, awsQuery):
1. ses.create_configuration_set(ConfigurationSet={'Name':'cfgset-<unique>'}).
2. ses.create_configuration_set_event_destination(ConfigurationSetName='cfgset-<unique>', EventDestination={'Name':'dest-<unique>','Enabled':True,'MatchingEventTypes':['send','bounce'],'CloudWatchDestination':{'DimensionConfigurations':[{'DimensionName':'probe-dimension','DimensionValueSource':'messageTag','DefaultDimensionValue':'none'}]}}).
3. ses.delete_configuration_set_event_destination(ConfigurationSetName='cfgset-<unique>', EventDestinationName='dest-<unique>').
4. Re-issue the update from step 2 via ses.update_configuration_set_event_destination with the same EventDestination body — observe 400 EventDestinationDoesNotExist with Message 'Event destination <dest-<unique>> does not exist for configuration set <cfgset-<unique>>.' instead of 'No EventDestination found for cfgset-<unique>'.

Then re-replay the checks in trace_ids.
