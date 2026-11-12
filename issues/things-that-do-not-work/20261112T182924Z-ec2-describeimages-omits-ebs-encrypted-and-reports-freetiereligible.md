Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-describe-images-self-owned-pagination-and-params
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-describe-images-self-owned-pagination-and-params
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-1"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-1"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-2"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-2"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-3"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-3"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-4"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-4"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-5"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-5"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-6"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-6"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-7"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-7"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-8"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-dc0f1f4b-8"].freeTierEligible
Issue-Commit: 9f30cc5f50e7736550fc9d928f8a80b554132b0d

# ec2 DescribeImages omits ebs.encrypted and reports freeTierEligible false for self-owned images

> _Note: this issue was identified against the code base at commit `9f30cc5f50e7736550fc9d928f8a80b554132b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Cause 1 of 1: fixture ec2-describe-images-self-owned-pagination-and-params, step task#15 (DescribeImages over the account's own images).
AWS: each Image in the imagesSet carries blockDeviceMapping.item.ebs.encrypted (Smithy: EbsBlockDevice.Encrypted, xmlName 'encrypted' - 'If you omit this parameter, it defaults to false (unencrypted)') and freeTierEligible (Smithy: Image.FreeTierEligible, xmlName 'freeTierEligible'), whose value follows from the image's backing snapshot/volume and the image's eligibility - the recording reports encrypted 'false' and freeTierEligible 'true' for every one of the eight self-owned images in this page.
Emulator: answers 200 but omits blockDeviceMapping.item.ebs.encrypted entirely for every image, and reports freeTierEligible 'false' for all of them.
Why: a client that filters or budgets on free-tier eligibility sees no eligible image, and encryption-posture tooling that reads ebs.encrypted off DescribeImages gets no value at all and must treat the image as unknown; both decisions invert the real answer.
Fix: the DescribeImages image projection/serializer and the image record it reads (the ebs.encrypted member must be emitted, and freeTierEligible must be derived rather than hard-coded false).

## Evidence
ec2-describe-images-self-owned-pagination-and-params#task#15, DescribeImages, expected status 200 / actual status 200, verdict mismatch. For each of the eight images (names sim2real-probe-ec2-img-...-1 .. -8): $.DescribeImagesResponse.imagesSet.item[name="..."].blockDeviceMapping.item.ebs.encrypted expected 'false', got None (member absent); $.DescribeImagesResponse.imagesSet.item[name="..."].freeTierEligible expected 'true', got 'false'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay fixture ec2-describe-images-self-owned-pagination-and-params up to step task#15 (DescribeImages with Owner=self over the images the fixture registered).
3. Inspect every imagesSet item: blockDeviceMapping.item.ebs.encrypted is missing and freeTierEligible is 'false' where the recording has 'false' for encrypted and 'true' for freeTierEligible.
