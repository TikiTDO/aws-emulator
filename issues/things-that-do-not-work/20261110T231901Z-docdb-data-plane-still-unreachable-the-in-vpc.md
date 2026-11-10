Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb.cluster-mongo-insert-find-round-trip
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb.cluster-mongo-insert-find-round-trip|run#23|AssertionError|raise AssertionError(
Issue-Commit: b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca

# docdb data plane still unreachable: the in-VPC Lambda probe can open no TCP socket at all — socket.create_connection to the advertised instance endpoint fails with "a Lambda function in this emulator has no host network to reach"

> _Note: this issue was identified against the code base at commit `b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
Cause 1 of 1: the emulator's Lambda execution environment provides no network stack to a function, so a handler running inside the run's VPC cannot open a TCP connection to anything — including the DocumentDB instance endpoint the emulator itself published. Failing check: docdb.cluster-mongo-insert-find-round-trip.

This is NOT the failure already filed for this check. Open issue 20261109T235356Z-docdb-data-plane-check-cannot-run-the-emulator (python3.12 runtime missing _ssl, replay key `_probe_call#21|AssertionError|assert "FunctionError" not in resp`) is about the probe not importing at all; the probe now imports and runs, and the invoke returns no FunctionError. Open issue 20261109T120625Z-docdb-cluster-instance-endpoints-are-published-as-real (replay key `_probe_call#21|PublishedUrlRejected`) is about the *address* DocDB advertises; this cause is downstream of it and independent: even an endpoint published on an own host cannot be dialed, because the function has no socket layer to dial with. Neither issue carries this replay key and neither covers it.

AWS: Smithy (com.amazonaws.docdb#AmazonRDSv19) models DBInstance.Endpoint as "Specifies the connection endpoint", and the Endpoint structure (Address "Specifies the DNS address of the instance", Port "Specifies the port that the database engine is listening on") is documented "Network information for accessing a cluster or instance. Client programs must specify a valid endpoint to access these Amazon DocumentDB resources." DocumentDB is VPC-only (DBInstance.PubliclyAccessible is documented as always false), so the only conforming way to reach it is from inside the VPC — on AWS that is exactly what a VpcConfig'd Lambda does: the function is attached to the subnets/security group, a plain socket.create_connection((Address, Port)) succeeds, TLS completes, and a SCRAM-SHA-1 MongoDB session inserts and finds the document. For the Lambda side itself the model says nothing about sandbox networking (recorded behavior only): on AWS a function in a VPC has a working network namespace and outbound TCP to any address its security groups and routes admit.

Emulator: the Lambda handler's `socket.create_connection((host, port), timeout=10)` raises an OSError whose message is the emulator's own text, "a Lambda function in this emulator has no host network to reach". The probe therefore returns {"ok": false, "error": "connect: OSError: a Lambda function in this emulator has no host network to reach"} and the check fails with DATAPLANE_UNIMPLEMENTED: the instance endpoint advertised in DescribeDBInstances accepts no TCP connection from inside its own VPC. The failure is not an AWS-shaped refusal at all: no error code, no status — an emulator-authored OSError surfaced to customer code in place of a connected socket. The instance identifier, account id, region and the port in the captured message are per-run/environment values; the requirement is that a connect from an in-VPC function to an admitted address succeeds.

Why: a client that does what the DocDB model documents — read DBInstance.Endpoint.Address/Port and connect from inside the cluster's VPC — gets an unconditional connect failure, so the whole DocumentDB data plane (authenticate, insert, find, aggregate, change streams, transactions, indexes) is unobservable. More broadly, any Lambda-hosted client of any in-VPC data plane (DocDB, ElastiCache, RDS, a private HTTP service) is impossible on this emulator, and the error text points at the emulator's own sandbox rather than at anything the customer configured, so a user debugging it is sent to security groups and subnets that are in fact correct.

Fix: the Lambda execution environment of the Lambda crate — the sandbox's socket/network layer, which currently short-circuits outbound connections with that hard-coded OSError, must give a VpcConfig'd function a usable network namespace and route outbound TCP to the emulator's own listeners (subject to the security-group/route checks the EC2 state models). Paired with it, the docdb cluster/instance endpoint state and serializer must publish an address the emulator actually serves and stand a MongoDB wire listener behind it (that half is open issue 20261109T120625Z-docdb-cluster-instance-endpoints-are-published-as-real); re-replay must get past both.

Traces: docdb.cluster-mongo-insert-find-round-trip (fails at the DATAPLANE_UNIMPLEMENTED assertion after _probe_call returns error "connect: OSError: ...").

## Evidence
Expected (AWS): the in-VPC python3.12 probe's socket.create_connection((Endpoint.Address, Endpoint.Port)) connects, TLS wraps, SCRAM-SHA-1 authenticates as the cluster master user, the insert reports ok=1/n=1 and the find returns exactly the inserted document.

Actual (emulator): Lambda Invoke succeeds (no FunctionError) but the handler's connect raises, and the probe reports error "connect: OSError: a Lambda function in this emulator has no host network to reach". The check then raises:

  AssertionError: DATAPLANE_UNIMPLEMENTED: the instance endpoint sim2real-behav-docdb-in-<id>.<account>.<region>.docdb.amazonaws.com:27017 advertised in DescribeDBInstances accepts no TCP connection from inside its own VPC (connect: OSError: a Lambda function in this emulator has no host network to reach); an endpoint that cannot be connected to is not a data plane

The host name, account id, region and port in that line are per-run/environment values, not requirements; the diverging behavior is the unconditional connect failure inside the function. The OSError message is emulator-authored text with no AWS counterpart — on AWS the call either connects or fails with a real network error (ETIMEDOUT/ECONNREFUSED) attributable to routing or security groups.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ec2 CreateVpc + two CreateSubnet calls in two AZs; CreateSecurityGroup for the probe and one for the cluster that admits TCP 27017 from the probe's group.
3. lambda CreateFunction(Runtime='python3.12', Handler='index.handler', Code={'ZipFile': <zip with a handler that does socket.create_connection((event['host'], event['port']), timeout=10)}, VpcConfig={SubnetIds: <the two subnets>, SecurityGroupIds: [<probe sg>]}); wait for State Active.
4. docdb CreateDBSubnetGroup over the subnets, CreateDBCluster(Engine='docdb', MasterUsername='dbadmin', ...), CreateDBInstance(DBInstanceClass='db.t3.medium', Engine='docdb'); poll DescribeDBInstances until DBInstanceStatus='available' and read Endpoint.Address/Endpoint.Port.
5. lambda Invoke the probe with that host/port. Observed: the handler's connect raises OSError('a Lambda function in this emulator has no host network to reach'). Expected: the socket connects and a TLS MongoDB session can authenticate and run insert/find.
6. Or replay docdb.cluster-mongo-insert-find-round-trip.
