Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-images-amazon-linux-2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-images-amazon-linux-2 ec2-catalog-describe-images-amazon-linux-2023 ec2-catalog-describe-images-deep-learning ec2-catalog-describe-images-ecs-optimized ec2-catalog-describe-images-eks-optimized ec2-catalog-describe-images-windows-server-2025-base ec2-catalog-describe-images-windows-server-base ec2-catalog-get-image-ancestry
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeImages|2xx->4xx|$.DescribeImagesResponse
Issue-Key: ec2|DescribeImages|2xx->4xx|status
Issue-Key: ec2|GetImageAncestry|2xx->4xx|$.GetImageAncestryResponse
Issue-Key: ec2|GetImageAncestry|2xx->4xx|status
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# EC2 image-catalog reads answer HTTP 400 instead of serving Amazon-published image records (2 causes)

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Two image-catalog read paths refuse requests that AWS answers 200, so no AMI can be resolved from the emulator. 2 causes.

Cause 1 of 2: DescribeImages refuses Amazon-published AMI selections (fixture ec2-catalog-describe-images-amazon-linux-2, step task#1).
AWS: DescribeImages answers HTTP 200 with a DescribeImagesResponse whose imagesSet holds one item per matching image (Smithy: EC2 DescribeImages -> DescribeImagesResult.imagesSet; the operation models no errors at all, errors: []). For an owner/name-filter selection of Amazon-owned images AWS returns the matching public AMI records (imageId, imageLocation, imageState, imageOwnerId, imageOwnerAlias, architecture, rootDeviceType/rootDeviceName, blockDeviceMapping with the ebs sub-structure, virtualizationType, hypervisor, platformDetails, usageOperation, deprecationTime, publicSsmParameterName, ...). There is no error class for a filter selection that simply matches nothing: an empty selection is a 200 with an empty imagesSet.
Emulator: the request is refused with HTTP 400 and no DescribeImagesResponse element at all (replay: "status: expected 200, got 400", "$.DescribeImagesResponse: missing"). The cluster covers 33 recorded steps across five catalog fixtures (ec2-catalog-describe-images-amazon-linux-2, -amazon-linux-2023, -deep-learning, -ecs-optimized, -eks-optimized).
Why: every client that resolves an AMI by owner/name filter - the SDKs, the CLI, CDK's MachineImage lookup, Auto Scaling launch templates - receives a client error instead of a catalog page, so nothing downstream can select an image or launch an instance. A 4xx also tells the client the request itself was invalid, which it is not.
Fix: the DescribeImages handler and its request-filter parsing (the owner/filter/name combinations used here must select from the image catalog instead of being rejected), plus the image catalog that handler reads.
Traces: ec2-catalog-describe-images-amazon-linux-2#task#1 (representative; 33 affected steps in the cluster).

Cause 2 of 2: GetImageAncestry refuses every ancestry read (fixture ec2-catalog-get-image-ancestry, step task#1).
AWS: GetImageAncestry answers HTTP 200 with a GetImageAncestryResponse whose imageAncestryEntrySet holds one item per ancestry entry, each carrying imageId, creationDate and imageOwnerAlias (Smithy: GetImageAncestry input ImageId (required) -> GetImageAncestryResult.ImageAncestryEntries, xmlName imageAncestryEntrySet; the operation models no errors, errors: []).
Emulator: HTTP 400 with no GetImageAncestryResponse element (replay: "status: expected 200, got 400", "$.GetImageAncestryResponse: missing"), for all 11 recorded steps of the fixture.
Why: a client walking an image's ancestry cannot read it at all and is told its request was malformed.
Fix: the GetImageAncestry handler and the image-ancestry data it should project from the image catalog.
Traces: ec2-catalog-get-image-ancestry#task#1 (representative; 11 affected steps in the cluster).

Related open issues that do NOT cover these steps (the replay reports both clusters unowned): "EC2 DescribeImages Amazon-published AMI records omit gp3 EBS iops/throughput and deprecationTime and report publicSsmParameterName wrongly" (20261107T085711Z) and "cloudfront: EC2 DescribeImages reports no Amazon-published Amazon Linux 2 AMI..." (20261104T051153Z) are about member-level content of a 200 response, not about the read being refused with 400.

## Evidence
Cause 1 - fixture ec2-catalog-describe-images-amazon-linux-2 step task#1, operation DescribeImages:
  expected status 200, actual status 400
  $.DescribeImagesResponse: missing (expected a response with imagesSet.item carrying imageId/imageLocation/imageState/imageOwnerId/architecture/blockDeviceMapping/..., got None)
  cluster affected_count 33 (ec2-catalog-describe-images-amazon-linux-2 task#1..#6, -amazon-linux-2023 task#1..#6, -deep-learning task#1..#3, -ecs-optimized task#1..#6, -eks-optimized task#1..#4, ...)

Cause 2 - fixture ec2-catalog-get-image-ancestry step task#1, operation GetImageAncestry:
  expected status 200, actual status 400
  $.GetImageAncestryResponse: missing (expected imageAncestryEntrySet.item with creationDate/imageId/imageOwnerAlias, got None)
  cluster affected_count 11 (ec2-catalog-get-image-ancestry task#1..#11)

Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model): DescribeImages errors [] ; GetImageAncestry input {ImageId required, DryRun} -> output {ImageAncestryEntries xmlName imageAncestryEntrySet}, errors [].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture ec2-catalog-describe-images-amazon-linux-2 step task#1 - a DescribeImages call selecting Amazon-owned AMIs by owner/name filter. Observe HTTP 400 with no DescribeImagesResponse; AWS answers 200 with imagesSet.
3. Cause 2: replay fixture ec2-catalog-get-image-ancestry step task#1 - a GetImageAncestry call for a recorded imageId. Observe HTTP 400 with no GetImageAncestryResponse; AWS answers 200 with imageAncestryEntrySet.
