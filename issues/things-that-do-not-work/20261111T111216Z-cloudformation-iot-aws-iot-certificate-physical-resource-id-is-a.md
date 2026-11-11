Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iot.cfn-iot-certificate-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__iot.cfn-iot-certificate-create-update-in-place-replace-retained-then-delete|_certificate_id#12|AssertionError|assert re.fullmatch(r"[0-9a-f]{64}", cert_id), (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# AWS::IoT::Certificate physical resource id is a generated stack name, not the 64-hex IoT certificate id

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResources

## Description
Cause 1 of 1: the certificate resource is tracked by a generated name instead of its IoT certificate id.

AWS: recorded behavior only. The physical resource id of AWS::IoT::Certificate is the IoT certificate id — a 64-character lowercase hexadecimal string (the SHA-256 of the certificate). That is what Fn::Ref resolves to and what IoT's DescribeCertificate, UpdateCertificate, AttachPolicy/AttachThingPrincipal and DeleteCertificate take as certificateId.

Emulator: the physical resource id is a CloudFormation-generated resource name, 'sim2real-cfnreal-iot-cert-7f36266287eb-Certificate-807E79750DD2', which does not match ^[0-9a-f]{64}$.

Why: the Ref value a template feeds to AWS::IoT::PolicyPrincipalAttachment / ThingPrincipalAttachment, and anything an operator passes to the IoT API, is not a certificate id AWS would accept — every downstream attachment or lookup is addressed to an id that identifies nothing.

Fix: the AWS::IoT::Certificate resource handler in the CloudFormation provisioner — return the IoT-minted certificateId (64 hex chars) as the physical resource id.

Traces: the failing check in trace_ids.

## Evidence
Expected: the certificate's PhysicalResourceId matches ^[0-9a-f]{64}$.
Actual: AssertionError: the PhysicalResourceId must be an IoT certificate id, got 'sim2real-cfnreal-iot-cert-7f36266287eb-Certificate-807E79750DD2'

## Steps to Reproduce
1. CreateStack a template with an AWS::IoT::Certificate (CertificatePem/Status ACTIVE); wait for CREATE_COMPLETE.
2. DescribeStackResources: the certificate's PhysicalResourceId is the generated stack resource name, not a 64-hex certificate id.
