Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-describe-images-self-owned-pagination-and-params
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-describe-images-self-owned-pagination-and-params
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-1"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-1"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-2"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-2"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-3"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-3"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-4"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-4"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-5"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-5"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-6"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-6"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-7"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-7"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-8"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-5cc3230f-8"].freeTierEligible
Issue-Commit: c3c8a5d5636f74138d44709d078c46bc0a5a7c2e

# ec2 DescribeImages drops blockDeviceMapping ebs.encrypted and reports freeTierEligible false for self-owned AMIs

> _Note: this issue was identified against the code base at commit `c3c8a5d5636f74138d44709d078c46bc0a5a7c2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Cause 1 of 1: DescribeImages answers 200 for self-owned AMIs, but every returned image row omits the EBS block-device mapping's encryption state and misreports the image's free-tier eligibility. This cluster is net-new — no open issue carries its replay key; the similar open issues 20261112T182924Z-ec2-describeimages-omits-ebs-encrypted-and-reports-freetiereligible.md and 20261112T211315Z-ec2-read-responses-carry-the-wrong-optional-members.md describe this family of fault but do not carry this fixture/step binding, so this replay step would not be re-run by a fix that closes them.

AWS: Smithy ec2#DescribeImages returns imagesSet of ec2#Image. Image's blockDeviceMapping entries target ec2#BlockDeviceMapping whose Ebs member is ec2#EbsBlockDevice, which models Encrypted (xmlName 'encrypted', ec2QueryName 'Encrypted', 'Indicates whether the encryption state of an EBS volume is changed while being restored from a backing snapshot'); AWS emits it on every EBS mapping of a described image, carrying false for an unencrypted snapshot rather than omitting it. Image also models FreeTierEligible (xmlName 'freeTierEligible'), which AWS states per image; the recorded AWS response reports it true for each of the self-owned images in this fixture.
Emulator: for every image in the page, blockDeviceMapping.item.ebs.encrypted is absent (expected 'false', got None) and freeTierEligible is 'false' where the recording has 'true'. The divergence repeats identically across all eight images the step returns, so it is a property of the Image projection, not of one record.
Why: a client that reads ebs.encrypted to decide whether a launched volume will be encrypted sees an unset optional and cannot distinguish 'unencrypted' from 'not reported'; a client that filters or reports on free-tier eligibility gets the opposite answer from AWS for its own images. Both make an AMI inventory built against the emulator disagree with the same inventory built against AWS.
Fix: the DescribeImages handler's Image projection/serializer — the EBS block-device mapping must emit the modeled encrypted member, and freeTierEligible must be derived from the image/instance-type eligibility rather than hard-coded false.
Traces: ec2-describe-images-self-owned-pagination-and-params#task#15 (the whole cluster; affected_count 1).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Cluster: fixture_id ec2-describe-images-self-owned-pagination-and-params, step_key task#15, service ec2, operation DescribeImages, verdict mismatch, expected_status 200, actual_status 200, stub_path empty, owned_by empty.
divergence_key: service=ec2|DescribeImages|status=200->200|keys=$.DescribeImagesResponse.imagesSet.item[name=...].blockDeviceMapping.item.ebs.encrypted,$.DescribeImagesResponse.imagesSet.item[name=...].freeTierEligible (repeated for images -1 through -8).
Recorded diffs (one pair per image, eight images in the page):
  $.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-<run-id>-7"].blockDeviceMapping.item.ebs.encrypted: missing (expected 'false', got None)
  $.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-<run-id>-7"].freeTierEligible: value_mismatch (expected 'true', got 'false')
(the image names are per-run probe names; the requirement is the member set and values, not the names)
Model grounding: Smithy ec2#EbsBlockDevice member Encrypted (xmlName 'encrypted') is modeled on the mapping DescribeImages returns, so its absence is a missing modeled member.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Fixture ec2-describe-images-self-owned-pagination-and-params, step task#15: DescribeImages over the self-owned AMIs created earlier in the fixture.
4. For each row of imagesSet: blockDeviceMapping.item.ebs.encrypted is missing (AWS sends 'false') and freeTierEligible is 'false' (AWS sends 'true').
