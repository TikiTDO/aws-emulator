Issue-Source: trace-sweep-divergence
Issue-Trace: rds-cluster-parameter-group-mutation-envelopes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-cluster-parameter-group-mutation-envelopes rds-create-db-instance-catalog-triple rds-db-cluster-lifecycle rds-event-subscription-lifecycle rds-event-subscription-mutation-envelopes rds-global-cluster-delete-with-member rds-global-cluster-mutation-envelopes rds-identifier-validation-and-lowercasing rds-parameter-group-enforcement
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBClusterParameterGroup|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateDBClusterParameterGroup|4xx->2xx|status
Issue-Key: rds|ModifyDBClusterParameterGroup|4xx->2xx|$.ErrorResponse
Issue-Key: rds|ModifyDBClusterParameterGroup|4xx->2xx|status
Issue-Key: rds|CreateDBInstance|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateDBInstance|4xx->2xx|status
Issue-Key: rds|ModifyDBCluster|4xx->2xx|$.ErrorResponse
Issue-Key: rds|ModifyDBCluster|4xx->2xx|status
Issue-Key: rds|CreateEventSubscription|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateEventSubscription|4xx->2xx|status
Issue-Key: rds|ModifyEventSubscription|4xx->2xx|$.ErrorResponse
Issue-Key: rds|ModifyEventSubscription|4xx->2xx|status
Issue-Key: rds|DeleteGlobalCluster|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DeleteGlobalCluster|4xx->2xx|status
Issue-Key: rds|CreateGlobalCluster|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateGlobalCluster|4xx->2xx|status
Issue-Key: rds|ModifyDBParameterGroup|4xx->2xx|$.ErrorResponse
Issue-Key: rds|ModifyDBParameterGroup|4xx->2xx|status
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS accepts nine requests AWS refuses: missing request and state preconditions

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBClusterParameterGroup

## Description
Nine requests that AWS refuses are answered 200 with a success envelope, so the emulator silently accepts and records state AWS never creates. In every case the recording carries a 400/404 error body and the emulator returns no $.ErrorResponse at all. 9 causes.

Cause 1 of 9: CreateDBClusterParameterGroup with a DBParameterGroupFamily that does not exist.
AWS: recorded 400, Code InvalidParameterValue, Type Sender, Message 'ParameterGroupFamily <DBParameterGroupFamily> is not a valid parameter group family'. Fixed text: "ParameterGroupFamily " + " is not a valid parameter group family"; the variable part is the family from the request.
Emulator: 200 with a CreateDBClusterParameterGroupResult and no error.
Why: a typo'd family is accepted, so a client believes a group exists for an engine family RDS does not have.
Fix: the CreateDBClusterParameterGroup handler's family validation.
Traces: rds-cluster-parameter-group-mutation-envelopes#task#3.

Cause 2 of 9: ModifyDBClusterParameterGroup naming a parameter that does not exist in the family.
AWS: recorded 400, Code InvalidParameterValue, Message 'Could not find parameter with name: <ParameterName>'. Fixed prefix "Could not find parameter with name: " plus the requested parameter name.
Emulator: 200, and the unknown parameter is stored (it is later echoed by DescribeDBClusterParameters).
Why: unknown parameter names are persisted, corrupting later reads.
Fix: the ModifyDBClusterParameterGroup handler's parameter-name validation.
Traces: rds-cluster-parameter-group-mutation-envelopes#task#5.

Cause 3 of 9: CreateDBInstance with an unsupported DBInstanceClass/Engine/EngineVersion/LicenseModel combination.
AWS: recorded 400, Code InvalidParameterCombination, Message 'RDS does not support creating a DB instance with the following combination: DBInstanceClass=<class>, Engine=<engine>, EngineVersion=<version>, LicenseModel=<license>. For supported combinations of instance class and database engine version, see the documentation.' Fixed text as quoted; the four variable parts are echoed from the request.
Emulator: 200, instance created.
Why: a client can create an instance shape RDS would reject, so capacity/compatibility tests pass that must fail.
Fix: the CreateDBInstance handler's engine/class compatibility validation.
Traces: rds-create-db-instance-catalog-triple#task#10.

Cause 4 of 9: ModifyDBCluster while the cluster is still creating.
AWS: recorded 400, Code InvalidDBClusterStateFault, Message "DB cluster isn't available for modification with status <status>." Fixed text as quoted; the variable part is the cluster's current status.
Emulator: 200, modification applied.
Why: the state machine is unobservable — a client can mutate a cluster that is not yet available.
Fix: the ModifyDBCluster handler's state precondition.
Traces: rds-db-cluster-lifecycle#task#3.

Cause 5 of 9: CreateEventSubscription naming an SNS topic that does not exist.
AWS: recorded 404, Code SNSTopicArnNotFound, Message 'Failed to create Subscription because of Topic <SnsTopicArn> Not Found.' Fixed text: "Failed to create Subscription because of Topic " + " Not Found."; the variable part is the request's SnsTopicArn.
Emulator: 200, subscription created against a nonexistent topic.
Why: the subscription's delivery target is never validated, so notification wiring tests pass with a dead topic.
Fix: the CreateEventSubscription handler's SNS topic existence check.
Traces: rds-event-subscription-lifecycle#task#5.

Cause 6 of 9: ModifyEventSubscription supplying EventCategories with a null SourceType.
AWS: recorded 400, Code InvalidParameterCombination, Message 'If SourceType is null, Categories must also be null.' — entirely fixed text.
Emulator: 200, modification applied.
Why: an inconsistent subscription is accepted.
Fix: the ModifyEventSubscription handler's parameter-combination validation.
Traces: rds-event-subscription-mutation-envelopes#task#9.

