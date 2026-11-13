Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact
Issue-Key: s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact|run#33|Timeout|time.sleep(10)
Issue-Commit: a897c1908ff5595645e24c2ee983a2dd8fc3b256

# s3__ses__sesv2: a raw multipart message accepted by ses SendRawEmail is never delivered to the active receipt rule's S3Action bucket, so byte-exact attachment survival is unobservable

> _Note: this issue was identified against the code base at commit `a897c1908ff5595645e24c2ee983a2dd8fc3b256`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendRawEmail

## Description
Cause 1 of 1: the SES inbound (email-receiving) path never stores a raw message, so a SendRawEmail accepted for a recipient matched by the account's active receipt rule set produces no object in the rule's S3Action bucket and the check runs out the harness clock inside its bounded wait. Failing check id for this cause: s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact.

AWS: recorded behavior only for the delivery itself — no single operation returns it; it is observed through s3:ListObjectsV2/GetObject on the destination bucket. The modelled pieces around it: Smithy com.amazonaws.ses#SimpleEmailService (awsQuery) SendRawEmail input {Source (Address), Destinations (AddressList), RawMessage required, FromArn/SourceArn/ReturnPathArn, ConfigurationSetName, Tags}, output {MessageId required}, errors MessageRejected (httpError 400), MailFromDomainNotVerifiedException, ConfigurationSetDoesNotExist, ConfigurationSetSendingPausedException, AccountSendingPausedException; com.amazonaws.ses#S3Action {BucketName required, ObjectKeyPrefix, KmsKeyArn, TopicArn}; SetActiveReceiptRuleSet / DescribeActiveReceiptRuleSet select the one rule set the account evaluates. Behaviorally: with a domain verified for the account and an active receipt rule whose Recipients match the envelope recipient, a matching rule's S3Action writes the raw RFC 5322 bytes as a single object under <ObjectKeyPrefix><messageId> in the named bucket (plus one AMAZON_SES_SETUP_NOTIFICATION object when a rule set is first activated). Reading that object back and parsing it yields the Subject exactly as sent, and the MIME part whose Content-Disposition filename equals the sent filename decodes (base64) to bytes byte-identical to what was attached — same SHA-256, same length — with Content-Type application/octet-stream preserved.

