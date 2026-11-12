Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-instance-reboot-and-status-report
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-instance-reboot-and-status-report ec2-run-instances-validation-and-diagnostic-interrupt ec2-instance-monitoring-toggle ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.groupSet
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.blockDeviceMapping
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.bootMode
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.dnsName
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.networkInterfaceSet.item.association.publicDnsName
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.privateDnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.bootMode
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.dnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.association.publicDnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.publicIpDnsNameOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.usageOperationUpdateTime
Issue-Related: 20261030T063325Z-ec2-instance-eni-and-volume-projections-omit-modeled.md
Issue-Related: 20261031T181451Z-ec2-instance-volume-snapshot-and-iam-profile-projections.md
Issue-Related: 20261103T071133Z-ec2-describeinstances-and-launch-template-projections-omit-modeled.md
Issue-Related: 20261111T180943Z-ec2-read-paths-serve-emulator-derived-catalog-data.md
Issue-Commit: 9f30cc5f50e7736550fc9d928f8a80b554132b0d

# ec2 instance projections ignore requested metadata options and omit modeled Instance members

> _Note: this issue was identified against the code base at commit `9f30cc5f50e7736550fc9d928f8a80b554132b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
The emulator's Instance projection (shared by RunInstances and DescribeInstances) does not echo the MetadataOptions the launch request supplied, omits several modeled Instance/InstanceNetworkInterface members, and reports attachment and group state that AWS does not report at that point in the lifecycle. Four net-new divergence clusters, one service, one root cause: the Instance -> wire projection. The similar open issue 20261103T071133Z-ec2-describeinstances-and-launch-template-projections-omit-modeled.md does NOT carry these replay keys (the replay reports them unowned), so these four steps stay unexercised until this issue is closed.

Cause 1 of 4: fixture ec2-instance-reboot-and-status-report, step setup#1 (RunInstances).
AWS: RunInstances returns the instance it just created with the metadata options as requested (Smithy: Instance.metadataOptions -> InstanceMetadataOptionsResponse with members httpTokens, httpPutResponseHopLimit, httpEndpoint, httpProtocolIpv6, instanceMetadataTags) and with Instance.bootMode and Instance.privateDnsName populated; the network interface attachment is still 'attaching' (Smithy: InstanceNetworkInterfaceAttachment.status -> AttachmentStatus) and the reservation-level groupSet and the instance blockDeviceMapping are empty in the RunInstances response. Recorded behavior confirms all of these for the launch in this fixture.
Emulator: metadataOptions.httpTokens comes back 'optional' though the request asked for 'required', httpPutResponseHopLimit comes back '1' though the request asked for 2, metadataOptions.httpProtocolIpv4 ('enabled') is absent (recorded behavior only - the member is not in the on-disk model), bootMode ('uefi-preferred') and privateDnsName are absent, networkInterfaceSet.item.attachment.status is already 'attached', and groupSet and blockDeviceMapping are populated where the recording has them empty.
Why: a client that launches with IMDSv2 required reads back 'optional' and concludes that hardening failed (or silently keeps using IMDSv1); boot-mode- and DNS-dependent provisioning code sees absent members; a waiter that watches attachment.status never observes the 'attaching' -> 'attached' transition.
Fix: the RunInstances handler's instance-creation path (it must persist the requested MetadataOptions instead of defaults) and the Instance serializer that projects metadataOptions, bootMode, privateDnsName, groupSet, blockDeviceMapping and networkInterfaceSet.attachment.

Cause 2 of 4: fixture ec2-run-instances-validation-and-diagnostic-interrupt, step task#7 (RunInstances). Same projection fault on a second launch shape: metadataOptions.httpTokens 'optional' instead of 'required', httpPutResponseHopLimit '1' instead of '2', httpProtocolIpv4 and bootMode absent, privateDnsName absent (recording has ip-10-63-0-142.us-west-2.compute.internal, i.e. the name derived from the allocated private address), attachment.status 'attached' instead of 'attaching', groupSet and blockDeviceMapping populated instead of empty.
Fix: as Cause 1.

Cause 3 of 4: fixture ec2-instance-monitoring-toggle, step task#3 (DescribeInstances; the cluster also covers ec2-instance-monitoring-toggle#task#5 and ec2-instance-reboot-and-status-report#task#2/#3/#4).
AWS: DescribeInstances projects, per instance, publicIpDnsNameOptions (dnsHostnameType plus publicIpv4DnsName), networkInterfaceSet.item.publicDnsName, networkInterfaceSet.item.publicIpDnsNameOptions, networkInterfaceSet.item.privateIpAddressesSet.item.association, metadataOptions as applied, bootMode and usageOperationUpdateTime, and the privateDnsName of each instance/interface/private address is the resource name derived from that instance's own private IPv4 address.
Emulator: publicIpDnsNameOptions, networkInterfaceSet.item.publicDnsName, networkInterfaceSet.item.publicIpDnsNameOptions, the per-private-address association block, metadataOptions.httpProtocolIpv4 and bootMode are all absent; metadataOptions.httpTokens/httpPutResponseHopLimit again report defaults instead of the applied values; privateDnsName is served from a fixed 172.31.0.0/16 name rather than the address the instance actually holds.
Why: DNS-driven clients cannot resolve an instance they just launched, because the only hostname members AWS gives them are missing; IMDS hardening again reads back wrong.
Fix: the DescribeInstances projection and the instance DNS-name derivation in state.

Cause 4 of 4: fixture ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip, step task#13 (DescribeInstances).
AWS: the reservation-level groupSet is empty for VPC instances (security groups are reported per network interface), instancesSet.item.clientToken is present (empty when none was supplied), each private address carries an association block with ipOwnerId 'amazon', and currentInstanceBootMode reflects the instance's boot mode ('uefi').
Emulator: the reservation groupSet carries the security group, clientToken is absent, dnsName and networkInterfaceSet.item.association.publicDnsName carry a generated name where the recording has them empty, privateIpAddressesSet.item.association is absent, metadataOptions.httpProtocolIpv4 is absent and currentInstanceBootMode is 'legacy-bios' instead of 'uefi'.
Why: tooling that reads security groups from the reservation (or boot mode from the instance) sees a shape AWS never emits and mis-classifies the instance.
Fix: the DescribeInstances projection (reservation groupSet placement, clientToken, per-address association, currentInstanceBootMode) in the instance read path.

## Evidence
Cause 1 - ec2-instance-reboot-and-status-report#setup#1, RunInstances, expected 200 / actual 200: $.RunInstancesResponse.instancesSet.item.metadataOptions.httpTokens expected 'required', got 'optional'; .metadataOptions.httpPutResponseHopLimit expected '2', got '1'; .metadataOptions.httpProtocolIpv4 expected 'enabled', got None; .bootMode expected 'uefi-preferred', got None; .privateDnsName expected present, got None; .networkInterfaceSet.item.attachment.status expected 'attaching', got 'attached'; $.RunInstancesResponse.groupSet expected '' (empty), got {'item': {'groupId': 'sg-...', 'groupName': 'default'}}; .blockDeviceMapping expected '' (empty), got a populated item.
Cause 2 - ec2-run-instances-validation-and-diagnostic-interrupt#task#7, RunInstances, 200/200: same member set, plus .privateDnsName expected the name derived from the instance's private IPv4 address, got None.
Cause 3 - ec2-instance-monitoring-toggle#task#3, DescribeInstances, 200/200: .publicIpDnsNameOptions expected {'dnsHostnameType': 'public-ipv4-dns-name', 'publicIpv4DnsName': <present>}, got None; .networkInterfaceSet.item.publicDnsName expected present, got None; .networkInterfaceSet.item.publicIpDnsNameOptions expected present, got None; .networkInterfaceSet.item.privateIpAddressesSet.item.association expected {'publicIp': <present>, 'publicDnsName': <present>, 'ipOwnerId': 'amazon'}, got None; .privateDnsName expected the instance's own derived name, got 'ip-172-31-0-4.us-west-2.compute.internal'; .metadataOptions.httpTokens expected 'required', got 'optional'; .metadataOptions.httpPutResponseHopLimit expected '2', got '1'.
Cause 4 - ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip#task#13, DescribeInstances, 200/200: $.DescribeInstancesResponse.reservationSet.item.groupSet expected '' (empty), got {'item': {'groupId': 'sg-...', 'groupName': '...'}}; .instancesSet.item.clientToken expected '' , got None; .instancesSet.item.networkInterfaceSet.item.privateIpAddressesSet.item.association expected {'publicIp': <present>, 'publicDnsName': '', 'ipOwnerId': 'amazon'}, got None; .instancesSet.item.metadataOptions.httpProtocolIpv4 expected 'enabled', got None; .instancesSet.item.currentInstanceBootMode expected 'uefi', got 'legacy-bios'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus simparity-wire-aws-v1.
2. Replay ec2-instance-reboot-and-status-report step setup#1 and ec2-run-instances-validation-and-diagnostic-interrupt step task#7: RunInstances with MetadataOptions.HttpTokens=required and HttpPutResponseHopLimit=2; compare metadataOptions, bootMode, privateDnsName, groupSet, blockDeviceMapping and networkInterfaceSet.item.attachment.status with the recording.
3. Replay ec2-instance-monitoring-toggle step task#3 and ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip step task#13: DescribeInstances on the launched instance; compare publicIpDnsNameOptions, networkInterfaceSet.item.publicDnsName/publicIpDnsNameOptions, privateIpAddressesSet.item.association, clientToken, reservation-level groupSet, metadataOptions and currentInstanceBootMode with the recording.
