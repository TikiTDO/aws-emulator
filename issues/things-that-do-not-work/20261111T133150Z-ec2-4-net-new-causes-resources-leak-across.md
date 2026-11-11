Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.key-pair-is-invisible-from-another-region ec2.ec2mr-byvpc-launch-with-another-regions-subnet-or-security-group-rejected ec2.ec2mr-pip-one-private-ip-in-two-regions-is-two-independent-interfaces ec2.import-key-pair-accepts-an-rsa-3072-key-and-reports-its-fingerprint ec2.vscan-public-exposure-derivation-selects-only-the-host-with-a-public-ip ec2.dpeer-overlapping-cidr-peering-fails-and-never-carries-traffic ec2.dpeer-route-is-blackhole-until-the-peering-is-accepted ec2.droute-deleted-peering-target-blackholes-the-route-and-drops-traffic
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.key-pair-is-invisible-from-another-region|_client_error#10|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. Result: {result!r}")
Issue-Key: ec2.ec2mr-byvpc-launch-with-another-regions-subnet-or-security-group-rejected|_launch_error#8|AssertionError|raise AssertionError(f"the launch succeeded with {ids!r}; real AWS rejects it")
Issue-Key: ec2|RunInstances|InvalidIPAddress.InUse
Issue-Key: ec2|ImportKeyPair|InvalidKeyPair.Format
Issue-Key: ec2.vscan-public-exposure-derivation-selects-only-the-host-with-a-public-ip|run#16|AssertionError|assert ip.count(".") == 3, f"the default-launched host must get a public IPv4, got {ip!r}"
Issue-Key: ec2.dpeer-overlapping-cidr-peering-fails-and-never-carries-traffic|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2.dpeer-route-is-blackhole-until-the-peering-is-accepted|_poll#6|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2.droute-deleted-peering-target-blackholes-the-route-and-drops-traffic|_poll#5|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Commit: ffc9519ba35ae474158cb1300960bfc0fdc8c7a4

# ec2: 4 net-new causes — resources leak across Regions, ImportKeyPair rejects an RSA-3072 OpenSSH key, a default launch gets no public IPv4, and guests never come Online in SSM

