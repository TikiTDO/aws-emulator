Issue-Source: trace-sweep-divergence
Issue-Trace: servicecatalog-provision-product-returns-template-outputs
Issue-Step: teardown#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicecatalog-provision-product-returns-template-outputs servicecatalog-provisioned-product-plan-and-record-not-found
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog|DescribeProvisionedProduct|4xx->2xx|$
Issue-Key: servicecatalog|DescribeProvisionedProduct|4xx->2xx|status
Issue-Key: servicecatalog|ProvisionProduct|2xx->2xx|$.RecordDetail.CreatedTime
Issue-Key: servicecatalog|ProvisionProduct|2xx->2xx|$.RecordDetail.UpdatedTime
Issue-Key: servicecatalog|DescribeProvisioningParameters|4xx->4xx|$.Message
Issue-Key: servicecatalog|DescribeProvisioningParameters|4xx->4xx|$.__type
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# Service Catalog provisioned-product lifecycle: 3 net-new causes in termination, path validation and record timestamps

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProvisionedProduct

## Description
Three net-new root causes in the Service Catalog emulator's provisioned-product lifecycle, all found replaying the wire corpus against this checkout. Each must be closed before this issue is submitted.

Cause 1 of 3 (HIGH) — a terminated provisioned product is never forgotten: DescribeProvisionedProduct keeps answering 200/AVAILABLE instead of 400 ResourceNotFoundException. Fixture servicecatalog-provision-product-returns-template-outputs, step teardown#13.
AWS: Smithy: TerminateProvisionedProduct(ProvisionedProductName|ProvisionedProductId, TerminateToken @idempotencyToken required) -> RecordDetail, errors ResourceNotFoundException (HTTP 400); RecordDetail$RecordType documents TERMINATE_PROVISIONED_PRODUCT as one of the three record types. Once the termination record settles, the provisioned product no longer exists, and DescribeProvisionedProduct (Smithy: errors InvalidParametersException | ResourceNotFoundException, both HTTP 400) refuses any later read of it with ResourceNotFoundException. The recorded step asserts the HTTP status (400) and the error code (ResourceNotFoundException) only; it does not assert the message, so the requirement here is the status and the code. AWS words this error class as a not-found naming the kind of resource and the identifier the request used; the identifier is the variable part, taken from the request.
Emulator: the teardown polls DescribeProvisionedProduct for the terminated product and every one of 37 attempts over the 180s cap answers HTTP 200 with ProvisionedProductDetail.Status 'AVAILABLE' (replay: "poll capped at 180s", "poll: error code None not in ['ResourceNotFoundException'] after 37 attempt(s)"). Termination is never carried out: services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/ops/terminate_provisioned_product.rs still returns Scaffolded::not_implemented (line 172), and nothing in src/state.rs ever removes an entry from `provisioned_products`, so no code path can make a provisioned product stop resolving.
Why: a client that terminates a provisioned product and waits for it to disappear never converges — it burns its whole poll budget and then fails. The leaked record also keeps the ProvisionedProductName taken, so a later ProvisionProduct of the same name is refused DuplicateResourceException, and a teardown/retry loop can never return the account to a clean state.
Fix: the TerminateProvisionedProduct handler plus the provisioned-product state (src/state.rs): record a TERMINATE_PROVISIONED_PRODUCT record, settle it on the same clock the other records use, and drop the provisioned product once it settles; DescribeProvisionedProduct's lookup must then answer ResourceNotFoundException (HTTP 400) for an Id or Name that no longer resolves. A separate stub issue covers TerminateProvisionedProduct answering 501 at all; this cause is the read-after-terminate divergence that remains once it answers.
Traces: servicecatalog-provision-product-returns-template-outputs#teardown#13.

