Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-create-namespace-input-validation
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-create-namespace-input-validation servicediscovery-dns-namespace-request-validation-errors servicediscovery-operation-polling-and-filtered-listing servicediscovery-resource-tagging-lifecycle-and-limits servicediscovery-service-attributes-crud-and-limits
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|CreatePublicDnsNamespace|4xx->4xx|$.Message
Issue-Key: servicediscovery|ListNamespaces|4xx->4xx|$.Message
Issue-Key: servicediscovery|TagResource|4xx->4xx|$.Message
Issue-Key: servicediscovery|UpdateServiceAttributes|4xx->4xx|$.Message
Issue-Related: 20261029T203804Z-servicediscovery-cloud-map-namespace-operation-and-tagging-errors-answer.md
Issue-Related: 20261029T203804Z-servicediscovery-cloud-map-service-instance-and-service-attribute-errors.md
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Cloud Map 400 responses carry wrong or absent InvalidInput message bodies

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePublicDnsNamespace

## Description
SIX independent, net-new wire divergences in the servicediscovery (Cloud Map) emulator where the HTTP status (400) and the error code are right but the Message body is wrong - twice it is MISSING entirely (the framework constraint-violation message AWS returns), four times it is emulator-authored prose instead of AWS's template. An error message is a requirement, so each of the six causes below must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Cause 1 of 6: CreatePublicDnsNamespace rejects an invalid Name with NO Message at all, where AWS returns the standard constraint-violation text.
AWS: Smithy CreatePublicDnsNamespace{Name (required), CreatorRequestId, Description, Tags, Properties} -> OperationId; errors InvalidInput (400), NamespaceAlreadyExists, ResourceLimitExceeded, DuplicateRequest, TooManyTagsException. Recorded: HTTP 400 InvalidInput with the framework validation message "<n> validation error detected: Value '<rejected value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Fixed text: '<n> validation error detected: Value ', " at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the error count, the rejected value taken from the request, the lower-camel member name ('name'), and the pattern Cloud Map enforces for a public DNS namespace name (recorded as ^([a-zA-Z0-9]([a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z0-9]([a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])?$).
Emulator: HTTP 400 with NO Message member in the body (the rejection itself is correct).
Why: every client surface - botocore's exception str, logs, user-facing errors - shows an empty reason, so a caller cannot tell WHICH member or constraint failed.
Fix: the input-validation layer that produces InvalidInput for pattern violations (the shared constraint-violation message builder used by the namespace create handlers).
Traces: servicediscovery-create-namespace-input-validation#task#3; same cluster also covers servicediscovery-dns-namespace-request-validation-errors#task#3.

Cause 2 of 6: UpdatePublicDnsNamespace applied to an HTTP namespace returns the wrong InvalidInput text.
AWS: Smithy UpdatePublicDnsNamespace{Id (required), UpdaterRequestId, Namespace: PublicDnsNamespaceChange (required)} -> OperationId; errors DuplicateRequest, InvalidInput (400), NamespaceNotFound, ResourceInUse. Recorded: HTTP 400 InvalidInput, Message 'Namespace with incorrect type HTTP provided. Please provide namespace with type DNS_PUBLIC.' Fixed text: 'Namespace with incorrect type ', ' provided. Please provide namespace with type ', '.'; variable parts: the stored namespace's actual Type and the type the called operation requires (DNS_PUBLIC here, DNS_PRIVATE for the private variant).
Emulator: HTTP 400 InvalidInput with "The namespace <namespaceId> is a HTTP namespace, so it can't be updated with UpdatePublicDnsNamespace." - a different template whose variable parts are the namespace id and the operation name.
Why: a client that parses the message to learn the actual and required namespace types gets neither; assertions on AWS's wording fail.
Fix: the type-mismatch InvalidInput construction in the UpdatePublicDnsNamespace / UpdatePrivateDnsNamespace handlers (shared helper).
Traces: servicediscovery-dns-namespace-request-validation-errors#task#7.

Cause 3 of 6: ListNamespaces' unsupported-filter-condition rejection uses the wrong template for the TYPE filter.
AWS: Smithy ListNamespaces{Filters: NamespaceFilters of NamespaceFilter{Name (required, enum TYPE|NAME|HTTP_NAME|RESOURCE_OWNER), Values (required), Condition: FilterCondition}} -> Namespaces; error InvalidInput (400). Recorded: HTTP 400 InvalidInput, Message 'Invalid filter condition(s) provided: filter TYPE supports condition(s) [BEGINS_WITH, EQ], but got BETWEEN; '. Fixed text: 'Invalid filter condition(s) provided: filter ', ' supports condition(s) [', '], but got ', '; ' (note the trailing semicolon-space, which is part of the template because the message accumulates one clause per offending filter); variable parts: the filter Name from the request, the supported condition list for that filter, and the rejected Condition from the request.
Emulator: HTTP 400 InvalidInput with "Filters.Condition BETWEEN isn't supported for a namespace filter. Valid values are EQ and BEGINS_WITH." - a different template that never names the filter.
Why: the message is the only place AWS reports WHICH filter was bad (it can list several), so multi-filter requests become undiagnosable. The open behavioral issue 20261023T124702Z-servicediscovery-cloud-map-servicediscovery-8-fidelity-divergences-missing-dnsconfig.md names this template for the NAME filter in its Cause 8; this wire step exercises the TYPE filter and is a separate replay key.
Fix: ListNamespaces' filter-condition validation message builder (one clause per rejected filter).
Traces: servicediscovery-operation-polling-and-filtered-listing#task#8.

Cause 4 of 6: TagResource's tag-limit rejection uses the wrong message.
AWS: Smithy TagResource{ResourceARN (required), Tags (required, TagList length 0..200)} -> {}; errors InvalidInput (400), ResourceNotFoundException, TooManyTagsException (400). Recorded: HTTP 400 with Message 'Allowable tag limit for resource exceeded' - the whole message is FIXED text, with no quota number, resource or key list in it.
Emulator: HTTP 400 with 'The list of tags on the resource is over the quota of 50.' - emulator prose that embeds the limit.
Why: tests and clients that match AWS's fixed text fail; the limit value is not part of AWS's message.
Fix: the tag-limit error construction in TagResource's handler (the shared tag-limit message constant).
Traces: servicediscovery-resource-tagging-lifecycle-and-limits#task#13.

Cause 5 of 6: UpdateService on a service in an HTTP (API-only) namespace returns the wrong InvalidInput text.
AWS: Smithy UpdateService{Id (required), Service: ServiceChange (required)} -> OperationId; errors DuplicateRequest, InvalidInput (400), ServiceNotFound. Recorded: HTTP 400 InvalidInput, Message 'Service in API-only namespace cannot be updated.' - entirely fixed text, naming neither the service nor the operation.
Emulator: HTTP 400 InvalidInput with "The service <serviceId> is associated with an HTTP namespace, so its settings can't be updated with UpdateService."
Why: a client asserting the documented refusal text diverges; the emulator's variable service id is not part of AWS's message.
Fix: the API-only-namespace refusal in UpdateService's handler.
Traces: servicediscovery-service-attributes-crud-and-limits#task#4.

Cause 6 of 6: UpdateServiceAttributes rejects an over-length Attributes map with NO Message, where AWS returns the constraint-violation text naming the map and its limit.
AWS: Smithy UpdateServiceAttributes{ServiceId (required), Attributes: ServiceAttributesMap (required, length min 1 max 30)} -> {}; errors InvalidInput (400), ServiceAttributesLimitExceededException (400), ServiceNotFound. Recorded: HTTP 400 with "<n> validation error detected: Value '<the submitted map rendered as {k=v, ...}>' at 'attributes' failed to satisfy constraint: Member must have length less than or equal to 30". Fixed text: '<n> validation error detected: Value ', " at 'attributes' failed to satisfy constraint: Member must have length less than or equal to ", and the limit 30 from the model; variable parts: the error count and the rendered request map.
Emulator: HTTP 400 with NO Message member (the rejection itself is correct).
Why: as cause 1 - the caller is told nothing about which member or limit was violated.
Fix: the same shared constraint-violation message builder, applied to map @length violations in UpdateServiceAttributes' validation.
Traces: servicediscovery-service-attributes-crud-and-limits#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 168 steps, 89 passed, 79 mismatched, score 53.0%. All six clusters are status-matched 400 responses whose Message diverges (expected = recorded AWS response):

Cause 1 - servicediscovery-create-namespace-input-validation#task#3, CreatePublicDnsNamespace: '$.Message: missing (expected "1 validation error detected: Value \'<the rejected name>\' at \'name\' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-zA-Z0-9]([a-zA-Z0-9\\-]{0,61}[a-zA-Z0-9])?\\.)+[a-zA-Z0-9]([a-zA-Z0-9\\-]{0,61}[a-zA-Z0-9])?$", got None)'. 2 steps affected.
Cause 2 - servicediscovery-dns-namespace-request-validation-errors#task#7, UpdatePublicDnsNamespace: '$.Message: value_mismatch (expected \'Namespace with incorrect type HTTP provided. Please provide namespace with type DNS_PUBLIC.\', got "The namespace <namespaceId> is a HTTP namespace, so it can\'t be updated with UpdatePublicDnsNamespace.")'.
Cause 3 - servicediscovery-operation-polling-and-filtered-listing#task#8, ListNamespaces: '$.Message: value_mismatch (expected \'Invalid filter condition(s) provided: filter TYPE supports condition(s) [BEGINS_WITH, EQ], but got BETWEEN; \', got "Filters.Condition BETWEEN isn\'t supported for a namespace filter. Valid values are EQ and BEGINS_WITH.")'.
Cause 4 - servicediscovery-resource-tagging-lifecycle-and-limits#task#13, TagResource: '$.Message: value_mismatch (expected \'Allowable tag limit for resource exceeded\', got \'The list of tags on the resource is over the quota of 50.\')'.
Cause 5 - servicediscovery-service-attributes-crud-and-limits#task#4, UpdateService: '$.Message: value_mismatch (expected \'Service in API-only namespace cannot be updated.\', got "The service <serviceId> is associated with an HTTP namespace, so its settings can\'t be updated with UpdateService.")'.
Cause 6 - servicediscovery-service-attributes-crud-and-limits#task#10, UpdateServiceAttributes: '$.Message: missing (expected "1 validation error detected: Value \'{key1=v, key2=v, ...}\' at \'attributes\' failed to satisfy constraint: Member must have length less than or equal to 30", got None)' (the request submitted 35 attributes).

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixtures/steps in Issue-Trace/Issue-Step and Issue-Trace-Ids, or with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)) and read e.response['Error']['Message']:
1. Cause 1: sd.create_public_dns_namespace(Name='bad public ns-<suffix>') (a name with spaces, failing the DNS-name pattern) -> emulator 400 with no Message; AWS 400 InvalidInput with the '1 validation error detected: ... at \'name\' ... regular expression pattern: ...' text.
2. Cause 2: create an HTTP namespace (poll to SUCCESS) and call sd.update_public_dns_namespace(Id=ns, Namespace={'Description':'x'}) -> compare the Message with 'Namespace with incorrect type HTTP provided. Please provide namespace with type DNS_PUBLIC.'
3. Cause 3: sd.list_namespaces(Filters=[{'Name':'TYPE','Values':['HTTP','DNS_PUBLIC'],'Condition':'BETWEEN'}]) -> compare the Message with 'Invalid filter condition(s) provided: filter TYPE supports condition(s) [BEGINS_WITH, EQ], but got BETWEEN; '.
4. Cause 4: tag a namespace or service up past the 50-tag limit with sd.tag_resource(ResourceARN=<arn>, Tags=[...]) -> compare the Message with 'Allowable tag limit for resource exceeded'.
5. Cause 5: create a service in an HTTP namespace and call sd.update_service(Id=svc, Service={'Description':'x','DnsConfig':{'DnsRecords':[{'Type':'A','TTL':60}]}}) -> compare the Message with 'Service in API-only namespace cannot be updated.'
6. Cause 6: sd.update_service_attributes(ServiceId=svc, Attributes={f'key{i}':'v' for i in range(35)}) -> emulator 400 with no Message; AWS 400 with the 'validation error detected ... at \'attributes\' ... length less than or equal to 30' text.
