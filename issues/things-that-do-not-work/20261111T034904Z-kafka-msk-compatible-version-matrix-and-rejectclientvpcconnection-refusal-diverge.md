Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-catalog-get-compatible-kafka-versions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-catalog-get-compatible-kafka-versions kafka-vpc-connection-list-and-reject-on-foreign-cluster
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|GetCompatibleKafkaVersions|2xx->2xx|$.compatibleKafkaVersions[*]
Issue-Key: kafka|RejectClientVpcConnection|4xx->4xx|$.message
Issue-Key: kafka|RejectClientVpcConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|RejectClientVpcConnection|4xx->4xx|status
Issue-Commit: 6b0a119ee0c782fe5cb0865ca1b5cb8bebcdd5f6

# MSK compatible-version matrix and RejectClientVpcConnection refusal diverge (2 causes)

> _Note: this issue was identified against the code base at commit `6b0a119ee0c782fe5cb0865ca1b5cb8bebcdd5f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCompatibleKafkaVersions

## Description
TWO distinct root causes on the kafka (Amazon MSK) wire surface, both net-new (no filed issue owns either replay key). Both must be closed.

Cause 1 of 2 - GetCompatibleKafkaVersions publishes the wrong upgrade matrix when no clusterArn is given.
AWS: Smithy kafka#GetCompatibleKafkaVersions (GET /v1/compatible-kafka-versions, 200) takes an OPTIONAL clusterArn @httpQuery and returns CompatibleKafkaVersions (jsonName compatibleKafkaVersions), a list of CompatibleKafkaVersion{sourceVersion, targetVersions}. The model constrains neither the rows nor their order, so: recorded behavior only - with no clusterArn MSK serves the whole upgrade-compatibility matrix as its OWN published table, in its own order, with one row per (sourceVersion -> targetVersions) pair the service publishes. That table may name the SAME source version in more than one row with different target lists (the recording carries sourceVersion '3.9.x.kraft' twice, once with targetVersions ['4.1.x.kraft','4.0.x.kraft'] and once with ['4.2.x.kraft']), and every row's targetVersions is the per-source upgrade set - a terminal version such as '4.2.x.kraft', '4.1.x.kraft' or '1.1.1' carries an EMPTY list, '4.0.x.kraft' carries exactly ['4.1.x.kraft'], '3.8.x' carries ['3.9.x'], '3.9.x' carries ['3.9.x.kraft'], '3.6.0' carries ['3.9.x','3.8.x'] and '3.8.x.kraft' carries ['4.1.x.kraft','3.9.x.kraft','4.0.x.kraft'].
Emulator: a DIFFERENT table. It is three rows SHORTER than MSK's (the recording asserts rows at indices 36, 37 and 38 - '3.8.x'->['3.9.x'], '3.9.x'->['3.9.x.kraft'] and '1.1.1'->[] - and the emulator's list simply ends, got None), it emits each catalogued version at most ONCE so the second '3.9.x.kraft' row and the '3.6.0'->['3.9.x','3.8.x'] row are absent, it hands pre-KRaft sources one BLANKET target list instead of a per-source one ('2.1.0', '2.4.1' and '2.8.2.tiered' all answer ['3.6.0','3.7.x','3.8.x','3.9.x']), and it offers upgrades MSK does not: '4.0.x.kraft'->['4.1.x.kraft','4.2.x.kraft'] where MSK publishes ['4.1.x.kraft'], and '4.1.x.kraft'->['4.2.x.kraft'] where MSK publishes [] (a terminal version). The ordering of the rows that do exist also differs, which is why the mismatch is reported positionally from index 0 onwards.
Why: this operation is the ONLY way a client discovers which Apache Kafka version an upgrade may target, and UpdateClusterKafkaVersion then rejects anything off that list. A client reading the emulator's table plans an upgrade to a version MSK refuses (4.2.x.kraft from 4.1.x.kraft), believes a terminal version still has headroom, or concludes that a version it can in fact upgrade (3.8.x, 3.9.x) is not upgradable at all because its row is missing - and upgrade-planning tooling that diffs the whole matrix sees a table of the wrong length in the wrong order.
Fix: the no-clusterArn branch of the handler and the version catalog it reads - the catalog rows and the per-source target derivation in the kafka crate's state module (KAFKA_VERSION_CATALOG / catalogued_kafka_versions / compatible_target_versions) must publish MSK's matrix row-for-row and in MSK's order, including repeated source-version rows, instead of deduplicating catalogued versions and deriving one shared target list for pre-KRaft sources.
Traces: kafka-catalog-get-compatible-kafka-versions#task#1 (GetCompatibleKafkaVersions, 200 vs 200, body mismatch on $.compatibleKafkaVersions[0,1,4,16,24,26,27,28,30,31,32,33,34,36,37,38]).

Cause 2 of 2 - RejectClientVpcConnection answers 400 BadRequestException where MSK answers 403 AccessDeniedException for a cluster nothing grants the action on.
AWS: Smithy kafka#RejectClientVpcConnection (PUT /v1/clusters/{ClusterArn}/client-vpc-connection) models BadRequestException(400), ForbiddenException(403), InternalServerErrorException(500), ServiceUnavailableException(503) and UnauthorizedException(401) - no AccessDeniedException and no NotFoundException - so: recorded behavior only. MSK takes the authorization decision on the target cluster BEFORE it resolves it, and a cluster ARN that no resource-based policy grants kafka:RejectClientVpcConnection on (here a well-formed, never-issued cluster ARN of the caller's own account) is refused with HTTP 403, header x-amzn-errortype 'AccessDeniedException', and the IAM sentence: fixed text 'User: ', ' is not authorized to perform: ', ' on resource: ', ' because no resource-based policy allows the ', ' action'; the variable parts are the calling principal's ARN, the action 'kafka:RejectClientVpcConnection' (the API name under the kafka: namespace, repeated in the trailing clause) and the cluster ARN exactly as submitted. Nothing about the registry's contents is disclosed.
Emulator: HTTP 400, x-amzn-errortype 'BadRequestException', $.message 'One or more of the parameters are not valid.' - the request passes the account-only gate (crate::state::authorization::authorize_operation only refuses an ARN of a FOREIGN account), falls through to the cluster lookup, and the unresolved cluster is reported as a bad parameter instead of an authorization refusal.
Why: 403/AccessDeniedException and 400/BadRequestException are different answers a client acts on differently. AccessDenied tells the caller the credential or the cluster policy is the problem (stop retrying, fix permissions); BadRequest tells it the ARN it built is malformed, which it is not - so SDK error-type dispatch takes the wrong branch, permission-diagnostic tooling reports a healthy authorization path, and a correct client retries or crashes on input it assembled correctly. It also leaks the distinction MSK deliberately hides between 'not permitted' and 'not present'.
Fix: the reject handler must take the cluster-scoped authorization decision over the submitted ClusterArn before resolving it, and refuse an ARN this account grants the action on nothing for through the service's access-denied channel (unmodeled code AccessDeniedException, status 403, the no-resource-policy sentence) rather than through the operation's local bad_request() unresolved-reference path - the same widening the cluster-ARN decision already needs in the kafka crate's authorization module, which today refuses only another account's ARN. Related but NOT covering this: open issue 20261029T175822Z-kafka-client-vpc-connection-surface-on-a-nonexistent-cluster.md records this operation as a 501 stub and expects NotFoundException from a behavioral check, and 20261029T175822Z-kafka-msk-arn-handling-malformed-and-cross-region-arns.md covers malformed and cross-region ARNs; neither carries this replay key or this operation's own-account ghost-cluster refusal.
Traces: kafka-vpc-connection-list-and-reject-on-foreign-cluster#task#2 (RejectClientVpcConnection, expected 403 got 400).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (50 fixtures, 163 steps, 129 passed, score 79.1%). Both clusters below have empty stub_path and empty owned_by.

Cause 1 - fixture kafka-catalog-get-compatible-kafka-versions, step task#1, GetCompatibleKafkaVersions, status 200 expected and 200 observed, body diverges:
  $.compatibleKafkaVersions[1]:  expected {'sourceVersion':'3.9.x.kraft','targetVersions':['4.1.x.kraft','4.0.x.kraft']}  got {'sourceVersion':'2.1.0','targetVersions':['3.6.0','3.7.x','3.8.x','3.9.x']}
  $.compatibleKafkaVersions[4]:  expected {'sourceVersion':'3.7.x.kraft','targetVersions':['4.1.x.kraft','3.8.x.kraft','3.9.x.kraft','4.0.x.kraft']}  got {'sourceVersion':'2.4.1','targetVersions':['3.6.0','3.7.x','3.8.x','3.9.x']}
  $.compatibleKafkaVersions[16]: expected {'sourceVersion':'3.9.x.kraft','targetVersions':['4.2.x.kraft']}  got {'sourceVersion':'2.8.2.tiered','targetVersions':['3.6.0','3.7.x','3.8.x','3.9.x']}
  $.compatibleKafkaVersions[27]: expected {'sourceVersion':'3.6.0','targetVersions':['3.9.x','3.8.x']}  got {'sourceVersion':'3.8.x','targetVersions':['3.9.x']}
  $.compatibleKafkaVersions[30]: expected {'sourceVersion':'4.2.x.kraft','targetVersions':[]}  got {'sourceVersion':'3.9.x','targetVersions':['3.9.x.kraft']}
  $.compatibleKafkaVersions[32]: expected {'sourceVersion':'4.0.x.kraft','targetVersions':['4.1.x.kraft']}  got {'sourceVersion':'4.0.x.kraft','targetVersions':['4.1.x.kraft','4.2.x.kraft']}
  $.compatibleKafkaVersions[33]: expected {'sourceVersion':'4.1.x.kraft','targetVersions':[]}  got {'sourceVersion':'4.1.x.kraft','targetVersions':['4.2.x.kraft']}
  $.compatibleKafkaVersions[34]: expected {'sourceVersion':'3.8.x.kraft','targetVersions':['4.1.x.kraft','3.9.x.kraft','4.0.x.kraft']}  got {'sourceVersion':'4.2.x.kraft','targetVersions':[]}
  $.compatibleKafkaVersions[36]: expected {'sourceVersion':'3.8.x','targetVersions':['3.9.x']}  got None
  $.compatibleKafkaVersions[37]: expected {'sourceVersion':'3.9.x','targetVersions':['3.9.x.kraft']}  got None
  $.compatibleKafkaVersions[38]: expected {'sourceVersion':'1.1.1','targetVersions':[]}  got None
  (also indices 0, 24, 26, 28, 31)
The three trailing None rows show the emulator's list is three rows short; indices 32 and 33 show the target sets themselves are wrong even where the source version lines up.

Cause 2 - fixture kafka-vpc-connection-list-and-reject-on-foreign-cluster, step task#2, RejectClientVpcConnection:
  status:                        expected 403, got 400
  $headers.x-amzn-errortype:     expected 'AccessDeniedException', got 'BadRequestException'
  $.message:                     expected 'User: <caller-principal-arn> is not authorized to perform: kafka:RejectClientVpcConnection on resource: <submitted cluster ARN> because no resource-based policy allows the kafka:RejectClientVpcConnection action', got 'One or more of the parameters are not valid.'
In the recording the principal was the emulator's ambient user ARN and the resource a never-issued cluster ARN of the caller's own account (a 'ghostcluster' probe name); those two values come from the request and the emulator's identity - the template, the action name and the trailing resource-policy clause are the fixed requirement. Step task#1 of the same fixture (the ListClientVpcConnections arm) passed, so the refusal is specific to this operation's ordering of authorization vs. resolution.

Handler evidence: services/service-com-amazonaws-kafka-kafka/src/ops/reject_client_vpc_connection.rs lines 158 and 201-206 (account-only authorize_operation, then bad_request(CLUSTER_ARN_MEMBER, UNRESOLVED_REFERENCE_MESSAGE)); services/service-com-amazonaws-kafka-kafka/src/state/authorization.rs lines 132-161 (authorize_action returns Ok for an ARN of the caller's own account); services/service-com-amazonaws-kafka-kafka/src/ops/get_compatible_kafka_versions.rs lines 244-260 (no-clusterArn branch maps each catalogued version once through compatible_target_versions).

## Steps to Reproduce
1. Boot the emulator built from this checkout (admin health 200, data listener accepting) and replay trace set simparity-wire-aws-v1.
2. Cause 1: GET /v1/compatible-kafka-versions with NO clusterArn query parameter (kafka.get_compatible_kafka_versions()). Expect MSK's full matrix: 39 rows in MSK's order, including a second '3.9.x.kraft' row with targetVersions ['4.2.x.kraft'], '3.6.0'->['3.9.x','3.8.x'], '3.8.x'->['3.9.x'], '3.9.x'->['3.9.x.kraft'], '1.1.1'->[], '4.0.x.kraft'->['4.1.x.kraft'] and '4.1.x.kraft'->[]. Observe 36 rows in a different order, a blanket ['3.6.0','3.7.x','3.8.x','3.9.x'] target list on pre-KRaft sources, and '4.2.x.kraft' offered from both '4.0.x.kraft' and '4.1.x.kraft'. Re-replay kafka-catalog-get-compatible-kafka-versions#task#1.
3. Cause 2: PUT /v1/clusters/{ClusterArn}/client-vpc-connection with ClusterArn a well-formed but never-issued cluster ARN of the emulator's OWN account (arn:aws:kafka:<region>:<account>:cluster/<never-issued-name>/00000000-0000-0000-0000-000000000000-1) and any well-formed vpcConnectionArn. Expect HTTP 403 with x-amzn-errortype 'AccessDeniedException' and the message 'User: <caller principal> is not authorized to perform: kafka:RejectClientVpcConnection on resource: <that cluster ARN> because no resource-based policy allows the kafka:RejectClientVpcConnection action'. Observe HTTP 400, 'BadRequestException', 'One or more of the parameters are not valid.'. Re-replay kafka-vpc-connection-list-and-reject-on-foreign-cluster#task#2.
