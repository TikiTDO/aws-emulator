Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-analytics-xacct-put-rejected-past-one-thousand-configurations s3.s3-itc-put-rejected-past-one-thousand-configurations s3.s3-metrics-cfg-new-id-rejected-at-one-thousand-until-one-is-deleted s3.s3-itc-put-rejected-for-a-config-id-over-sixty-four-characters s3.s3-itc-put-rejected-for-a-duplicate-access-tier s3.s3-analytics-xacct-bucket-account-id-is-stored-without-validation s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body s3.s3-website-hosting-e2e-buckets-lifecycle s3.update-object-encryption-error-preconditions
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-analytics-xacct-put-rejected-past-one-thousand-configurations|run#6|AssertionError|assert (code, message) == ("TooManyConfigurations", _LIMIT_MESSAGE), (
Issue-Key: s3.s3-itc-put-rejected-past-one-thousand-configurations|run#11|AssertionError|assert message == _LIMIT_MESSAGE, f"TooManyConfigurations must carry the limit message; got {message!r}"
Issue-Key: s3.s3-metrics-cfg-new-id-rejected-at-one-thousand-until-one-is-deleted|run#7|AssertionError|assert (code, message) == ("TooManyConfigurations", _LIMIT_MESSAGE), (
Issue-Key: s3.s3-itc-put-rejected-for-a-config-id-over-sixty-four-characters|run#11|AssertionError|assert message == _INVALID_ID_MSG, (
Issue-Key: s3.s3-itc-put-rejected-for-a-duplicate-access-tier|run#11|AssertionError|assert message == (
Issue-Key: s3|PutBucketAnalyticsConfiguration|MalformedXML
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body|_error#11|AssertionError|raise AssertionError("the call was expected to be refused and it succeeded")
Issue-Key: s3|GetBucketWebsite|NoSuchWebsiteConfiguration
Issue-Key: s3.update-object-encryption-error-preconditions|run#30|AssertionError|assert message == expected_denial, (
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# Seven S3 fidelity causes: configuration-limit and validation messages, analytics BucketAccountId, website-redirect validation, CFN WebsiteConfiguration and the SSE-KMS denial action

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketIntelligentTieringConfiguration

## Description
Seven net-new root causes in S3.

