Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive
Issue-Key: cloudformation__ec2|DescribeVolumes|InvalidVolumeID.Malformed
Issue-Key: ssm|DescribeAssociation|ValidationException
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# CloudFormation: physical resource ids of CFN-created resources were synthesized logical names, not service-issued ids

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this report was filed as a bundle of five causes, "CloudFormation: stack Outputs carry no OutputValue, failing resources never roll the stack back, and physical resource ids are synthesized names". What is FIXED, and what this report now records, is cause 3 — its two `Issue-Key` lines and its two `Issue-Trace-Ids` are the ones kept above. The other four causes are unfixed and are re-filed under `issues/things-that-do-not-work/`, each carrying its own keys and trace ids so that no still-failing check is recorded as working:_
>
> * _`20261111T070000Z-cloudformation-three-stacks-report-an-output-with-no-outputvalue.md` (cause 1, three of its four checks; the fourth, `cloudformation__ec2__sqs__ssm.cfn-yaml-short-form-intrinsics-shape-resources`, passes now)_
> * _`20261111T070000Z-cloudformation-ec2-a-stack-whose-instance-names-a-nonexistent-ami-reaches-create-complete.md` (cause 2, two of its three checks; the third, `cloudformation__ec2.deploy-ec2-1-overlapping-subnet-cidr-rejected-and-rolled-back`, passes now)_
> * _`20261111T070000Z-ssm-a-securestring-is-not-readable-as-ciphertext-and-no-ec2-instance-is-a-managed-node.md` (cause 4)_
> * _`20261111T070000Z-ssm-getparameter-does-not-enforce-the-policy-that-restricts-a-scoped-read.md` (cause 5)_

## Operation
CreateStack, DeleteStack (physical-id assignment in the CloudFormation resource provisioners)

## Description
AWS: `Ref`/`Fn::GetAtt` on an `AWS::EC2::Volume` resolves to a `vol-<17 hex>` id and on an `AWS::SSM::Association` to the UUID `AssociationId`; those ids are accepted by the owning service.

Emulator: the volume resolved to `sim2real-xsvc-ec2vol-27af355a7e43-SnapVol-DE69CB08BE1B`, and EC2 `DescribeVolumes` refused it with `InvalidVolumeID.Malformed`; the SSM association resolved to the document name `AWS-RunShellScript`, and SSM `DescribeAssociation` refused it with `ValidationException` for failing the `associationId` UUID pattern.

Why: every stack-then-inspect flow broke — the id a stack handed back could not be used against the service that supposedly owned the resource.

Fixed: physical-id assignment for a type whose identifier the owning service MINTS now comes from that service's own create. `AWS::EC2::Volume` is created in Amazon EC2 and reports the `vol-` id `CreateVolume` answered; `AWS::SSM::Association` is created in AWS Systems Manager (`CreateAssociation`, with `UpdateAssociation` applying a changed declaration, `DeleteAssociation` releasing it with the stack and `DescribeAssociation` reading it back for drift) and reports the `AssociationId` that service minted as its physical resource id and as its `AssociationId` attribute.

Traces: cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain; cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive. Related open issues naming the same family for other resource types: 20261022T154846Z-ec2-emr-cloudformation-stack-outputs-return-synthesized-logical-names-instead.md, 20261022T175927Z-ds-ec2-e2e-cloudformation-reports-create-complete-but.md.

## Evidence
Before the fix: botocore `ClientError` (`InvalidVolumeID.Malformed`) from `DescribeVolumes` — "Invalid id: 'sim2real-xsvc-ec2vol-27af355a7e43-SnapVol-DE69CB08BE1B'"; botocore `ValidationException` from `DescribeAssociation` — "Value 'AWS-RunShellScript' at 'associationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-fA-F]{8}-...".

## Steps to Reproduce
`CreateStack` with an `AWS::EC2::Volume` and an `AWS::SSM::Association`, read each resource's `Ref`, and pass it to `ec2:DescribeVolumes` and `ssm:DescribeAssociation` respectively; then `DeleteStack` and confirm both are gone in their owning services.
