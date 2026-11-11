Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3.cfn-deploy-s3-lambda-notification
Issue-Key: events__lambda__s3.cfn-deploy-s3-lambda-notification|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# events__lambda__s3: a CloudFormation-deployed S3 bucket notification never invokes its Lambda target, so the ObjectCreated pipeline produces nothing

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: an AWS::S3::Bucket NotificationConfiguration deployed by CloudFormation (with the AWS::Lambda::Permission ordered via DependsOn) is accepted but never delivers an s3:ObjectCreated:* event to the function.
AWS: recorded behavior only for the end-to-end effect; the configuration itself is s3#PutBucketNotificationConfiguration, whose LambdaFunctionConfigurations bind an event list to a function ARN. Once the stack is CREATE_COMPLETE, a PutObject of key "incoming/report.txt" causes S3 to invoke the function asynchronously; the handler copies the object, so HeadObject on "processed/incoming/report.txt" succeeds and GetObject on it returns the original body `quarterly data`, within the check's 120 s budget.
Emulator: the copied object never appears — the probe polls HeadObject on processed/incoming/report.txt and nothing is ever created, and the check is cut off by the harness after 60.0 s. No notification is delivered, so the function is never invoked and no side effect lands.
Why: S3 event notifications are the backbone of the event-driven patterns this service covers. A configuration that is accepted and then never fires means every bucket-triggered workflow is silently dead while the control plane reports it as wired — a client cannot even detect the breakage from the API surface.
Fix: the S3 object-write path's notification dispatch (the component that matches a committed object against the bucket's LambdaFunctionConfigurations and issues the asynchronous Lambda invoke), plus the CloudFormation AWS::S3::Bucket NotificationConfiguration handler that installs it.
The open issue 20261029T182228Z-events-lambda-s3-eventbridge-and-s3-notifications-never.md describes the same class of breakage for other checks in this service but does not carry this check's replay key.
Traces: events__lambda__s3.cfn-deploy-s3-lambda-notification.

## Evidence
Expected: after the stack is CREATE_COMPLETE and PutObject(Bucket=<stack bucket>, Key="incoming/report.txt", Body=b"quarterly data"), HeadObject(Bucket, "processed/incoming/report.txt") succeeds and GetObject on that key returns b"quarterly data".
Actual: the processed/ key never comes into existence; the HeadObject probe returns None for the whole poll and the check ends with "Error: Check timed out after 60.0s" without the notification ever being delivered.

## Steps to Reproduce
Replay events__lambda__s3.cfn-deploy-s3-lambda-notification: CreateStack from deployments/s3-lambda-notification-cfn.yaml (bucket + function + AWS::Lambda::Permission + NotificationConfiguration, DependsOn-ordered), wait for CREATE_COMPLETE, read the BucketName output, PutObject key "incoming/report.txt", then poll HeadObject for "processed/incoming/report.txt".
