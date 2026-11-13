Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: f0788135e0bb0704427b37eee4190850f5fa33c6

# The managed EC2 instance a WorkSpace Instance launches is never created in EC2 state, so the EC2ManagedInstance.InstanceId this service reports resolves nowhere

_Filed: 2026-10-24T12:27:25Z_
_Operation: CreateWorkspaceInstance | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `f0788135e0bb0704427b37eee4190850f5fa33c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateWorkspaceInstance mints a synthetic `i-<counter>` identifier inside this crate's own store and returns it (through GetWorkspaceInstance / ListWorkspaceInstances) as `EC2ManagedInstance.InstanceId`, but no EC2 instance is ever created in the EC2 emulator's state. Nothing in `ManagedInstance` is acted on either: `SubnetId`, `SecurityGroupIds`, `ImageId`, `KeyName`, `IamInstanceProfile`, `BlockDeviceMappings`, `Placement`, `UserData` and `TagSpecifications` are stored verbatim on the WorkSpaces record and applied to no resource, and `DeleteWorkspaceInstance` tears down only the local record, terminating nothing.

Expected: a WorkSpace Instance is a managed EC2 instance launched in the caller's own account and VPC — WorkSpaces calls EC2 on the caller's behalf with the caller's subnet, security groups, AMI, key pair and instance profile, and surfaces EC2's own failures back through `EC2InstanceError{EC2ErrorCode, EC2ExceptionType, EC2ErrorMessage}`. So after a successful CreateWorkspaceInstance, `ec2:DescribeInstances` with the reported `EC2ManagedInstance.InstanceId` resolves a running instance in the named subnet carrying the requested type/AMI/tags, and after DeleteWorkspaceInstance that instance is shut down/terminated.

Actual: `ec2:DescribeInstances(InstanceIds=[<the reported i-… id>])` against the emulator answers the EC2 not-found error (`InvalidInstanceID.NotFound`) because the id exists only in this crate's `BTreeMap`, and `ec2:DescribeVolumes` shows no root volume for it. A client that creates a WorkSpace Instance and then inspects or manages the underlying EC2 instance — the reason the service returns that id at all — cannot do so.

## Evidence

Smithy model: `GetWorkspaceInstanceResponse$EC2ManagedInstance` -> `EC2ManagedInstance{InstanceId}` ("Unique identifier of the managed EC2 instance"); `ManagedInstanceRequest` is a near-clone of EC2 RunInstances input (ImageId ^ami-…, InstanceType, SubnetId ^subnet-…, SecurityGroupIds, KeyName, IamInstanceProfile, BlockDeviceMappings, UserData, Placement, TagSpecifications), all of which name resources that only exist in the caller's own account; `GetWorkspaceInstanceResponse$EC2InstanceErrors` -> `EC2InstanceError{EC2ErrorCode, EC2ExceptionType, EC2ErrorMessage}` exposes EC2's own exception type to the caller, i.e. the launch is performed against the caller's EC2.
Handler: services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/create_workspace_instance.rs — `state.mint_ids()` fabricates both ids (src/state.rs `mint_ids`: `format!("i-{n:017x}")`) and the record stores `managed_instance: input.managed_instance` untouched; nothing reaches another service's state. delete_workspace_instance.rs only calls `record.begin_deallocation(now)`.
Feasibility/observability: the EC2 emulator holds real instance state (services/service-com-amazonaws-ec2-amazonec2/src/state.rs `pub instances: HashMap<String, InstanceRecord>`) and both `DescribeInstances` and `RunInstances` are implemented there (src/ops/describe_instances.rs: "Implemented: reports the reservations and instances crate::state holds"), so the divergence is directly observable over HTTP.
Tests: no test in services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/tests/all/ makes any EC2 call; the lifecycle tests only assert that the reported `i-…` id is stable within this service (tests/all/workspace_instance_lifecycle.rs). The open issue 20261024T015846Z-workspaces-instances-createworkspaceinstance-validation covers REFUSING unlaunchable configurations; it does not cover propagating a launched instance into EC2 state.

## Steps to Reproduce

1. Boot the multi-service emulator so both `workspaces-instances` and `ec2` are mounted
2. Call CreateWorkspaceInstance with {"ManagedInstance":{"ImageId":"ami-0123456789","InstanceType":"t3.medium","SubnetId":"<a subnet of the signed Region>"},"Tags":[{"Key":"env","Value":"dev"}]} and keep the returned WorkspaceInstanceId
3. Call GetWorkspaceInstance with that id and read `EC2ManagedInstance.InstanceId` (an `i-…` value)
4. Call ec2:DescribeInstances with InstanceIds=[that i-… value] — the emulator answers InvalidInstanceID.NotFound; expected is a reservation holding that instance in the named subnet with the requested instance type
5. Call DeleteWorkspaceInstance for the WorkSpace Instance, then ec2:DescribeInstances again — no EC2 instance was ever created, so nothing is terminated either