Cause 1 of 7: the TooManyConfigurations message is not S3's, on every bucket sub-resource that has the 1000-configuration limit.
AWS: recorded behavior only for the wording (the limit itself is S3's documented 1,000 configurations per bucket per type). For PutBucketAnalyticsConfiguration, PutBucketIntelligentTieringConfiguration and PutBucketMetricsConfiguration the recording shows code TooManyConfigurations, HTTP 400 and the exact message 'You have attempted to create more configurations than the 1000 allowed.' — the message is entirely fixed text, with no value from the request in it.
Emulator: code TooManyConfigurations and HTTP 400 are right, but the message is 'You are attempting to create a new configuration but have already reached the 1,000-configuration limit.'
Why: clients and test suites match on the message text to tell this refusal apart from other 400s; the wrong sentence breaks that match on all three configuration types at once.
Fix: the refusal messages in the analytics, intelligent-tiering and metrics configuration handlers' limit validation.
Traces: s3.s3-analytics-xacct-put-rejected-past-one-thousand-configurations, s3.s3-itc-put-rejected-past-one-thousand-configurations, s3.s3-metrics-cfg-new-id-rejected-at-one-thousand-until-one-is-deleted.

Cause 2 of 7: the InvalidConfigId message for an over-long intelligent-tiering Id is reworded.
AWS: recorded behavior only for the wording; code InvalidConfigId, HTTP 400, message 'Config Id should be more than 1 character and less than 64 characters in length.' (all fixed text).
Emulator: 'Config Id should not be more than 1 char and less than 64 chars in length' — different wording and a different meaning ('should not be more than 1 char').
Why: the refusal cannot be matched on, and the sentence tells a caller the opposite of the rule.
Fix: PutBucketIntelligentTieringConfiguration's Id validation message.
Traces: s3.s3-itc-put-rejected-for-a-config-id-over-sixty-four-characters.

Cause 3 of 7: the InvalidAccessTier message for a duplicate access tier is reworded.
AWS: recorded behavior only for the wording; code InvalidAccessTier, HTTP 400, message 'All Access Tiers in a configuration must be unique. Found duplicate entry for ARCHIVE_ACCESS.' — fixed text except the access-tier name, which comes from the request.
Emulator: 'All AccessTiers in Tierings must be unique. Found duplicate entry for ARCHIVE_ACCESS.' (member names instead of S3's prose).
Why: message-matching clients miss the refusal.
Fix: PutBucketIntelligentTieringConfiguration's Tierings uniqueness validation message.
Traces: s3.s3-itc-put-rejected-for-a-duplicate-access-tier.

Cause 4 of 7: PutBucketAnalyticsConfiguration refuses a destination whose BucketAccountId is not a canonical 12-digit account id.
AWS: Smithy — AnalyticsS3BucketDestination.BucketAccountId targets AccountId with no length or pattern constraint, and its documentation says the value is used to validate the destination owner ("If no account ID is provided, the owner is not validated before exporting data"); the recording shows '123', 'abcdefghijkl' and '1111-2222-3333' all accepted and read back verbatim through ListBucketAnalyticsConfigurations.
Emulator: the first such Put is refused with code MalformedXML, HTTP 400, 'The XML you provided was not well-formed or did not validate against our published schema' — a well-formed document that validates against the schema is rejected, and nothing is stored.
Why: a client cannot configure a cross-account analytics export at all, and the error blames its XML rather than the value; MalformedXML is also the wrong class for a value-level complaint.
Fix: PutBucketAnalyticsConfiguration's input validation — treat BucketAccountId as an opaque string and store it verbatim.
Traces: s3.s3-analytics-xacct-bucket-account-id-is-stored-without-validation.

Cause 5 of 7: PutObject accepts a WebsiteRedirectLocation that is neither '/'-rooted nor http(s).
AWS: Smithy — PutObject carries WebsiteRedirectLocation as the x-amz-website-redirect-location header; the recording shows a value like 'new.html' refused with code InvalidRedirectLocation, HTTP 400 and the message "The website redirect location must have a prefix of 'http://' or 'https://' or '/'." (fixed text), and the object not created (a following GetObject is NoSuchKey / 'The specified key does not exist.').
Emulator: the PutObject succeeds, so the object exists with an invalid redirect location.
Why: an invalid redirect is stored and later served by the website endpoint as a 301 to a location browsers resolve differently, and a client's validation test passes where AWS refuses.
Fix: PutObject's WebsiteRedirectLocation validation.
Traces: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body.

Cause 6 of 7: a CloudFormation-created bucket that declares WebsiteConfiguration has no website configuration.
AWS: recorded behavior only — AWS::S3::Bucket's WebsiteConfiguration is applied at create, so GetBucketWebsite on the stack's bucket returns the declared IndexDocument (Suffix 'index.html') rather than an error.
Emulator: the stack reaches its deploy phase and GetBucketWebsite answers NoSuchWebsiteConfiguration / 'The specified bucket does not have a website configuration'.
Why: static-website stacks deploy "successfully" but the website endpoint serves nothing, and a client reading back its own template's configuration gets an error.
Fix: the CloudFormation provisioner for AWS::S3::Bucket — apply the template's WebsiteConfiguration (equivalent to PutBucketWebsite) when the bucket is created and on update.
Traces: s3.s3-website-hosting-e2e-buckets-lifecycle.

Cause 7 of 7: an SSE-KMS write denied by the key policy names the wrong KMS action.
AWS: recorded behavior only for the message; the AccessDenied message names kms:Encrypt — 'User: <caller arn> is not authorized to perform: kms:Encrypt on resource: <key arn> with an explicit deny in a resource-based policy'. The fixed text is 'is not authorized to perform: kms:Encrypt on resource: ... with an explicit deny in a resource-based policy'; the caller ARN and key ARN come from the request and state.
Emulator: the same sentence naming kms:GenerateDataKey.
Why: a client diagnosing a key-policy denial (or writing the policy statement that would fix it) is pointed at the wrong action, so the repair it makes does not grant what S3 actually needs.
Fix: the SSE-KMS write path's authorization error construction — name the action the operation is authorized against in the denial message.
Traces: s3.update-object-encryption-error-preconditions.

## Evidence
All under s3:
[mismatch] s3-analytics-xacct-put-rejected-past-one-thousand-configurations — AssertionError: a NEW configuration Id past the 1000-per-bucket limit must be rejected with the 1,000-configuration limit message; got ('TooManyConfigurations', 'You are attempting to create a new configuration but have already reached the 1,000-configuration limit.'); the check's constant is 'You have attempted to create more configurations than the 1000 allowed.'
[mismatch] s3-itc-put-rejected-past-one-thousand-configurations — same got/expected pair
[mismatch] s3-metrics-cfg-new-id-rejected-at-one-thousand-until-one-is-deleted — same got/expected pair
[mismatch] s3-itc-put-rejected-for-a-config-id-over-sixty-four-characters — must carry 'Config Id should be more than 1 character and less than 64 characters in length.'; got 'Config Id should not be more than 1 char and less than 64 chars in length'
[mismatch] s3-itc-put-rejected-for-a-duplicate-access-tier — must carry 'All Access Tiers in a configuration must be unique. Found duplicate entry for ARCHIVE_ACCESS.'; got 'All AccessTiers in Tierings must be unique. Found duplicate entry for ARCHIVE_ACCESS.'
[error] s3-analytics-xacct-bucket-account-id-is-stored-without-validation — ClientError (MalformedXML) on PutBucketAnalyticsConfiguration: 'The XML you provided was not well-formed or did not validate against our published schema' for BucketAccountId values '123' / 'abcdefghijkl' / '1111-2222-3333'
[mismatch] s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body — AssertionError: the call was expected to be refused and it succeeded (PutObject with WebsiteRedirectLocation='new.html'; expected InvalidRedirectLocation/400 "The website redirect location must have a prefix of 'http://' or 'https://' or '/'.")
[error] s3-website-hosting-e2e-buckets-lifecycle — [phase 1/4 deploy] ClientError (NoSuchWebsiteConfiguration) on GetBucketWebsite: 'The specified bucket does not have a website configuration'
[mismatch] update-object-encryption-error-preconditions — expected 'User: <arn> is not authorized to perform: kms:Encrypt on resource: <key arn> with an explicit deny in a resource-based policy', got the same sentence with kms:GenerateDataKey

## Steps to Reproduce
Cause 1: put 1000 analytics (resp. intelligent-tiering, metrics) configurations on a bucket, then put a 1001st new Id and read Error.Message.
Cause 2: PutBucketIntelligentTieringConfiguration with Id of 65 characters and read Error.Message.
Cause 3: PutBucketIntelligentTieringConfiguration with two Tierings both AccessTier=ARCHIVE_ACCESS and read Error.Message.
Cause 4: PutBucketAnalyticsConfiguration with StorageClassAnalysis.DataExport.Destination.S3BucketDestination.BucketAccountId='123' — refused MalformedXML instead of stored verbatim.
Cause 5: PutObject with WebsiteRedirectLocation='new.html' — succeeds instead of InvalidRedirectLocation/400.
Cause 6: CreateStack with an AWS::S3::Bucket declaring WebsiteConfiguration.IndexDocument, then GetBucketWebsite on the created bucket — NoSuchWebsiteConfiguration.
Cause 7: deny kms:Encrypt/GenerateDataKey for the caller in a CMK's key policy and PutObject with SSE-KMS under it; read the AccessDenied message's action name.