> _Note: this issue was identified against the code base at commit `ffc9519ba35ae474158cb1300960bfc0fdc8c7a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKeyPairs

## Description
Four net-new root causes in EC2's Region scoping, key-pair import, public addressing and guest reachability. All four must be closed before submitting. The EC2 Smithy model declares no modeled errors for these operations (errors: []), so each AWS requirement below is recorded behavior.

Cause 1 of 4: EC2 resources are visible and conflicting across Regions — one store is shared by every regional endpoint.
AWS: EC2 resources are Region-scoped. DescribeKeyPairs in another Region for a key created in the home Region is refused with code `InvalidKeyPair.NotFound` and the message "The key pair '<name>' does not exist" (fixed text plus the requested name). RunInstances in Region B naming Region A's subnet is refused `InvalidSubnetID.NotFound` with "The subnet ID '<subnet-id>' does not exist"; with its own subnet but Region A's security group it is refused `InvalidGroup.NotFound` with "The security group '<sg-id>' does not exist in VPC '<vpc-id>'"; neither launches anything. The same private IPv4 address is usable independently in two Regions, so a launch pinning it in Region B succeeds while Region A also holds it.
Emulator: DescribeKeyPairs in the other Region SUCCEEDS and returns the home Region's key pair; RunInstances in the peer Region with the home Region's subnet SUCCEEDS and launches an instance; pinning a private IPv4 already used in another Region is refused `InvalidIPAddress.InUse` when AWS accepts it.
Why: a client's multi-Region estate collapses into one — names and addresses collide across Regions, cross-Region misconfiguration is silently accepted instead of refused, and per-Region isolation tests cannot pass. Open issue 20261104T142958Z-ec2-regional-endpoints-share-one-store-cross-region.md describes a similar shared-store symptom but does not carry these checks.
Fix: the EC2 service state — key the resource store by Region and resolve every id (key pair, subnet, security group, private address) within the request's Region only.
Traces: ec2.key-pair-is-invisible-from-another-region, ec2.ec2mr-byvpc-launch-with-another-regions-subnet-or-security-group-rejected, ec2.ec2mr-pip-one-private-ip-in-two-regions-is-two-independent-interfaces.

Cause 2 of 4: ImportKeyPair refuses a valid 3072-bit RSA OpenSSH public key.
AWS: ImportKeyPair accepts an RSA public key in OpenSSH (`ssh-rsa AAAA...`) encoding — in the recording a 3072-bit one — storing it as KeyType `rsa` and reporting KeyFingerprint as the MD5 digest of the key's DER SubjectPublicKeyInfo, colon-separated; DescribeKeyPairs then reports the same KeyPairId, KeyType `rsa` and fingerprint.
Emulator: the call is refused with code `InvalidKeyPair.Format` and the message "Key is not in valid OpenSSH public key format", so the import never happens.
Why: a client importing its own RSA key cannot create a key pair at all, and the refusal blames the encoding when the material is well-formed OpenSSH.
Fix: ImportKeyPair's PublicKeyMaterial parsing — accept the OpenSSH `ssh-rsa` encoding at every supported modulus size and derive the MD5-of-SubjectPublicKeyInfo fingerprint.
Traces: ec2.import-key-pair-accepts-an-rsa-3072-key-and-reports-its-fingerprint.

Cause 3 of 4: an instance launched with the account's default networking receives no public IPv4 address.
AWS: an instance launched into a subnet that maps public IPs on launch (the default-VPC default) is assigned a public IPv4 address, and DescribeInstances reports it in PublicIpAddress.
Emulator: PublicIpAddress is the empty string.
Why: a client that derives public exposure, or that reaches an instance over its public address, sees every host as private; exposure analysis and any public-path test are wrong rather than merely incomplete.
Fix: the RunInstances network-interface path and the subnet's MapPublicIpOnLaunch handling — allocate and report a public IPv4 for a launch into a public-mapping subnet.
Traces: ec2.vscan-public-exposure-derivation-selects-only-the-host-with-a-public-ip.

Cause 4 of 4: a launched instance's SSM agent never reports Online, so no guest-level EC2 data-plane behavior can be observed.
AWS: an instance launched from an SSM-enabled AMI with an instance profile granting AmazonSSMManagedInstanceCore and a network path to the SSM endpoints (its own internet gateway, or the ssm/ssmmessages/ec2messages interface endpoints) registers and DescribeInstanceInformation reports PingStatus `Online`; RunCommand then executes in the guest.
Emulator: the agent never comes Online, and the peering/route data-plane checks never reach their first observation.
Why: every behavior that must be read from inside the guest — VPC peering actually carrying packets, a blackholed route dropping traffic, an overlapping-CIDR peering never carrying traffic — is unobservable, so a client cannot validate its network at all against this emulator.
Fix: the instance boot path and its SSM registration (the agent's reachability to the SSM endpoints and the instance-information state that DescribeInstanceInformation reads).
Traces: ec2.dpeer-route-is-blackhole-until-the-peering-is-accepted, ec2.droute-deleted-peering-target-blackholes-the-route-and-drops-traffic, ec2.dpeer-overlapping-cidr-peering-fails-and-never-carries-traffic.

## Evidence
Cause 1 — DescribeKeyPairs in another Region: expected ('InvalidKeyPair.NotFound', "The key pair '<name>' does not exist"); actual HTTP 200 — "the call succeeded; real AWS rejects it. Result: {'KeyPairs': [{'KeyPairId': 'key-00000000000000080', 'KeyType': 'ed25519', ... 'KeyName': 'sim2real-kpreg-71e2beec83ec', ...}]}". RunInstances with another Region's subnet: expected ('InvalidSubnetID.NotFound', "The subnet ID '<subnet-id>' does not exist") then ('InvalidGroup.NotFound', "The security group '<sg-id>' does not exist in VPC '<vpc-id>'"); actual "the launch succeeded with ['i-00000000000000002']; real AWS rejects it". Private IP in a second Region: actual "An error occurred (InvalidIPAddress.InUse) when calling the RunInstances operation: Address 10.73.1.10 is in use." where AWS launches successfully (the address is a per-run value).

Cause 2 — expected: ImportKeyPair succeeds, KeyType 'rsa', KeyFingerprint = colon-separated MD5 of the DER SubjectPublicKeyInfo. Actual: "An error occurred (InvalidKeyPair.Format) when calling the ImportKeyPair operation: Key is not in valid OpenSSH public key format" for a 3072-bit RSA key serialized with Encoding.OpenSSH / PublicFormat.OpenSSH.

Cause 3 — expected a dotted-quad PublicIpAddress on a default-networking launch. Actual: "the default-launched host must get a public IPv4, got ''".

Cause 4 — "TimeoutError: the SSM agent on side a (i-00000000000000001) coming Online over its own internet gateway igw-00000000000000097: the run budget ended first; not graded"; "TimeoutError: the SSM agent on the server i-00000000000000001 coming Online: the run budget ended first; not graded"; and for the overlapping-CIDR peering check "Error: Check timed out after 60.0s".

## Steps to Reproduce
Replay the behavioral suite and re-run the eight checks listed in trace_ids. Individually:
1. CreateKeyPair in Region A, then DescribeKeyPairs(KeyNames=[name]) against a Region-B client — expect InvalidKeyPair.NotFound. Create a VPC/subnet/security group in Region A and in Region B, then RunInstances against the Region-B endpoint with Region A's SubnetId, and again with Region B's subnet plus Region A's security group. Launch an instance pinning PrivateIpAddress 10.x.y.z in Region A, then launch with the same address in Region B.
2. Generate a 3072-bit RSA key, serialize the public half as OpenSSH, and call ImportKeyPair(KeyName=..., PublicKeyMaterial=<ssh-rsa ...>).
3. RunInstances into the default VPC's public subnet (MapPublicIpOnLaunch true) and read PublicIpAddress from DescribeInstances.
4. Launch an SSM-enabled instance with an AmazonSSMManagedInstanceCore instance profile and an internet gateway route, then poll DescribeInstanceInformation for PingStatus Online.
