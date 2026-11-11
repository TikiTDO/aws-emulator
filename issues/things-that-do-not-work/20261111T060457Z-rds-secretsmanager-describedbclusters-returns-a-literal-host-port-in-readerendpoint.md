Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.refarch-aur-reader-endpoint-exists-with-zero-reader-instances
Issue-Key: rds__secretsmanager.refarch-aur-reader-endpoint-exists-with-zero-reader-instances|run#14|AssertionError|assert reader_host.startswith(f"{name}.cluster-ro-"), (
Issue-Commit: 0dd06decd64ca030d29e4dddabe070ef79046880

# DescribeDBClusters returns a literal host:port in ReaderEndpoint/Endpoint instead of cluster-identifier-derived DNS endpoints

> _Note: this issue was identified against the code base at commit `0dd06decd64ca030d29e4dddabe070ef79046880`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
Cause 1 of 1: An Aurora DB cluster's writer and reader endpoints are not represented as DNS endpoint names.

AWS: Smithy (com-amazonaws-rds-amazonrdsv19) DBCluster has three distinct members for how a cluster is reached: `Endpoint` (the writer connection endpoint), `ReaderEndpoint` — documented as "The reader endpoint for the DB cluster. The reader endpoint for a DB cluster load-balances connections across the Aurora Replicas that are available in a DB cluster" — `Port` (an Integer, the single port both endpoints are reached on) and `HostedZoneId` (the Route 53 hosted zone the endpoints live in). The endpoint members carry a DNS host NAME only, never an embedded port: the port is modelled separately in `Port`. Aurora mints both names from the cluster identifier, the writer as `<DBClusterIdentifier>.cluster-<suffix>` and the reader as `<DBClusterIdentifier>.cluster-ro-<suffix>`, where the two share one DNS suffix (the regional `rds.amazonaws.com` zone named by `HostedZoneId`) and one `Port`. The reader endpoint is minted by the CLUSTER, so it exists and keeps that form even when `DBClusterMembers` is empty — there is no reader instance to derive it from. The identifier-derived prefixes and the shared suffix are the requirement; the concrete suffix/zone/port of any one environment is not.

Emulator: CreateDBCluster followed by DescribeDBClusters on an aurora-postgresql Serverless v2 cluster with zero members returns a `ReaderEndpoint` that is a literal socket address with the port glued on — the replay observed `'127.0.0.1:33453'` for cluster `'sim2real-aur-175573fc45d5'`. It neither starts with `<DBClusterIdentifier>.cluster-ro-` nor is a bare host name: the port is duplicated into the endpoint string instead of being left to `Port`. The exact address is environment-specific and is not the finding; the finding is that the value has no `<identifier>.cluster-ro-<suffix>` structure and embeds a port.

Why: a client cannot tell a writer endpoint from a reader endpoint, because the discriminating `cluster-` vs `cluster-ro-` infix is gone, and every client that composes a connection string from `Endpoint`/`ReaderEndpoint` plus `Port` (every driver, every DSN builder, psycopg/JDBC URL assembly, and any code that splits the host's DNS suffix or pairs it with `HostedZoneId`) builds a malformed target with the port appearing twice. Code that routes reads to the reader and writes to the writer silently sends both to the same place.

Fix: the RDS cluster-endpoint representation — where CreateDBCluster/RestoreDBClusterFromSnapshot mint a cluster's endpoints into state and where DescribeDBClusters serializes DBCluster.Endpoint/ReaderEndpoint/Port/HostedZoneId. The reachable address must stay reachable, but the port belongs in `Port` and the two endpoint names must be derived from the cluster identifier with the `cluster-` and `cluster-ro-` infixes over one shared DNS suffix, independent of how many members the cluster has.

Traces: rds__secretsmanager.refarch-aur-reader-endpoint-exists-with-zero-reader-instances (mismatch) — asserts `ReaderEndpoint.startswith(f"{name}.cluster-ro-")` after confirming `DBClusterMembers == []` and `ReadReplicaIdentifiers == []`.

## Evidence
Check: rds__secretsmanager.refarch-aur-reader-endpoint-exists-with-zero-reader-instances [mismatch]

Replay output:
  File "check_rds__secretsmanager_refarch_aur_reader_endpoint_exists_with_zero_reader_instances.py", line 61, in run
    assert reader_host.startswith(f"{name}.cluster-ro-"), (
  AssertionError: the reader endpoint is derived from the cluster identifier with a 'cluster-ro-' infix; got '127.0.0.1:33453' for cluster 'sim2real-aur-175573fc45d5'

Expected (AWS): DescribeDBClusters.DBClusters[0].ReaderEndpoint is a bare DNS host of the form <DBClusterIdentifier>.cluster-ro-<shared-suffix>, distinct from Endpoint (<DBClusterIdentifier>.cluster-<shared-suffix>), both under the same non-empty DNS suffix, with the port reported only in the integer member Port and the zone in HostedZoneId.

Actual (emulator): ReaderEndpoint = "<address>:<port>" (observed '127.0.0.1:33453') — no identifier-derived prefix, no 'cluster-ro-' infix, and the port embedded in the host string. The preceding assertions (DBClusterMembers == [], ReadReplicaIdentifiers == [], ReaderEndpoint non-empty and != Endpoint) passed, so only the endpoint representation diverges. The literal address/port shown is environment-specific; the divergence is the missing structure.

## Steps to Reproduce
1. Boot the emulator and point an RDS client at it.
2. CreateDBSubnetGroup over the shared network's subnets.
3. CreateDBCluster(DBClusterIdentifier="<id>", Engine="aurora-postgresql", EngineVersion=<newest 16.x from DescribeDBEngineVersions>, MasterUsername="dbadmin", MasterUserPassword=..., ServerlessV2ScalingConfiguration={MinCapacity:0.5,MaxCapacity:1.0}) and create NO DB instances.
4. Poll DescribeDBClusters(DBClusterIdentifier="<id>") until Status == "available".
5. Inspect the returned DBCluster: DBClusterMembers is [] (correct), but ReaderEndpoint is a "host:port" literal rather than a name beginning "<id>.cluster-ro-", and Endpoint likewise does not begin "<id>.cluster-"; the two do not share a DNS suffix after their per-cluster prefix.
Expected: ReaderEndpoint == "<id>.cluster-ro-<suffix>", Endpoint == "<id>.cluster-<suffix>", same <suffix>, port only in Port.