Cause 7 of 9: DeleteGlobalCluster while the global cluster still has members.
AWS: recorded 400, Code InvalidGlobalClusterStateFault, Message 'Global Cluster <GlobalClusterArn> is not empty'. Fixed text: "Global Cluster " + " is not empty"; the variable part is the global cluster ARN.
Emulator: 200, the non-empty global cluster is deleted.
Why: a destructive operation AWS blocks succeeds, orphaning member clusters.
Fix: the DeleteGlobalCluster handler's membership precondition.
Traces: rds-global-cluster-delete-with-member#task#8; also rds-global-cluster-mutation-envelopes#task#6.

Cause 8 of 9: CreateGlobalCluster with a GlobalClusterIdentifier that violates the identifier rule.
AWS: Smithy CreateGlobalCluster input GlobalClusterIdentifier @length 1..255, @pattern ^[A-Za-z][0-9A-Za-z-:._]*$. Recorded 400, Code InvalidParameterValue, Message 'The parameter GlobalClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' Fixed text as quoted, with the member name GlobalClusterIdentifier as its only variable part.
Emulator: 200, the invalid identifier is accepted.
Why: the modeled identifier constraint is not enforced, so identifiers AWS rejects become live state.
Fix: the CreateGlobalCluster handler's identifier validation.
Traces: rds-identifier-validation-and-lowercasing#task#7; also #task#8.

Cause 9 of 9: ModifyDBParameterGroup setting a parameter that is not modifiable.
AWS: recorded 400, Code InvalidParameterValue, Message 'The parameter <ParameterName> cannot be modified.' Fixed text: "The parameter " + " cannot be modified."; the variable part is the requested parameter name.
Emulator: 200, the non-modifiable parameter is stored (and later echoed by DescribeDBParameters).
Why: IsModifiable is not honoured, so a client can set engine parameters RDS protects.
Fix: the ModifyDBParameterGroup handler's IsModifiable enforcement.
Traces: rds-parameter-group-enforcement#task#3; also #task#4, #task#5.

## Evidence
Replay cluster details — expected status 400/404 with an $.ErrorResponse body, actual 200 with no error:
1. rds-cluster-parameter-group-mutation-envelopes#task#3 CreateDBClusterParameterGroup 400->200; $.ErrorResponse missing (expected Code InvalidParameterValue, Message 'ParameterGroupFamily aurora-postgresql99 is not a valid parameter group family').
2. #task#5 ModifyDBClusterParameterGroup 400->200; expected InvalidParameterValue 'Could not find parameter with name: rdsenv_no_such_parameter'.
3. rds-create-db-instance-catalog-triple#task#10 CreateDBInstance 400->200; expected InvalidParameterCombination 'RDS does not support creating a DB instance with the following combination: DBInstanceClass=db.t2.micro, Engine=mysql, EngineVersion=8.0.40, LicenseModel=general-public-license. ...'.
4. rds-db-cluster-lifecycle#task#3 ModifyDBCluster 400->200; expected InvalidDBClusterStateFault "DB cluster isn't available for modification with status creating.".
5. rds-event-subscription-lifecycle#task#5 CreateEventSubscription 404->200; expected SNSTopicArnNotFound 'Failed to create Subscription because of Topic arn:aws:sns:us-west-2:000000000000:sim2real-probe-rds-nonexistent-topic Not Found.'.
6. rds-event-subscription-mutation-envelopes#task#9 ModifyEventSubscription 400->200; expected InvalidParameterCombination 'If SourceType is null, Categories must also be null.'.
7. rds-global-cluster-delete-with-member#task#8 DeleteGlobalCluster 400->200; expected InvalidGlobalClusterStateFault 'Global Cluster arn:aws:rds::000000000000:global-cluster:rdserr-gm-gc-a599b675 is not empty'.
8. rds-identifier-validation-and-lowercasing#task#7 CreateGlobalCluster 400->200; expected InvalidParameterValue 'The parameter GlobalClusterIdentifier is not a valid identifier. ...'.
9. rds-parameter-group-enforcement#task#3 ModifyDBParameterGroup 400->200; expected InvalidParameterValue 'The parameter basedir cannot be modified.'.
(ARNs, family names and generated ids above are this run's values; the requirement is the status, code and the fixed sentence with the request's own values substituted.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-cluster-parameter-group-mutation-envelopes#task#3 and every step in trace_ids; each sends the invalid request and the replay compares status and $.ErrorResponse against the recording.

## Resolution
Eight of the nine causes are fixed: each of those requests is now refused with the status, the
error code and the sentence the recording carries, built from the request and the stored
record. Cause 2 (ModifyDBClusterParameterGroup naming a parameter the family does not publish)
is DECLINED for a family whose parameter vocabulary this emulator only SAMPLES — the
MySQL- and PostgreSQL-compatible families, whose parameters are the open-source engines' own
system variables, as both published catalogs state in their module headers. Refusing a name
against a sample rejects parameters AWS accepts (a stack declaring `time_zone` on an
aurora-mysql cluster group proved it), and completeness there would mean publishing hundreds
of records per family that nobody has in hand. The rule itself is implemented and enforced for
every family whose vocabulary IS complete — the service-managed Amazon DocumentDB and Amazon
Neptune lists this emulator carries — in one place both the modify and the reset read
(`crate::state::validate_cluster_parameter_name`, gated on
`crate::state::publishes_a_complete_vocabulary`). So the replay still reports that one cause's
two keys failing, and it will keep doing so until those families' vocabularies are published
in full.
