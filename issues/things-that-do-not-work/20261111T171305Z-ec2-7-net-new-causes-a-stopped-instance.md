Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.ec2mr-inv2-stopping-an-instance-releases-its-public-ip-and-drops-it-from-the-running-filter ec2.vscan-public-dns-name-appears-only-once-vpc-dns-hostnames-is-enabled ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not ec2.op-contract-get-declarative-policies-report-summary ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance ec2.ec2-security-group-ingress-decides-whether-the-instance-answers ec2.timesync-chrony-in-the-guest-locks-onto-the-link-local-amazon-time-sync-source ec2.timesync-guest-clock-agrees-with-the-aws-api-date-header-inside-two-seconds
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.ec2mr-inv2-stopping-an-instance-releases-its-public-ip-and-drops-it-from-the-running-filter|run#19|AssertionError|assert "PublicIpAddress" not in stopped, (
Issue-Key: ec2.vscan-public-dns-name-appears-only-once-vpc-dns-hostnames-is-enabled|run#12|AssertionError|assert before.get("PublicDnsName") == "", (
Issue-Key: ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not|_build#3|AssertionError|assert volume["Encrypted"] is True, (
Issue-Key: ec2.op-contract-get-declarative-policies-report-summary|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance|_poll_until#13|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: ec2.ec2-security-group-ingress-decides-whether-the-instance-answers|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: ec2.timesync-chrony-in-the-guest-locks-onto-the-link-local-amazon-time-sync-source|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: ec2.timesync-guest-clock-agrees-with-the-aws-api-date-header-inside-two-seconds|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# ec2: 7 net-new causes — a stopped instance keeps its mapped public IPv4, PublicDnsName ignores enableDnsHostnames, RunInstances drops Ebs.Encrypted, GetDeclarativePoliciesReportSummary's not-found text is re-authored, and three guest data-plane scenarios never reach a verdict

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances, RunInstances, GetDeclarativePoliciesReportSummary

## Description
Seven net-new root causes in the ec2 emulator, all grounded in failing behavioral checks of the `ec2` service. They are numbered below; a fixer must close ALL SEVEN before submitting.

