Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored|run#61|AssertionError|assert [g["DBParameterGroupName"] for g in pgs] == [pg_name], (
Issue-Commit: 45d3476fddc0c303f86586399cfbfaaac87d2cc4

# rds: a CFN-generated AWS::RDS::DBParameterGroup name is reported mixed-case as the stack resource's PhysicalResourceId while RDS stores and returns it lowercase, so the retained group cannot be matched by name (1 cause)

> _Note: this issue was identified against the code base at commit `45d3476fddc0c303f86586399cfbfaaac87d2cc4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBParameterGroup

## Description
One net-new root cause, grounded in the single surviving failure of this replay for service `rds` (every other `rds` failing check in this run is marked ALREADY FILED or OWNED by an open issue).

Cause 1 of 1: when CloudFormation auto-names an `AWS::RDS::DBParameterGroup` (no `DBParameterGroupName` in the template), the name CloudFormation reports as the stack resource's `PhysicalResourceId` is NOT the name RDS stores for that group: CFN reports a mixed-case `<stack-name>-<LogicalId>-<SUFFIX>` while RDS stores and returns the same name lowercased. Failing check: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.

AWS: `CreateDBParameterGroup.DBParameterGroupName` is documented in the RDS Smithy model (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json, CreateDBParameterGroupMessage$DBParameterGroupName) as "Must be 1 to 255 letters, numbers, or hyphens. First character must be a letter. Can't end with a hyphen or contain two consecutive hyphens" with the note "This value is stored as a lowercase string." So RDS lowercasing what it stores is correct, and real CloudFormation therefore hands back, as the resource's PhysicalResourceId, a name that is byte-identical to the `DBParameterGroupName` that DescribeDBParameterGroups returns for that group: the identifier a client reads out of DescribeStackResources/DescribeStackResource (and out of `Ref` on the resource) round-trips exactly into RDS reads. Recorded behavior only for the generated-name shape itself (AWS does not model CFN's physical-name generation); the check grades only the equality of the two values, never a literal recorded name.

Emulator: the whole flow now converges up to the last stage — stack CREATE_COMPLETE with the NoEcho password masked, the DeletionProtection=true DeleteStack settling DELETE_FAILED with StackStatusReason `The following resource(s) failed to delete: [Db]. ` and a Db DELETE_FAILED event citing deletion protection, UpdateStack on the DELETE_FAILED stack refused ValidationError, the second DeleteStack reaching DELETE_COMPLETE, DescribeDBInstances answering DBInstanceNotFound, and the DeletionPolicy=Snapshot final snapshot now present and `available`. Then the DeletionPolicy=Retain parameter group cannot be matched by the identifier the stack reported: DescribeStackResources gives PhysicalResourceId `...-Pg-F19332ECF625` (logical id and random suffix in their original case) while DescribeDBParameterGroups returns `DBParameterGroupName` (and a `DBParameterGroupArn` whose resource part) fully lowercased, `...-pg-f19332ecf625`. The lookup itself is case-insensitive — DescribeDBParameterGroups(DBParameterGroupName=<mixed-case id>) does return the group — so only the ECHOED name diverges. Hypothesis: the CloudFormation RDS DBParameterGroup provisioner generates the physical name preserving the logical id's and suffix's case and records that string as the PhysicalResourceId, while the RDS CreateDBParameterGroup handler normalises the name to lowercase before storing it, and the two are never reconciled.

Why: the PhysicalResourceId (and `Ref`) of a CFN-managed resource is the identifier a client is expected to feed back to the owning service. Any client that reads the stack's resources and then compares, indexes, caches or reaps by that name — the normal pattern for a `DeletionPolicy: Retain` resource that outlives its stack, and for cross-stack/`Ref`-driven wiring — sees a name RDS never reports, so exact-match logic (`name == group['DBParameterGroupName']`, dictionary/set membership, tag or ARN matching) silently fails and the retained group looks absent even though it exists.

Fix: reconcile CloudFormation's physical-name generation for RDS resources with the RDS handler's name normalisation — the CloudFormation AWS::RDS::DBParameterGroup provisioning path must record, as the resource's PhysicalResourceId, the name RDS actually stores (lowercase, per the model's note), i.e. generate the name already lowercased or adopt the name echoed by the CreateDBParameterGroup response; the same reconciliation applies to the other CFN-auto-named RDS resource types whose names RDS stores lowercase (DB instance/cluster identifiers, DB subnet groups, option groups).

Traces: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored (assertion at check line 166, `assert [g["DBParameterGroupName"] for g in pgs] == [pg_name]`).

Related open issue that does NOT cover this: 20261029T080433Z-rds-a-cloudformation-deletionpolicy-snapshot-aws-rds-dbinstance.md records the earlier stage of this same check (no final snapshot after DELETE_COMPLETE; its replay key is the TimeoutError in `_wait_snapshot_available`). That stage now passes — the snapshot is produced and reaches `available` — and the check advances to and fails on this name-case divergence, so fixing the snapshot cause alone leaves this check failing.

## Evidence
Expected (real AWS): after the stack reaches DELETE_COMPLETE, DescribeDBParameterGroups(DBParameterGroupName=<the PhysicalResourceId that DescribeStackResources reported for the Retain-policy AWS::RDS::DBParameterGroup>) returns exactly one group whose DBParameterGroupName is byte-identical to that PhysicalResourceId, with DBParameterGroupFamily = the template's Family and the user parameter max_connections = the template's value.

Actual (emulator): the two identifiers differ only in case — the stack reports the generated name with its original capitalisation, RDS reports it lowercased. Replay output for the check, verbatim:

  #### [mismatch] rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
  File "check_rds_deploy_rds_1_protected_stack_delete_blocked_then_snapshot_retain_honored.py", line 166, in run
  assert [g["DBParameterGroupName"] for g in pgs] == [pg_name], (
  AssertionError: DeletionPolicy Retain parameter group 'sim2real-rds-dp-09325095fb6d-Pg-F19332ECF625' not found after stack delete: [{'DBParameterGroupName': 'sim2real-rds-dp-09325095fb6d-pg-f19332ecf625', 'DBParameterGroupFamily': 'mysql8.4', 'Description': 'sim2real retained parameter group', 'DBParameterGroupArn': 'arn:aws:rds:us-west-2:000000000000:pg:sim2real-rds-dp-09325095fb6d-pg-f19332ecf625'}]

The group exists, its family and description are right, and the DeletionPolicy=Retain itself was honoured; the only divergence is the case of the echoed name. In the strings above, the fixed parts of the requirement are: the generated name's shape `<stack-name>-<LogicalId>-<suffix>` and the equality between DescribeStackResources' PhysicalResourceId and DescribeDBParameterGroups' DBParameterGroupName (and the ARN's resource part). The stack name `sim2real-rds-dp-09325095fb6d`, the suffix `F19332ECF625`, the account id, region and family `mysql8.4` are per-run/environment values that merely fill the template and are not part of it.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. rds: DescribeDBEngineVersions(Engine='mysql', DefaultOnly=true) to get the default EngineVersion and DBParameterGroupFamily; pick two AZs via DescribeOrderableDBInstanceOptions + ec2 DescribeAvailabilityZones.
3. cloudformation: CreateStack with VPC + 2 Subnets + SecurityGroup + AWS::RDS::DBSubnetGroup + AWS::RDS::DBParameterGroup (logical id `Pg`, DeletionPolicy and UpdateReplacePolicy `Retain`, Properties {Description, Family, Parameters {max_connections: '77'}} and NO DBParameterGroupName) + AWS::RDS::DBInstance (logical id `Db`, DBParameterGroupName {Ref: Pg}, DeletionProtection=true, DeletionPolicy/UpdateReplacePolicy `Snapshot`). Wait for CREATE_COMPLETE.
4. cloudformation: DescribeStackResources(StackName=<stack-id>) and note the `Pg` resource's PhysicalResourceId — it carries the logical id and random suffix in their original case.
5. rds: DescribeDBParameterGroups(DBParameterGroupName=<that PhysicalResourceId>). Observe the returned DBParameterGroups[0].DBParameterGroupName (and DBParameterGroupArn) are the SAME name fully lowercased, so an exact string comparison with the PhysicalResourceId fails.
6. (The same mismatch persists after DeleteStack: lift DeletionProtection with ModifyDBInstance(ApplyImmediately=true), DeleteStack again to DELETE_COMPLETE, and the retained group is still only findable under the lowercased name.)
