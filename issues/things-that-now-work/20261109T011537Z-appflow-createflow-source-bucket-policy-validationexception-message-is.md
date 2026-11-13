Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appflow.s3-to-s3-flow-data-transfer-requires-bucket-policy
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow.s3-to-s3-flow-data-transfer-requires-bucket-policy|run#25|AssertionError|assert message == expected_message, message
Issue-Commit: ec8f6ec2805a2454e78a14c856d8e041d8e7cb40

# AppFlow CreateFlow source-bucket-policy ValidationException message is emulator prose, not AWS's "Create Flow request failed: [...]" sentence

> _Note: this issue was identified against the code base at commit `ec8f6ec2805a2454e78a14c856d8e041d8e7cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlow

## Description
One net-new root cause in appflow.

Cause 1 of 1: CreateFlow rejects an S3 source bucket whose resource policy does not grant the AppFlow service principal with the right status (400) and the right error code (ValidationException), but with a message the emulator writes itself instead of AWS's message template for that error class.

AWS: Smithy: CreateFlow (POST /create-flow) takes a @required sourceFlowConfig and models ValidationException with httpError 400 ("The request has invalid or missing parameters."). The model does not specify the message text, so recorded behavior only for the wording: when the S3 bucket named in sourceFlowConfig.sourceConnectorProperties.S3.bucketName has no bucket policy granting the required s3 actions to the appflow.amazonaws.com service principal, AWS validates this at definition time and refuses CreateFlow with 400 / ValidationException whose message FIXED text is:
"Create Flow request failed: [The provided S3 Bucket [" + <bucket> + "] is missing the following permitted actions: [" + <missing actions> + "]\n. These policies must be applied to the [" + <service principal> + "] service principal and must be attached to all the following resources: [arn:aws:s3:::" + <bucket> + ", arn:aws:s3:::" + <bucket> + "/*]]".
Its VARIABLE parts are all taken from the request and from the bucket's policy state: the bucket name from the request's source connector properties, the lowercased list of the s3 actions still missing from that bucket's policy (for an S3 source read: "s3:listbucket, s3:getobject"), the IAM service principal that must be granted ("appflow.amazonaws.com" — an IAM principal, never an endpoint host), and the two resource ARNs derived from that bucket name. No literal bucket name, flow name or id from any single run is part of the template.

Emulator: same 400 / ValidationException, but the message is unrelated prose: "Amazon AppFlow is not authorized to access the Amazon S3 bucket <bucket>. Add a bucket policy that grants appflow.amazonaws.com the permissions the flow needs and try again." It omits the "Create Flow request failed: [...]" envelope, omits the enumerated missing s3 actions, and omits the required resource ARNs.

Why: a client that classifies this refusal by the AWS message — or that parses out which s3 actions and which resource ARNs the policy must carry in order to repair the bucket policy automatically — recognises nothing in the emulator's text and cannot derive the remediation. Because the status and the error code are correct, the divergence is silent: the call is refused for the right reason but is reported in a way no AWS-tracking consumer understands.

Fix: the appflow CreateFlow handler's source-bucket-policy validation branch, where the ValidationException message is constructed (services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/src/ops/create_flow.rs) — it must render the AWS template above, enumerating the missing actions and the derived bucket ARNs, rather than its own sentence.

Note on related filings: open issue 20261029T192701Z-appflow-error-message-text-diverges-from-aws-wording.md records the same class of fault (emulator prose instead of AWS message text) for CreateConnectorProfile's @pattern violation and for DescribeFlow's not-found sentence. It does not cover CreateFlow's bucket-policy ValidationException, which is a distinct message site in a distinct handler.

Traces: appflow.s3-to-s3-flow-data-transfer-requires-bucket-policy (the check stops at this assertion, so the later positive-restore legs of that check — re-granting the policy, re-creating the flow and moving the bytes — are not yet exercised and may hide further divergences behind this one).

## Evidence
Check appflow.s3-to-s3-flow-data-transfer-requires-bucket-policy, verdict mismatch, assertion at check_appflow_s3_to_s3_flow_data_transfer_requires_bucket_policy.py line 64 (`assert message == expected_message`).

Error code: expected "ValidationException", actual "ValidationException" (matches; the preceding code assertion passed).
HTTP status: 400 both sides.

Expected message (AWS, template with request/state values filled in):
"Create Flow request failed: [The provided S3 Bucket [<source-bucket>] is missing the following permitted actions: [s3:listbucket, s3:getobject]\n. These policies must be applied to the [appflow.amazonaws.com] service principal and must be attached to all the following resources: [arn:aws:s3:::<source-bucket>, arn:aws:s3:::<source-bucket>/*]]"

Actual message from the emulator:
"Amazon AppFlow is not authorized to access the Amazon S3 bucket <source-bucket>. Add a bucket policy that grants appflow.amazonaws.com the permissions the flow needs and try again."

Fixed text vs values: everything outside the angle brackets above is fixed text AWS always emits for this error class; <source-bucket> is the bucketName from the request's sourceFlowConfig.sourceConnectorProperties.S3, the action list is the set of s3 actions missing from that bucket's policy (lowercased), and the two ARNs are derived from the same bucket name. The generated bucket name in any one replay (e.g. a `sim2real-af-bsrc-*` name) is a per-run value, not part of the requirement.

Model basis: Smithy CreateFlow — sourceFlowConfig @required; errors include ValidationException (client, httpError 400, "The request has invalid or missing parameters."); http POST /create-flow, success code 200.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite); check appflow.s3-to-s3-flow-data-transfer-requires-bucket-policy.
2. Create two S3 buckets. On the destination bucket put a policy granting appflow.amazonaws.com s3:PutObject/s3:AbortMultipartUpload/s3:ListMultipartUploadParts/s3:ListBucketMultipartUploads/s3:GetBucketAcl/s3:PutObjectAcl on arn:aws:s3:::<dst> and arn:aws:s3:::<dst>/*. Leave the source bucket with NO policy granting appflow.amazonaws.com anything.
3. Call appflow CreateFlow with triggerConfig {triggerType: OnDemand}, sourceFlowConfig = S3 connector over the ungranted source bucket (bucketPrefix "input", s3InputFormatConfig CSV), destinationFlowConfigList = [S3 connector over the granted destination bucket], and passthrough Filter/Map tasks.
4. Observe the refusal: HTTP 400, Error.Code "ValidationException" (both correct), but Error.Message is "Amazon AppFlow is not authorized to access the Amazon S3 bucket <src>. Add a bucket policy that grants appflow.amazonaws.com the permissions the flow needs and try again." instead of AWS's "Create Flow request failed: [The provided S3 Bucket [<src>] is missing the following permitted actions: [s3:listbucket, s3:getobject]\n. These policies must be applied to the [appflow.amazonaws.com] service principal and must be attached to all the following resources: [arn:aws:s3:::<src>, arn:aws:s3:::<src>/*]]".
