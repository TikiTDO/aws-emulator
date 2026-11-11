Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap
Issue-Step: task#21
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2-instance-connect|SendSSHPublicKey|4xx->4xx|$.Message
Issue-Commit: 69c900cbbb228bcd9ce31c977c27cf9cdda27129

# ec2-instance-connect SendSSHPublicKey refuses an unknown Availability Zone with emulator-authored wording

> _Note: this issue was identified against the code base at commit `69c900cbbb228bcd9ce31c977c27cf9cdda27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendSSHPublicKey

## Description
ONE cause (1 of 1).

AWS: recorded behavior only (the ec2-instance-connect model types the refusal but fixes no text). When SendSSHPublicKey is called with an AvailabilityZone value that is not a zone of the Region, AWS refuses with HTTP 400 and the message 'Unrecognized zone <AvailabilityZone>' — fixed text 'Unrecognized zone ' plus the AvailabilityZone value taken verbatim from the request. The refusal is about the zone name itself, so it mentions neither the instance nor the instance's own zone.
Emulator: it answers HTTP 400 with the right error class but writes its own message, 'AvailabilityZone us-west-2z is not the Availability Zone of instance i-00000000000000001, which was launched in us-west-2a' — different fixed text, and it interpolates the instance id and the instance's launch zone, i.e. it reports a zone-mismatch-against-the-instance rather than an unrecognized zone.
Why: a client that matches or surfaces the refusal text (retry logic, user-facing diagnostics, tests pinned to AWS wording) sees a message AWS never sends, and the diagnosis it presents is wrong: the zone does not exist at all, rather than belonging to another instance.
Fix: the SendSSHPublicKey validation in the ec2-instance-connect service — validate the AvailabilityZone against the Region's zone list first and emit AWS's 'Unrecognized zone <value>' text; only compare against the instance's zone once the zone is known.
Traces: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#task#21 (representative) and #task#22 in the same cluster.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap step task#21 (cluster also covers task#22), operation SendSSHPublicKey, expected HTTP 400 / actual HTTP 400:
$.Message: value_mismatch — expected 'Unrecognized zone us-west-2z', got 'AvailabilityZone us-west-2z is not the Availability Zone of instance i-00000000000000001, which was launched in us-west-2a'.
Fixed text that must match: 'Unrecognized zone '. Variable part: the AvailabilityZone value from the request ('us-west-2z' in this recording). The instance id and launch zone the emulator adds are not part of AWS's message.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) and inspect fixture ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap, step task#21 (and task#22): call ec2-instance-connect SendSSHPublicKey for a running instance with InstanceId=<id>, AvailabilityZone=us-west-2z (a zone that does not exist in the Region) and a valid public key, and compare the 400 response's Message with 'Unrecognized zone us-west-2z'.