Cause 2 of 3 (HIGH) — DescribeProvisioningParameters refuses a request that names no launch path with the wrong error class and an emulator sentence. Fixture servicecatalog-provisioned-product-plan-and-record-not-found, step task#3 (same cause at task#4).
AWS: recorded behavior: HTTP 400 with __type 'InvalidParametersException' and Message "Either Path Id or Path Name needs to be specified" — AWS validates the PathId/PathName pair BEFORE it resolves the product, so the refusal is a parameter error even when the request's ProductId names a product that does not exist. Smithy: DescribeProvisioningParameters errors InvalidParametersException | ResourceNotFoundException (both HTTP 400); member doc for PathId: "This value is optional if the product has a default path, and required if the product has more than one path." The message is all fixed text — it carries no values from the request or state.
Emulator: HTTP 400 with __type 'ResourceNotFoundException' and Message "No launch paths found for resource: <ProductId>" (replay actual: 'No launch paths found for resource: prod-zzzzzzzzzzzzz'); the product id is the variable part the emulator's own template fills, the rest is state.rs's `no_launch_paths_message` wording. The order is wrong: src/ops/describe_provisioning_parameters.rs `resolve()` resolves the product and its launch paths first (returning not-found) and only reaches the PathId/PathName check afterwards.
Why: a client branches on the error class — InvalidParametersException means "fix the request" (supply a path), ResourceNotFoundException means "create or wait for the resource". It takes the wrong branch, and any handling keyed on AWS's sentence never matches.
Fix: ops/describe_provisioning_parameters.rs — the PathId/PathName presence check must precede product/launch-path resolution and must refuse with InvalidParametersException carrying AWS's sentence. The shared refusal constant in src/state.rs (PATH_REQUIRED, "A path is required when the product has more than one path...") is emulator wording for the same error class and must be AWS's text too; ProvisionProduct resolves the same triple and must stay consistent.
Traces: servicecatalog-provisioned-product-plan-and-record-not-found#task#3, #task#4.

Cause 3 of 3 (LOW) — ProvisionProduct's RecordDetail.CreatedTime / UpdatedTime are not shaped as AWS's record timestamps. Fixture servicecatalog-provision-product-returns-template-outputs, step task#9.
AWS: Smithy: RecordDetail$CreatedTime -> CreatedTime (timestamp, "The UTC time stamp of the creation time"), RecordDetail$UpdatedTime -> UpdatedTime (timestamp, "The time when the record was last updated"); the service's protocol is awsJson1_1, so both are epoch-seconds on the wire. Beyond the type the model states nothing about resolution: recorded behavior only — the fixture shape-asserts both members, and the emulator's values do not satisfy the recorded shape, while other servicecatalog timestamp members asserted in the same corpus (e.g. ProductViewDetail.CreatedTime in servicecatalog-product-lifecycle) do.
Emulator: the 200 body carries RecordDetail.CreatedTime and RecordDetail.UpdatedTime as bare epoch-second numbers with a sub-millisecond fraction, both equal to the request instant (replay actual: 1794354982.005712 for both). Both come from a raw SystemTime::now(): src/state.rs Record::detail (lines ~768-774) wraps `created_time` and `updated_time(now)` with aws_smithy_types::DateTime::from, and `updated_time` returns `created_time` while the record is still in flight.
Hypothesis: AWS's record timestamps carry at most millisecond resolution, so an epoch value with a microsecond remainder is shaped differently from every recorded servicecatalog record timestamp.
Why: a client or deserializer that relies on the shape AWS serves for these members reads a differently shaped value, and assertions over a record's CreatedTime/UpdatedTime do not hold against the emulator even though the status and the rest of the record match.
Fix: the construction/serialization of RecordDetail's two timestamps in src/state.rs (Record::detail and Record::updated_time) — the instant a record is minted with must be produced at the resolution AWS reports, consistently for every reporter of the same record (ProvisionProduct, DescribeRecord, ListRecordHistory).
Traces: servicecatalog-provision-product-returns-template-outputs#task#9.

