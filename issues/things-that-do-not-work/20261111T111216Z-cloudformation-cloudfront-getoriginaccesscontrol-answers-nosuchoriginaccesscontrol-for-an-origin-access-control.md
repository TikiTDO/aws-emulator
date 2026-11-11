Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront.cfn-cloudfront-origin-access-control-create-update-in-place-stale-etag-refused-then-delete
Issue-Key: cloudfront|GetOriginAccessControl|NoSuchOriginAccessControl
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# GetOriginAccessControl answers NoSuchOriginAccessControl for an origin access control CloudFormation reported as created

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOriginAccessControl

## Description
Cause 1 of 1: a stack-created AWS::CloudFront::OriginAccessControl is not readable through CloudFront.

AWS: after a stack containing AWS::CloudFront::OriginAccessControl reaches CREATE_COMPLETE, CloudFront GetOriginAccessControl with the resource's physical id (the OAC Id) returns HTTP 200 with the OriginAccessControlConfig and an ETag; the ETag is what a subsequent UpdateOriginAccessControl must present, and a stale ETag is refused with PreconditionFailed.

Emulator: GetOriginAccessControl refuses the id CloudFormation just handed back — 'An error occurred (NoSuchOriginAccessControl) when calling the GetOriginAccessControl operation: The specified origin access control does not exist.' The error code is NoSuchOriginAccessControl (HTTP 404) and the message is CloudFront's fixed text for a genuinely absent OAC. The check cannot get an ETag at all, so the stale-ETag refusal it exists to prove is never reached.

Why: a client that provisions an OAC with CloudFormation and then reads or updates it through CloudFront (the only way to attach it to a distribution origin, or to rotate its config with If-Match) is told the control does not exist. Either CloudFormation reported CREATE_COMPLETE for something it never created, or it returned a physical id CloudFront does not recognise.

Fix: the AWS::CloudFront::OriginAccessControl resource handler (it must create the OAC in CloudFront state and return the OAC Id as the physical resource id) and/or CloudFront's origin-access-control state behind GetOriginAccessControl.

Traces: the failing check in trace_ids.

## Evidence
Expected: GetOriginAccessControl(Id=<physical id from the stack>) returns 200 with an ETag.
Actual: botocore.errorfactory.NoSuchOriginAccessControl: An error occurred (NoSuchOriginAccessControl) when calling the GetOriginAccessControl operation: The specified origin access control does not exist.

## Steps to Reproduce
1. CreateStack a template with an AWS::CloudFront::OriginAccessControl; wait for CREATE_COMPLETE.
2. Read the resource's PhysicalResourceId from DescribeStackResources.
3. Call cloudfront GetOriginAccessControl with that Id — it answers 404 NoSuchOriginAccessControl.