Emulator: every API call in the pipeline now succeeds — s3.CreateBucket, s3.PutBucketPolicy (Allow ses.amazonaws.com s3:PutObject on arn:aws:s3:::<bucket>/raw/* conditioned on AWS:SourceAccount), ses.CreateReceiptRuleSet, ses.CreateReceiptRule with Actions=[{S3Action:{BucketName,ObjectKeyPrefix:"raw/"}}], ses.DescribeActiveReceiptRuleSet, ses.SetActiveReceiptRuleSet, and ses.SendRawEmail (no ClientError is raised anywhere) — but s3.ListObjectsV2(Bucket=<bucket>, Prefix="raw/") never returns any key other than (at most) the setup notification. The check is killed inside its bounded 600 s poll and the replay report carries only `Error: Check timed out after 60.0s`. Hypothesis: there is no inbound delivery pipeline — a receipt rule is stored as inert configuration and an accepted SendRawEmail is not looped back through the active rule set, so no S3Action ever fires.

Why: the whole receiving half of SES is unusable for raw/MIME traffic. A client that sends to one of its own verified domains and waits for the message to appear in S3 — the canonical inbound-processing pattern — waits forever with no error to distinguish "not delivered yet" from "never will be", and nothing about S3Action (ObjectKeyPrefix placement, KmsKeyArn encryption, byte-exact raw MIME, the headers SES adds) can be observed or tested. Binary-attachment integrity in particular — the guarantee that what was attached is what is stored — is silently ungradeable rather than reported as unsupported.

Fix: the SES inbound delivery path that ties the crates together — a message accepted by the v1 SendRawEmail handler (services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_raw_email.rs) for an envelope recipient on a locally verified domain must be evaluated against the account's ACTIVE receipt rule set (the state SetActiveReceiptRuleSet/DescribeActiveReceiptRuleSet own), and a matching rule's S3Action must write the raw message bytes unaltered into the named bucket under its ObjectKeyPrefix (honouring KmsKeyArn), preserving the submitted MIME structure, part Content-Types, Content-Disposition filenames and transfer encodings. Named related issues, none of which covers this check's current cause: 20261028T020253Z-s3-ses-sesv2-ses-sendrawemail-refuses-a-source.md carries THIS check id but describes a now-superseded failure mode (SendRawEmail refusing the Source with 400 MessageRejected); the send is no longer refused, which is why its replay key (ses|SendRawEmail|MessageRejected) no longer matches and the report does not mark this check owned — its fix moved the failure one stage later rather than making the flow work. 20261027T223106Z-s3-ses-sesv2-inbound-email-receiving-never-completes.md and 20261027T223115Z-s3-ses-sesv2-ses-email-receiving-never-runs.md report the same missing receive path for sesv2 SendEmail and own the other three s3__ses__sesv2 checks; neither carries this check's replay key nor covers the SendRawEmail/raw-MIME path.

Attribution note: the check id carries the composite service token `s3__ses__sesv2`; all three constituent crates (service-com-amazonaws-s3-amazons3, service-com-amazonaws-ses-simpleemailservice, service-com-amazonaws-sesv2-simpleemailservice-v2) are bootstrapped, and the missing behavior belongs to the SES receive path.

Traces: s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact.

## Evidence
Cause 1 — s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact [mismatch]

Request sequence (all calls accepted by the emulator):
  1. route53.create_hosted_zone for a stand-in domain D; sesv2.create_email_identity(EmailIdentity=D); UPSERT each DkimAttributes.Tokens CNAME `<token>._domainkey.D -> <token>.dkim.amazonses.com` plus `D MX 10 inbound-smtp.<region>.amazonaws.com`; wait until sesv2.get_email_identity(D) reports VerifiedForSendingStatus true and DkimAttributes.Status "SUCCESS" (reached).
  2. s3.create_bucket(Bucket=B); s3.put_bucket_policy(B, Allow Principal {"Service":"ses.amazonaws.com"} s3:PutObject on arn:aws:s3:::B/raw/* with Condition StringEquals AWS:SourceAccount=<account>).
  3. ses.create_receipt_rule_set(RuleSetName=RS); ses.create_receipt_rule(RuleSetName=RS, Rule={Name:"sim2real-attach-to-s3", Enabled:true, ScanEnabled:false, Recipients:[<rcpt@D>], Actions:[{S3Action:{BucketName:B, ObjectKeyPrefix:"raw/"}}]}); ses.describe_active_receipt_rule_set(); ses.set_active_receipt_rule_set(RuleSetName=RS); fixed 20 s settle.
  4. ses.send_raw_email(Source=<sender@D>, Destinations=[<rcpt@D>], RawMessage={"Data": <multipart/mixed: a text/plain part plus an application/octet-stream part with Content-Disposition attachment; filename="sim2real-attach-<token>.bin" carrying 1024 bytes of binary payload>}) — returns a MessageId, no error.
  5. Poll s3.list_objects_v2(Bucket=B, Prefix="raw/") for up to 600 s for any key not ending in AMAZON_SES_SETUP_NOTIFICATION.

Expected (AWS): exactly one such object appears; s3.get_object on it returns the raw message, whose Subject equals the probe token; the MIME part whose filename equals the sent filename has Content-Type application/octet-stream and its base64-decoded payload has the same SHA-256 and the same byte length as the attachment that was sent.

Actual (emulator): the listing stays empty of any delivered message for the whole bounded wait — no object is ever written under raw/ — so the check is still inside the poll when the harness kills it. Replay detail, verbatim and complete: `Error: Check timed out after 60.0s`. No ClientError is raised by any call in steps 1-4, so the refusal previously reported for this check (400 MessageRejected on SendRawEmail) no longer occurs; the flow now stalls one stage later, at delivery. None of the byte-exactness assertions (Subject preserved, part located by filename, Content-Type application/octet-stream, decoded SHA-256 equal to the sent payload's) is ever evaluated.

## Steps to Reproduce
1. Boot the emulator and point boto3 clients 'route53', 'sesv2', 'ses' and 's3' at it (any receiving region, e.g. us-west-2).
2. route53.create_hosted_zone(Name='d1.example.net', CallerReference=<uuid>); sesv2.create_email_identity(EmailIdentity='d1.example.net'); UPSERT each DkimAttributes.Tokens as '<token>._domainkey.d1.example.net' CNAME -> '<token>.dkim.amazonses.com', plus 'd1.example.net' MX -> '10 inbound-smtp.<region>.amazonaws.com'.
3. Poll sesv2.get_email_identity(EmailIdentity='d1.example.net') until VerifiedForSendingStatus is true and DkimAttributes.Status == 'SUCCESS'.
4. s3.create_bucket(Bucket=B) and s3.put_bucket_policy(B, allowing ses.amazonaws.com s3:PutObject on arn:aws:s3:::B/raw/*).
5. ses.create_receipt_rule_set(RuleSetName=RS); ses.create_receipt_rule(RuleSetName=RS, Rule={'Name':'to-s3','Enabled':True,'ScanEnabled':False,'Recipients':['rcpt@d1.example.net'],'Actions':[{'S3Action':{'BucketName':B,'ObjectKeyPrefix':'raw/'}}]}); ses.set_active_receipt_rule_set(RuleSetName=RS); wait ~20 s.
6. Build a multipart/mixed message with a text/plain part and an application/octet-stream attachment (e.g. bytes(range(256))*4) named 'probe.bin', Subject set to a unique token, From 'sender@d1.example.net', To 'rcpt@d1.example.net'; call ses.send_raw_email(Source='sender@d1.example.net', Destinations=['rcpt@d1.example.net'], RawMessage={'Data': msg.as_bytes()}) and note it returns a MessageId with no error.
7. Poll s3.list_objects_v2(Bucket=B, Prefix='raw/') — observe no key other than (at most) 'raw/AMAZON_SES_SETUP_NOTIFICATION' ever appears, so there is no stored raw message to read back.
8. Re-replay s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact.
