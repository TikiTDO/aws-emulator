Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-connector-lifecycle-missing-prerequisites
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-connector-lifecycle-missing-prerequisites
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|CreateConnector|4xx->2xx|$.Message
Issue-Key: pca-connector-ad|CreateConnector|4xx->2xx|$.ResourceId
Issue-Key: pca-connector-ad|CreateConnector|4xx->2xx|$.ResourceType
Issue-Key: pca-connector-ad|CreateConnector|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: pca-connector-ad|CreateConnector|4xx->2xx|status
Issue-Key: pca-connector-ad|ListConnectors|2xx->2xx|$.Connectors[*]
Issue-Commit: c2fc8676d63317349fbaa2ea39a789b7f43b5131

# CreateConnector skips the DirectoryRegistration prerequisite and persists a connector AWS never creates

> _Note: this issue was identified against the code base at commit `c2fc8676d63317349fbaa2ea39a789b7f43b5131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Two net-new wire causes with one root: CreateConnector never resolves its DirectoryId against the directory-registration state, so a request AWS refuses is accepted, and the phantom connector it mints then leaks into the read path.

Cause 1 of 2: CreateConnector accepts a pattern-valid DirectoryId that has no DirectoryRegistration.
AWS: refuses with HTTP 404, code ResourceNotFoundException (Smithy: CreateConnector models ResourceNotFoundException @httpError 404, doc "The operation tried to access a nonexistent resource..."; its members Message, ResourceId and ResourceType are all @required, ResourceType documented as one of Connector, Template, TemplateGroupAccessControlEntry, ServicePrincipalName, DirectoryRegistration), and creates nothing. The error body is `Message` = `DirectoryRegistration was not found: <DirectoryId exactly as supplied>.` (fixed text `DirectoryRegistration was not found: ` plus the request's identifier plus a trailing period), `ResourceId` = the DirectoryId from the request, `ResourceType` = `DirectoryRegistration`, and the x-amzn-errortype header names ResourceNotFoundException. The message text is recorded behavior only; the status, the error class and the required members are the model's.
Emulator: answers the modeled success status HTTP 202 with a freshly minted ConnectorArn, and persists a ConnectorRecord. No Message/ResourceId/ResourceType and no x-amzn-errortype are emitted because no error is raised at all.
Why: a client whose error handling is written against the 404 never sees it, and the emulator mutates state where AWS mutates none — so a test that asserts "a connector cannot be created for an unregistered directory" passes against the emulator and fails against AWS.
Fix: CreateConnector's handler (services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_connector.rs) must resolve DirectoryId against the directory-registration store before minting the connector and return the modeled ResourceNotFoundException with all three required members populated.
Traces: fixture pca-connector-ad-connector-lifecycle-missing-prerequisites, step task#1.

Cause 2 of 2: the connector created in Cause 1 is then served by ListConnectors.
AWS: because CreateConnector refused, ListConnectors for that account/region returns a `Connectors` list with no such entry (HTTP 200; Smithy: ListConnectors GET /connectors, output Connectors -> ConnectorList, paginated).
Emulator: HTTP 200 whose Connectors list contains the phantom connector, carrying Arn, CertificateAuthorityArn, CertificateEnrollmentPolicyServerEndpoint, DirectoryId (the unregistered one), VpcInformation, Status CREATING and CreatedAt/UpdatedAt.
Why: the read path reports an existent connector for a directory that was never registered, so any caller that enumerates connectors to drive subsequent calls (templates, service principal names, teardown) operates on a resource AWS does not have.
Fix: same handler as Cause 1 — once the prerequisite gate refuses, nothing is inserted into the connector store (src/state.rs), and this read path agrees automatically.
Traces: fixture pca-connector-ad-connector-lifecycle-missing-prerequisites, step task#2.

Related open issue (does NOT cover these wire steps): 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md, Cause 3, reports the same acceptance from the behavioral corpus; it carries no wire replay key and does not mention the ListConnectors leak.

## Evidence
Cause 1 — pca-connector-ad-connector-lifecycle-missing-prerequisites#task#1 (CreateConnector against an unregistered DirectoryId):
  status: expected 404, got 202
  $.Message: expected 'DirectoryRegistration was not found: <DirectoryId>.' (recorded: 'DirectoryRegistration was not found: d-9067abcd12.'), got None
  $.ResourceId: expected the request's DirectoryId (recorded 'd-9067abcd12'), got None
  $.ResourceType: expected 'DirectoryRegistration', got None
  $headers.x-amzn-errortype: expected ResourceNotFoundException (shaped, not asserted verbatim), got None

Cause 2 — …#task#2 (ListConnectors):
  status 200 == 200, but $.Connectors[0] is EXTRA: expected no entry, got a connector record {Arn: arn:aws:pca-connector-ad:<region>:<account>:connector/<uuid>, CertificateAuthorityArn: <the request's CA ARN>, CertificateEnrollmentPolicyServerEndpoint: https://<uuid>.pca-connector-ad.<region>.amazonaws.com/ADPolicyProvider_CEP_Kerberos/service.svc/CEP, DirectoryId: <the unregistered directory id>, VpcInformation: {SecurityGroupIds: [<sg from the request>]}, Status: 'CREATING', CreatedAt/UpdatedAt: <creation time>} — i.e. exactly the resource Cause 1 should not have created.

## Steps to Reproduce
Boot the emulator and replay simparity-wire-aws-v1, or drive it directly (region us-west-2):
1. Without registering any directory, call CreateConnector(DirectoryId=<a pattern-valid id matching ^d-[0-9a-f]{10}$ that has no DirectoryRegistration>, CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={'SecurityGroupIds': ['sg-0123456789abcdef0']}). The emulator answers 202 with a ConnectorArn; AWS answers 404 ResourceNotFoundException with Message 'DirectoryRegistration was not found: <that DirectoryId>.', ResourceId=<that DirectoryId>, ResourceType='DirectoryRegistration'. (pca-connector-ad-connector-lifecycle-missing-prerequisites#task#1)
2. Call ListConnectors(). The emulator's Connectors list contains the connector from step 1; on AWS the list has no such entry because step 1 created nothing. (…#task#2)