=== Cause 1 of 7: DescribeInstances keeps reporting PublicIpAddress after the instance reaches `stopped` ===
AWS: an auto-assigned (mapped) public IPv4 address is released when an instance is stopped; only an Elastic IP survives a stop/start. Smithy: Instance$publicIpAddress is "The public IPv4 address, or the Carrier IP address assigned to the instance, if applicable" — an optional member, so when the address has been released the member is simply not serialized. AWS therefore omits the `publicIpAddress` element entirely for a stopped instance that holds no Elastic IP; it is not present-and-empty and not present-and-null.
Emulator: after StopInstances and after DescribeInstances reports `stopped`, the instance reservation still carries PublicIpAddress with the address it held while running (the replay shows an address from the emulator's mapped-public range retained verbatim).
Why: a client that reads Instance.PublicIpAddress to decide whether a host is still publicly reachable, or that diffs running vs stopped inventories to find addresses it must re-discover after a start, sees an address that no longer exists. Any "which instances are publicly addressable" or "reconnect to the new address after start" logic silently keeps the stale address.
Fix: the StopInstances state transition in the ec2 crate — releasing the instance's mapped public IPv4 (and the matching DescribeInstances serializer, which must omit the member rather than emit an empty/null one) when the instance is not holding an Elastic IP.
Traces: ec2.ec2mr-inv2-stopping-an-instance-releases-its-public-ip-and-drops-it-from-the-running-filter

=== Cause 2 of 7: DescribeInstances serves a PublicDnsName for an instance in a VPC whose enableDnsHostnames is false ===
AWS: Smithy, Instance$publicDnsName: "The public DNS name assigned to the instance. This name is not available until the instance enters the running state. This name is only available if you've enabled DNS hostnames for your VPC." With the VPC attribute enableDnsHostnames false, AWS reports PublicDnsName as the empty string (and no public hostname is resolvable).
Emulator: with DNS hostnames turned off on the instance's VPC, DescribeInstances still composes and serves a public hostname of the form `ec2-<dashed-public-ipv4>.<region>.compute.amazonaws.com` derived from the instance's public IPv4, instead of the empty string.
Why: the public DNS name is exactly what a client uses to decide whether a host is published in public DNS; a caller auditing public exposure, or one building a connection string from PublicDnsName, is told a name exists and resolves when the VPC has hostnames disabled. The whole point of ModifyVpcAttribute(EnableDnsHostnames) becomes unobservable.
Fix: the DescribeInstances read path / instance serializer in the ec2 crate — gate PublicDnsName on the owning VPC's enableDnsHostnames attribute (and on the instance being `running`) instead of deriving it unconditionally from the public IPv4.
Traces: ec2.vscan-public-dns-name-appears-only-once-vpc-dns-hostnames-is-enabled

=== Cause 3 of 7: RunInstances ignores BlockDeviceMappings[].Ebs.Encrypted — the created root volume is unencrypted ===
AWS: Smithy, EbsBlockDevice$encrypted: for a block device mapping for a new (empty) volume "you can include this parameter, and specify either true for an encrypted volume, or false for an unencrypted volume. If you omit this parameter, it defaults to false". A RunInstances launch whose root mapping sets Ebs.Encrypted=true produces an encrypted volume, and DescribeVolumes reports Encrypted=true (with KmsKeyId set to the EBS default key when none was named). This is also the precondition AWS enforces for hibernation: HibernationOptions.Configured=true requires an encrypted root volume.
Emulator: RunInstances accepts `BlockDeviceMappings=[{DeviceName: <root>, Ebs: {VolumeSize: N, VolumeType: 'gp3', Encrypted: true, DeleteOnTermination: true}}]` with 200, but the volume it creates reports `Encrypted: false` and carries no KmsKeyId — the requested encryption state is dropped on the floor. The replayed DescribeVolumes body shows the root volume of the launched instance with every other requested field honoured (VolumeType gp3, Size 8, the attachment at /dev/xvda) and only `Encrypted` wrong.
Why: a client that launches an encrypted-root instance and then verifies it (a compliance gate, or any hibernation-capable launch, which AWS refuses without an encrypted root) is told the volume is unencrypted; conversely nothing in the emulator can ever exercise the encrypted-root preconditions, so encryption-dependent behaviour is untestable and a stack that requires encryption-at-rest appears to be violating its own policy.
Fix: the RunInstances handler in the ec2 crate — carry EbsBlockDevice.Encrypted (and KmsKeyId) from the request's block device mappings into the created volume's state, so DescribeVolumes/DescribeInstances report it back.
Traces: ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not

=== Cause 4 of 7: GetDeclarativePoliciesReportSummary's not-found refusal carries emulator-authored message text ===
AWS: the operation is modelled with ReportId required (Smithy: GetDeclarativePoliciesReportSummary.ReportId -> DeclarativePoliciesReportId, required) and declares no operation-specific errors, so an unknown report id is refused through the EC2 Query common client-error channel. The recorded refusal message AWS serves is, verbatim as its fixed text: `The declarative-policies-report ID '<reportId>' does not exist` — the only variable part is the report id, taken from the request's ReportId; there is no trailing period.
Emulator: the same call is refused with the message `The report ID '<reportId>' does not exist.` — the fixed text is re-authored ("report ID" instead of "declarative-policies-report ID") and a trailing period is added.
Why: the message is part of the refusal contract. A client (or a human reading a log) that matches on AWS's wording to distinguish "no such declarative-policies report" from other id-not-found refusals in the same account does not match, and tooling that asserts on AWS text fails against the emulator.
Fix: the refusal/message construction in the GetDeclarativePoliciesReportSummary handler in the ec2 crate (the not-found branch), which must emit AWS's fixed text with the request's report id substituted. Note the only open issue naming this family, 20261023T102725Z-ec2-canceldeclarativepoliciesreport-stub.md, is about CancelDeclarativePoliciesReport being an unimplemented stub and does not cover this message.
Traces: ec2.op-contract-get-declarative-policies-report-summary

=== Cause 5 of 7: the Elastic IP data-plane scenario never reaches a verdict — the guest never reports its listener state inside the run budget ===
AWS: recorded behavior only. The scenario associates an Elastic IP with a running instance, re-associates it, and checks which address actually serves the instance's listener; it reads the guest's listener state out of the instance console output while polling. On AWS the guest boots, user data brings up the listener, and the console output carries the listener's state well inside the budget.
Emulator: the poll for the guest's listener state never satisfies its predicate and the run budget is exhausted first — the replay ends with `TimeoutError: the guest on <instance-id> reports its listener state: the run budget ended first; not graded`. The emulator answers the control-plane calls but never produces a guest whose console output reports the listener, so the Elastic-IP-moves-which-address-serves leg is never observable.
Why: a client cannot verify that associating or re-associating an Elastic IP changes which address serves the instance — the behaviour that Elastic IP exists for. Every data-plane consequence of AssociateAddress/DisassociateAddress is unobservable.
Fix: the instance data plane in the ec2 crate — the guest must boot, run its user data and publish console output, and the Elastic IP association must be reflected in which address reaches the guest. This is adjacent to, but not covered by, the open issues 20261107T124656Z-autoscaling-ec2-s3-an-asg-launched-instance-never-runs-its-userdata.md and 20261107T075601Z-ec2-s3-an-asg-launched-instance-never-boots-a-guest.md, which are filed under other service tokens and other scenarios.
Traces: ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance

=== Cause 6 of 7: the security-group ingress data-plane scenario never reaches a verdict — the check is killed at its 60 s budget ===
AWS: recorded behavior only. The scenario launches a t3.micro from the AL2023 AMI resolved through SSM, has user data bring up a listener on TCP 80, and then probes that port from outside with and without an authorizing ingress rule — a connection must complete only while a rule admits it. On AWS the whole scenario completes inside the check's budget.
Emulator: the check never completes; the replay ends with `Check timed out after 60.0s`, so neither the admitted leg nor the refused leg is ever graded.
Why: security-group enforcement is the single most relied-on EC2 network behaviour. A client cannot confirm that authorizing or revoking ingress changes whether the instance answers, so a scenario that depends on a group actually blocking traffic silently proves nothing.
Fix: the instance data plane and security-group enforcement in the ec2 crate — a launched guest must become reachable on its listening port, and ingress authorization must gate that reachability, within the time a client is willing to wait.
Traces: ec2.ec2-security-group-ingress-decides-whether-the-instance-answers

=== Cause 7 of 7: the Amazon Time Sync data-plane scenarios never reach a verdict — both timesync checks are killed at their 60 s budget ===
AWS: recorded behavior only, against documented AWS behaviour: the Amazon Time Sync Service is reachable from every instance at the link-local address 169.254.169.123, chrony in an AL2023 guest selects it as its source (chronyc reports the peer with refid A9FEA97B, the hex form of 169.254.169.123), and the guest's system clock tracks NTP time to well under a millisecond — so the guest clock agrees with the Date header of an AWS API response to within a couple of seconds.
Emulator: both checks end with `Check timed out after 60.0s`; the guest never reports a chrony source at 169.254.169.123 and the guest-clock-vs-Date-header comparison is never made. No verdict is reached on either.
Why: a client that relies on the link-local time source (certificate validity windows, SigV4 request signing, any clock-sensitive workload inside the guest) has no way to see that the emulator's guests get time at all, and a guest whose clock is wrong would skew SigV4 signing without the suite ever noticing.
Fix: the instance data plane in the ec2 crate — serve the Amazon Time Sync Service on the link-local address 169.254.169.123 to booted guests, and let the guest's chrony/clock state be read back inside the check's budget.
Traces: ec2.timesync-chrony-in-the-guest-locks-onto-the-link-local-amazon-time-sync-source, ec2.timesync-guest-clock-agrees-with-the-aws-api-date-header-inside-two-seconds

## Evidence
Cause 1 — expected: DescribeInstances omits the PublicIpAddress member for a stopped instance with no Elastic IP. actual: AssertionError: '<instance-id>' still reports PublicIpAddress '<mapped-address>' after reaching stopped. A mapped public IPv4 is released on stop — only an Elastic IP survives, and this instance has none — so the key must be ABSENT, not retained and not nulled.

Cause 2 — expected: with the VPC's enableDnsHostnames false, Instance.PublicDnsName == "". actual: AssertionError: with DNS hostnames off the host must have no public DNS name, got 'ec2-<dashed-public-ipv4>.<region>.compute.amazonaws.com'.

Cause 3 — expected: the root volume of a launch whose BlockDeviceMappings[].Ebs.Encrypted is true reports Encrypted: true. actual: AssertionError: hibernation requires an encrypted root volume; <volume-id> on <instance-id> reports a DescribeVolumes body with 'VolumeType': 'gp3', 'Size': 8, attachment at '/dev/xvda' and 'Encrypted': False.

Cause 4 — expected message (fixed text, report id from the request): "The declarative-policies-report ID '<reportId>' does not exist". actual message: "The report ID '<reportId>' does not exist." — AssertionError: expected message "The declarative-policies-report ID 'dpr-...' does not exist", got "The report ID 'dpr-...' does not exist."

Cause 5 — expected: the guest's console output reports its listener state so the Elastic-IP re-association leg can be graded. actual: TimeoutError: the guest on <instance-id> reports its listener state: the run budget ended first; not graded.

Cause 6 — expected: the scenario completes and grades both the admitted and the refused connection leg. actual: Check timed out after 60.0s.

Cause 7 — expected: chronyc in the guest reports the source 169.254.169.123 (refid A9FEA97B) and the guest clock agrees with the API Date header within 2 s. actual: Check timed out after 60.0s, for both timesync checks.

## Steps to Reproduce
Replay the behavioral suite against the emulator and re-run exactly these checks:
1. ec2.ec2mr-inv2-stopping-an-instance-releases-its-public-ip-and-drops-it-from-the-running-filter — RunInstances into a public subnet, wait for `running`, StopInstances, wait for `stopped`, then DescribeInstances and assert the PublicIpAddress key is absent.
2. ec2.vscan-public-dns-name-appears-only-once-vpc-dns-hostnames-is-enabled — create a VPC with enableDnsHostnames false, launch an instance with a public IPv4, DescribeInstances and assert PublicDnsName == ""; then ModifyVpcAttribute EnableDnsHostnames=true and assert the name appears.
3. ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not — RunInstances with HibernationOptions={'Configured': True} and BlockDeviceMappings=[{DeviceName: <root>, Ebs: {VolumeSize: 8, VolumeType: 'gp3', Encrypted: True, DeleteOnTermination: True}}], then DescribeVolumes on the root volume and assert Encrypted is True.
4. ec2.op-contract-get-declarative-policies-report-summary — call GetDeclarativePoliciesReportSummary with a well-formed but nonexistent ReportId and compare the refusal's Message to AWS's fixed text.
5. ec2.ec2-elastic-ip-association-moves-which-address-serves-the-instance — launch an instance whose user data brings up a listener, poll the console output for the listener state, then associate/re-associate an Elastic IP and probe which address serves it.
6. ec2.ec2-security-group-ingress-decides-whether-the-instance-answers — launch a t3.micro from the AL2023 SSM AMI with a listener on TCP 80 and probe the port with and without an authorizing ingress rule.
7. ec2.timesync-chrony-in-the-guest-locks-onto-the-link-local-amazon-time-sync-source and ec2.timesync-guest-clock-agrees-with-the-aws-api-date-header-inside-two-seconds — launch an AL2023 guest with the SSM core profile, read chronyc sources through Run Command and assert the source 169.254.169.123 / refid A9FEA97B, and compare the guest clock to an API response's Date header.