Related but distinct open issues (none of them covers the three causes above): 20261110T133549Z-servicecatalog-provisionproduct-never-fails-a-launch-a-parameter-value.md (no asynchronous FAILURE path for a launch), 20261104T032856Z-servicecatalog-describeproduct-not-found-access-denied-error-message-uses.md (DescribeProduct's not-found wording), 20261029T191224Z-servicecatalog-service-catalog-portfolio-and-product-operations-diverge-from.md (portfolio/product error messages and metadata).

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout (emulator up; /__admin/health 200): 128 steps, 99 passed, 26 mismatched, 3 blocked, score 77.3%. Seven divergence clusters, all service=servicecatalog; four are owned by open issues and are not re-filed here. The three below are net-new (no stub_path, no owned_by).

Cause 1 — fixture servicecatalog-provision-product-returns-template-outputs, step teardown#13, operation DescribeProvisionedProduct, verdict mismatch:
  expected status 400, actual 200
  "$: poll capped at 180s (expected None, got {'ProvisionedProductDetail': {... 'Status': 'AVAILABLE', ...}, 'CloudWatchDashboards': []})"
  "$: poll: error code None not in ['ResourceNotFoundException'] after 37 attempt(s)"
  Expected: 400 / ResourceNotFoundException once the provisioned product is terminated. Actual: 200 with ProvisionedProductDetail.Status 'AVAILABLE' forever (the generated name, ARN, ids and CreatedTime in the recorded body are per-run values, not requirements).
  Code: src/ops/terminate_provisioned_product.rs:172 returns Scaffolded::not_implemented("TerminateProvisionedProduct"); src/state.rs never removes from `provisioned_products`.

Cause 2 — fixture servicecatalog-provisioned-product-plan-and-record-not-found, steps task#3 and task#4, operation DescribeProvisioningParameters, verdict mismatch (both 400):
  "$.__type: value_mismatch (expected 'InvalidParametersException', got 'ResourceNotFoundException')"
  "$.Message: value_mismatch (expected 'Either Path Id or Path Name needs to be specified', got 'No launch paths found for resource: prod-zzzzzzzzzzzzz')"
  Expected error code InvalidParametersException with that exact sentence (all fixed text). Actual error code ResourceNotFoundException with the emulator's own template "No launch paths found for resource: {identifier}" filled with the request's ProductId.
  Code: src/ops/describe_provisioning_parameters.rs `resolve()` lines ~264-335 — product and launch-path resolution (returning not_found_resource) runs before the PathId/PathName check.

Cause 3 — fixture servicecatalog-provision-product-returns-template-outputs, step task#9, operation ProvisionProduct, verdict mismatch (both 200):
  "$.RecordDetail.CreatedTime: value_mismatch (expected '<shaped-not-asserted>', got 1794354982.005712)"
  "$.RecordDetail.UpdatedTime: value_mismatch (expected '<shaped-not-asserted>', got 1794354982.005712)"
  The recorded epoch value itself is a per-run value and no requirement; the shape of these two members is. Every other asserted member of the same RecordDetail matched, and ProductViewDetail.CreatedTime matched in servicecatalog-product-lifecycle, so the divergence is specific to the record timestamps.
  Code: src/state.rs Record::detail lines ~768-774 (DateTime::from(self.created_time) / DateTime::from(self.updated_time(now))), Record::updated_time lines ~755-764.

## Steps to Reproduce
Cause 1 (read-after-terminate):
1. CreatePortfolio, CreateProduct (CLOUD_FORMATION_TEMPLATE with a template URL), AssociateProductWithPortfolio, AssociatePrincipalWithPortfolio, ListLaunchPaths.
2. ProvisionProduct(ProductId, ProvisioningArtifactId, PathId, ProvisionedProductName=<new>, ProvisionToken=<uuid>) and wait for DescribeProvisionedProduct to report AVAILABLE.
3. TerminateProvisionedProduct(ProvisionedProductId=<pp-id>, TerminateToken=<uuid>).
4. Poll DescribeProvisionedProduct(Id=<pp-id>). AWS: the call starts failing 400 ResourceNotFoundException. Emulator: 200 with Status 'AVAILABLE' on every attempt until the poll budget (180s / 37 attempts) is exhausted.
5. ProvisionProduct again with the same ProvisionedProductName: emulator refuses DuplicateResourceException because the terminated product still occupies the name.

Cause 2 (path validation order):
1. Call DescribeProvisioningParameters(ProductId='prod-zzzzzzzzzzzzz', ProvisioningArtifactId=<any>) with neither PathId nor PathName.
2. AWS: 400 InvalidParametersException, Message "Either Path Id or Path Name needs to be specified". Emulator: 400 ResourceNotFoundException, Message "No launch paths found for resource: prod-zzzzzzzzzzzzz".
3. Repeat with a real ProductId that has several launch paths and no PathId/PathName: the emulator answers its own PATH_REQUIRED sentence instead of AWS's.

Cause 3 (record timestamps):
1. Run the provisioning flow of cause 1 up to and including ProvisionProduct.
2. Inspect the raw JSON response body: RecordDetail.CreatedTime and RecordDetail.UpdatedTime are epoch seconds carrying a microsecond fraction (e.g. ...982.005712) and are equal to each other.
3. Compare with the recorded AWS response for the same call — the shape of those two members differs. DescribeRecord on the same RecordId reports the same values, so the fix belongs where the record's timestamps are built, not in one handler.
