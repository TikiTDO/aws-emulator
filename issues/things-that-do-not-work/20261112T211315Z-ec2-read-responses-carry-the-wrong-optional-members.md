Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-describe-images-self-owned-pagination-and-params
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-describe-images-self-owned-pagination-and-params ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-1"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-1"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-2"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-2"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-3"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-3"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-4"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-4"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-5"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-5"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-6"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-6"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-7"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-7"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-8"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-8986edde-8"].freeTierEligible
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.privateIpAddressesSet.item.association.publicDnsName
Issue-Related: 20261112T182924Z-ec2-instance-projections-ignore-requested-metadata-options-and.md
Issue-Commit: 75cd4519d4d8ae943d7fac1c316cbee2ee5c3551

# ec2 read responses carry the wrong optional members: DescribeImages drops ebs.encrypted and freeTierEligible, DescribeInstances adds a per-address publicDnsName

> _Note: this issue was identified against the code base at commit `75cd4519d4d8ae943d7fac1c316cbee2ee5c3551`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Two ec2 read operations answer HTTP 200 with the right top-level shape but the wrong set of optional members: one omits members AWS always emits, the other emits a member AWS leaves out. Both are serializer/record-construction faults on the read path rather than state or status faults.

Cause 1 of 2: DescribeImages omits blockDeviceMapping.item.ebs.encrypted and reports freeTierEligible false for self-owned images (fixture ec2-describe-images-self-owned-pagination-and-params, step task#15).
AWS: Smithy com.amazonaws.ec2#EbsBlockDevice models member Encrypted (xmlName 'encrypted', boolean) - 'Indicates whether the encryption state of an EBS volume is changed while being restored from a backing snapshot'; com.amazonaws.ec2#Image models FreeTierEligible - 'Indicates whether the image is eligible for Amazon Web Services Free Tier. If true, the AMI is eligible for Free Tier and can be used to launch instances under the Free Tier limits.' For every image in the recorded DescribeImages response AWS emits blockDeviceMapping.item.ebs.encrypted ('false' for an unencrypted snapshot-backed mapping, i.e. the member is present with value false rather than elided) and freeTierEligible 'true' for these images.
Emulator: blockDeviceMapping.item.ebs.encrypted is absent from every imagesSet item, and freeTierEligible is serialized as 'false' where AWS reports 'true'. This holds uniformly across all eight images the fixture registered, so it is the image record the handler builds, not one row.
Why: a client that reads ebs.encrypted to decide whether a copy needs a KMS key sees an absent member and cannot tell 'unencrypted' from 'not reported'; a client that filters the catalog on freeTierEligible drops every image that is in fact Free-Tier launchable, so a Free-Tier-constrained launch path finds no AMI at all.
Fix: the DescribeImages handler/serializer in the ec2 service crate - the Image record it projects from stored AMI state must always emit the ebs.encrypted member of each block-device mapping and must derive freeTierEligible from the image rather than hard-coding false. Open issue 20261112T182924Z-ec2-describeimages-omits-ebs-encrypted-and-reports-freetiereligible.md describes the same fault but does not carry this cluster's replay key (the replay reports owned_by empty for it), so this cluster is filed here and that issue should be closed together with it.
Traces: ec2-describe-images-self-owned-pagination-and-params#task#15.

Cause 2 of 2: DescribeInstances emits association.publicDnsName inside privateIpAddressesSet where AWS emits none (fixture ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap, step task#24).
AWS: Smithy com.amazonaws.ec2#InstanceNetworkInterfaceAssociation models publicDnsName (xmlName 'publicDnsName'), publicIp, ipOwnerId, carrierIp and customerOwnedIp, and the same association structure is reachable both at networkInterfaceSet.item.association and at networkInterfaceSet.item.privateIpAddressesSet.item.association. Recorded behavior only for which of the two carries the name: in the recording for an instance whose Elastic IP has just been swapped, the per-address association under privateIpAddressesSet.item carries NO publicDnsName member, while the interface-level association and the instance-level dnsName both carry the public DNS name derived from the currently associated public IPv4 address.
Emulator: it emits networkInterfaceSet.item.privateIpAddressesSet.item.association.publicDnsName as well, so the response carries a member AWS does not send at that path.
Why: a client walking privateIpAddressesSet to find which secondary address is externally reachable treats the presence of publicDnsName as the signal that the address is associated; the emulator makes that signal always true, so address-level reachability logic cannot distinguish the associated address from the others.
Fix: the DescribeInstances serializer in the ec2 service crate - the per-private-address association projection must not populate publicDnsName (the interface-level association and the instance dnsName keep it).
NOT part of this finding: the same cluster also reports the interface-level association.publicDnsName and the instance dnsName differing in their host label ('ec2-44-235-109-6.us-west-2.compute.amazonaws.com' recorded vs 'ec2-54-0-0-3.us-west-2.compute.amazonaws.com' served). That label is derived from whichever public IPv4 address the environment allocated for the run, which differs in every environment, so it is not a requirement and must not be asserted; only the extra member above is the defect.
Traces: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#task#24.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 - ec2-describe-images-self-owned-pagination-and-params#task#15, operation DescribeImages, service ec2, verdict mismatch, expected_status 200 / actual_status 200 (divergence_key service=ec2|DescribeImages|status=200->200). Reported diffs, repeated for each of the eight images in imagesSet:
  $.DescribeImagesResponse.imagesSet.item[name="<image name>"].blockDeviceMapping.item.ebs.encrypted: missing (expected 'false', got None)
  $.DescribeImagesResponse.imagesSet.item[name="<image name>"].freeTierEligible: value_mismatch (expected 'true', got 'false')
The divergence_key enumerates both keys for every imagesSet item in the page, i.e. the emulator never emits ebs.encrypted and never reports freeTierEligible true.

Cause 2 - ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#task#24, operation DescribeInstances, service ec2, verdict mismatch, expected_status 200 / actual_status 200 (divergence_key service=ec2|DescribeInstances|status=200->200). Reported diffs:
  $.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.privateIpAddressesSet.item.association.publicDnsName: expected None, got a public DNS name  <-- the member AWS does not emit at this path; this is the defect
  $.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.association.publicDnsName: differs only in the host label derived from the run's allocated public IPv4 address  <-- environment-specific, not asserted
  $.DescribeInstancesResponse.reservationSet.item.instancesSet.item.dnsName: same environment-specific host label  <-- not asserted

Model evidence (repo on-disk Smithy, services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): com.amazonaws.ec2#EbsBlockDevice member Encrypted (xmlName 'encrypted', boolean); com.amazonaws.ec2#Image member FreeTierEligible, documented 'Indicates whether the image is eligible for Amazon Web Services Free Tier'; com.amazonaws.ec2#InstanceNetworkInterfaceAssociation members CarrierIp, CustomerOwnedIp, IpOwnerId, PublicDnsName, PublicIp.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from this checkout.
2. Replay wire fixture ec2-describe-images-self-owned-pagination-and-params up to step task#15 (DescribeImages over the self-owned AMI catalog the fixture registered). Inspect each DescribeImagesResponse.imagesSet.item: the emulator emits no blockDeviceMapping.item.ebs.encrypted member and freeTierEligible 'false', where the recording has ebs.encrypted 'false' present and freeTierEligible 'true'.
3. Replay wire fixture ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap up to step task#24 (DescribeInstances after the Elastic IP swap). Inspect reservationSet.item.instancesSet.item.networkInterfaceSet.item.privateIpAddressesSet.item.association: the emulator includes a publicDnsName member there, the recording has none. Ignore the host label itself in the interface-level association and in dnsName - it tracks the public IPv4 address the environment allocated.
4. Both steps report HTTP 200 on each side with a body mismatch, so re-replay the two fixtures after the fix to confirm both clusters clear.
