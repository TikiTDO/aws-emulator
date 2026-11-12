Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devicefarm.project-vpc-config-validates-against-real-ec2-subnets
Issue-Shape-Id: com.amazonaws.devicefarm#DeviceFarm_20150623
Issue-Key: devicefarm.project-vpc-config-validates-against-real-ec2-subnets|_error#23|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# UpdateProject accepts a vpcConfig naming a nonexistent EC2 subnet instead of refusing with ArgumentException

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateProject

## Description
Single cause (Cause 1 of 1): DeviceFarm's UpdateProject does not validate the members of `vpcConfig` against the EC2 resources they name, so a well-formed but nonexistent subnet id is accepted and persisted instead of being refused.

AWS: UpdateProject takes a required `arn` (Smithy: UpdateProject.arn -> AmazonResourceName, required, @length min 32 max 1011, @pattern ^arn:aws:devicefarm:.+$) and an optional `vpcConfig` (Smithy: shape VpcConfig with required members securityGroupIds (VpcSecurityGroupIds, @length 1..5), subnetIds (VpcSubnetIds, @length 1..8) and vpcId (NonEmptyString, @length 1..4096, @pattern \S)). UpdateProject models ArgumentException (Smithy: errorKind client, httpError 400, member `message` -> Message @length 0..16384, doc "An invalid argument was specified."). The model does not fix the message text, so the message is recorded behavior only: when a subnet id in `vpcConfig.subnetIds` is syntactically valid but names no existing subnet, AWS resolves the id against EC2, the nested EC2 call fails, and DeviceFarm answers HTTP 400 with error code ArgumentException and forwards EC2's own error verbatim with the message template "The subnet ID '<subnetId>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <ec2 request id>)". Fixed text of that template: "The subnet ID '", "' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: ", ")". Variable parts: the subnet id, taken verbatim from the request's vpcConfig.subnetIds member, and the per-call EC2 request id (a per-run value, not part of the requirement — the check redacts it). Consequently the refused update is a no-op: a subsequent GetProject still returns the project's previous `vpcConfig`.

Emulator: the call is not refused at all. UpdateProject with a vpcConfig whose subnetIds contains a well-formed subnet id that exists in no EC2 state returns HTTP 200 with the project echoed back carrying that unknown subnet id (replay: `expected the call to be refused, but it succeeded: {'project': {... 'vpcConfig': {'securityGroupIds': [...], 'subnetIds': ['subnet-<unknown>'], 'vpcId': ...}}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}`). No ArgumentException and no 400 are produced, and because the write is applied, the project's previously-valid vpcConfig is overwritten by the invalid one rather than surviving untouched.

Why: a client that relies on DeviceFarm validating VPC wiring at configuration time gets no signal that the project now points at a subnet that does not exist. Infrastructure tooling that treats a 200 from UpdateProject as "this VPC configuration is usable" proceeds to schedule runs against an unroutable configuration, and error-handling paths keyed on ArgumentException / the InvalidSubnetID.NotFound text are never exercised. Worse, because the rejected update is applied anyway, the emulator loses the project's prior good vpcConfig — state a caller on real AWS would still be able to read back.

Fix: the UpdateProject handler's vpcConfig validation in the devicefarm service crate (services/service-com-amazonaws-devicefarm-devicefarm-20150623/src/ops/update_project.rs and whatever shared vpc-config validation it should call). It must resolve each id in vpcConfig.subnetIds (and, for the same reason, securityGroupIds and vpcId) against the emulator's EC2 state before mutating project state, and on an unresolvable subnet raise ArgumentException with HTTP 400 and the forwarded-EC2 message template above; the mutation must happen only after validation succeeds, so a refused update leaves the stored vpcConfig unchanged. CreateProject's vpcConfig path should share the same validation.

Traces: devicefarm.project-vpc-config-validates-against-real-ec2-subnets (the positive half — a real subnet/SG/VPC accepted and read back by GetProject — passes; the failure is the negative half). No open devicefarm issue covers this: the only open one, 20261029T180559Z-devicefarm-notfoundexception-message-wording-diverges-from-aws-for.md, is about GetProject's NotFoundException message wording.

## Evidence
Expected (AWS): UpdateProject with vpcConfig.subnetIds = ['subnet-<well-formed but nonexistent>'] -> HTTP 400, Error.Code "ArgumentException", Error.Message "The subnet ID '<subnetId>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <ec2 request id>)"; the project's stored vpcConfig is unchanged afterwards.
Actual (emulator): HTTP 200 with content-type application/x-amz-json-1.1 and body {'project': {'arn': ..., 'name': ..., 'defaultJobTimeoutMinutes': 150, 'created': ..., 'vpcConfig': {'securityGroupIds': ['sg-...'], 'subnetIds': ['subnet-783cb00a9419406b9'], 'vpcId': 'vpc-...'}}} — i.e. the unknown subnet id was accepted and written. No error code, no 400. The replay fails with AssertionError: "expected the call to be refused, but it succeeded".
The subnet/SG/VPC/project ids and the timestamp in that body are per-run emulator-generated values and are not part of the requirement; the requirement is the status, the error code and the message template.

## Steps to Reproduce
1. Create an EC2 VPC, a subnet in it and a security group in it (ec2 CreateVpc / CreateSubnet / CreateSecurityGroup).
2. devicefarm CreateProject with any name; keep the returned project arn.
3. devicefarm UpdateProject(arn=<project arn>, vpcConfig={securityGroupIds:[<sg id>], subnetIds:[<the real subnet id>], vpcId:<the real vpc id>}) — succeeds, and GetProject reads the config back (this part already behaves correctly).
4. devicefarm UpdateProject(arn=<project arn>, vpcConfig={securityGroupIds:[<sg id>], subnetIds:["subnet-" + 17 random hex chars, i.e. a well-formed id that was never created], vpcId:<the real vpc id>}).
5. Observe HTTP 200 and the bogus subnet id echoed in the response, where AWS answers 400 ArgumentException with the forwarded EC2 InvalidSubnetID.NotFound message.
6. Call GetProject(arn=<project arn>) and observe the vpcConfig has been overwritten with the invalid config, where AWS still returns the config from step 3.

Replay: run behavioral check devicefarm.project-vpc-config-validates-against-real-ec2-subnets.
