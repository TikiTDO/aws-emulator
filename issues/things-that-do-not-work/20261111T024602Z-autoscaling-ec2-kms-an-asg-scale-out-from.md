Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__kms.asglf-default-version-literal-rebinds-when-the-default-moves
Issue-Key: autoscaling__ec2__kms.asglf-default-version-literal-rebinds-when-the-default-moves|run#15|AssertionError|assert act["StatusCode"] == "Cancelled", (
Issue-Commit: 6a777fbd6800e714176d50f5d7d416617b079449

# autoscaling__ec2__kms: an ASG scale-out from a launch-template version whose root volume names a disabled CMK is recorded Successful instead of Cancelled

> _Note: this issue was identified against the code base at commit `6a777fbd6800e714176d50f5d7d416617b079449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
Cause 1 of 1: a scaling activity whose underlying RunInstances cannot succeed (the launch template's root-volume BlockDeviceMapping names a KMS CMK that is Disabled) is reported as a completed, successful launch.

AWS: Auto Scaling resolves a group's launch-template Version literal "$Default" at LAUNCH time, then calls EC2 RunInstances. When the resolved version's Ebs.KmsKeyId names a key whose KeyState is Disabled, EC2 refuses the volume creation with a client KMS error, so Auto Scaling records the activity terminally as StatusCode "Cancelled" (Smithy: Activity.StatusCode targets the ScalingActivityStatusCode enum, which includes Cancelled; the StatusMessage carries EC2's client KMS reason) and no instance ever reaches InService. Nothing in the model forbids the launch, so the state coupling itself is recorded behavior: the check's recording has the first SetDesiredCapacity(1) activity end Cancelled while $Default is version 1, and the same group launch an InService instance recording LaunchTemplate.Version "2" after ModifyLaunchTemplate moves $Default to version 2 (version 2 carries no customer key).

Emulator: the first terminal activity for the group is StatusCode 'Successful' — the launch is accepted and recorded as having succeeded even though the resolved template version's CMK is Disabled. The emulator therefore never produces a Cancelled activity for a KMS-blocked launch.

Why: a client that waits on scaling-activity status to learn whether a capacity change actually landed is told the fleet grew when it did not. Any remediation path keyed on Cancelled + the KMS status message (re-enable the key, move $Default) is never exercised, and capacity reported as added is phantom.

Fix: the Auto Scaling launch path — the component that resolves the group's launch-template version reference and performs/records the EC2 launch, and the scaling-activity recorder that assigns StatusCode/StatusMessage. The EC2 side must refuse the volume under a non-Enabled CMK so the activity can be recorded Cancelled. Related but distinct open issues: 20261110T080328Z-ec2-kms-runinstances-ignores-the-kms-key-state-of-the.md (ec2 RunInstances ignores key state) and 20261107T075601Z-autoscaling-ec2-kms-auto-scaling-group-reads-omit-servicelinkedrolearn-and-group.md — neither carries this check's replay key.

Traces: autoscaling__ec2__kms.asglf-default-version-literal-rebinds-when-the-default-moves.

## Evidence
Check autoscaling__ec2__kms.asglf-default-version-literal-rebinds-when-the-default-moves, at the first SetDesiredCapacity(AutoScalingGroupName=<group>, DesiredCapacity=1) after the group is created on {LaunchTemplateId: <lt>, Version: "$Default"} with $Default = version 1 (BlockDeviceMappings[0].Ebs.KmsKeyId = a CMK that DescribeKey reports KeyState=Disabled):

Expected (AWS): the first terminal scaling activity has StatusCode == "Cancelled" (StatusMessage carries EC2's client KMS reason); no instance reaches InService.
Actual (emulator): AssertionError: while $Default is version 1 the launch must end Cancelled, got 'Successful'

The key's state is established in the same run: kms.disable_key(KeyId=<key>) followed by kms.describe_key(...)['KeyMetadata']['KeyState'] == 'Disabled' is asserted before the group is created, so the disabled state is observable to the emulator at launch time.

## Steps to Reproduce
1. CreateKey (symmetric CMK) and DisableKey it; confirm DescribeKey reports KeyState=Disabled.
2. CreateLaunchTemplate whose LaunchTemplateData has BlockDeviceMappings[0] = {DeviceName: /dev/xvda, Ebs: {VolumeSize: 8, VolumeType: gp3, Encrypted: true, KmsKeyId: <disabled CMK ARN>}}; this is version 1 and the default.
3. CreateLaunchTemplateVersion from SourceVersion=1 with the same mapping but NO KmsKeyId (version 2).
4. CreateAutoScalingGroup with LaunchTemplate={LaunchTemplateId: <lt>, Version: "$Default"}, MinSize 0 / MaxSize 1 / DesiredCapacity 0 in a private subnet.
5. SetDesiredCapacity(DesiredCapacity=1) and poll DescribeScalingActivities for the first terminal activity (Successful/Cancelled/Failed).
   -> AWS: StatusCode "Cancelled". Emulator: StatusCode "Successful".
6. ModifyLaunchTemplate(DefaultVersion="2"), drain, SetDesiredCapacity(1) again: AWS now reports an InService instance whose LaunchTemplate.Version is "2" (the group's stored Version literal stays "$Default").
