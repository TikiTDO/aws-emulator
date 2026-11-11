Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-account-and-instance-type-metadata
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-account-and-instance-type-metadata ec2-flowlog-idformat-iaminstanceprofile-describes ec2-gateway-vpc-endpoint-lifecycle ec2-instance-event-window-and-application-status-check-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeRegions|2xx->2xx|$.DescribeRegionsResponse.regionInfo.item.regionEndpoint
Issue-Key: ec2|DescribeIdentityIdFormat|2xx->2xx|$.DescribeIdentityIdFormatResponse.statusSet.item.deadline
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.baseEndpointDnsNameSet.item[*]
Issue-Key: ec2|CreateApplicationStatusCheck|2xx->2xx|$.CreateApplicationStatusCheckResponse.applicationStatusCheck.applicationStatusCheckId
Issue-Key: ec2|CreateApplicationStatusCheck|2xx->2xx|$.CreateApplicationStatusCheckResponse.applicationStatusCheck.protocol
Issue-Key: ec2|DeleteApplicationStatusCheck|2xx->2xx|$.DeleteApplicationStatusCheckResponse.applicationStatusCheck.protocol
Issue-Related: 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md
Issue-Related: 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md
Issue-Related: 20261104T022117Z-ec2-success-responses-omit-members-aws-returns-for.md
Issue-Related: 20261104T130156Z-ec2-describevpcendpointservices-merges-gateway-and-interface-service-details.md
Issue-Related: 20261108T143229Z-ec2-static-describe-catalogs-serve-synthesized-hardware-and.md
Issue-Related: 20261109T225653Z-ec2-vpc-endpoint-plane-missing-endpoint-policy-refusal.md
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# EC2 describe and create responses omit modeled members and advertise hosts the emulator does not serve (5 causes)

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRegions

## Description
Five 200-status response projections drop members AWS always returns, mis-case an enum value, mint ids in a non-AWS shape, and advertise real AWS hostnames the emulator does not serve. 5 causes.

Cause 1 of 5: DescribeRegions drops geographySet and advertises a real AWS endpoint host (fixture ec2-account-and-instance-type-metadata, step task#2).
AWS: Smithy Region has members Geography (xmlName geographySet, target RegionGeographyList, doc "The geography information for the Region. The geography is returned as a list.") and Endpoint (xmlName regionEndpoint, doc "The Region service endpoint."), so each regionInfo.item carries a geographySet holding the region's geography entry (for a US region, item.name "United States of America") and a regionEndpoint naming the endpoint the client should use for that region.
Emulator: geographySet is absent from every regionInfo.item, and regionEndpoint is rendered as the real AWS host for the region, which the replay flags host_not_own - the emulator announces an address other than the one it is reached on.
Why: a client enumerating regions sees no geography at all, and any client that follows the advertised regionEndpoint leaves the emulator and dials real AWS (or fails to resolve it), so a multi-region flow silently escapes the emulated estate.
Fix: the DescribeRegions handler's region catalog (it must carry each region's geography) and its serializer, which must render regionEndpoint from the emulator's own served host rather than a literal AWS hostname.
Traces: ec2-account-and-instance-type-metadata#task#2. Related open issue that does NOT carry this step's replay key: "EC2 DescribeRegions serves the wrong region catalog and omits geographySet" (20261103T123451Z).

Cause 2 of 5: DescribeIdentityIdFormat omits the deadline member (fixture ec2-flowlog-idformat-iaminstanceprofile-describes, step task#5).
AWS: Smithy IdFormat.Deadline (xmlName deadline, target DateTime) - "The date in UTC at which you are permanently switched over to using longer IDs. If a deadline is not yet available for this resource type, this field is not returned." The recording shows AWS returning a deadline on the statusSet item for the queried resource, so a deadline IS available for it.
Emulator: statusSet.item.deadline is absent.
Why: a client reading the long-id migration deadline for a resource type concludes no deadline exists, which is how AWS signals the opposite case.
Fix: the DescribeIdentityIdFormat handler's id-format state/serializer, which must carry and emit the per-resource deadline timestamp.
Traces: ec2-flowlog-idformat-iaminstanceprofile-describes#task#5.

Cause 3 of 5: DescribeVpcEndpointServices serves a synthesized endpoint-service row (fixture ec2-gateway-vpc-endpoint-lifecycle, step task#3).
AWS: Smithy ServiceDetail models ServiceType as a list (xmlName serviceType) whose entries are the types the service actually offers - for the recorded S3 gateway endpoint service exactly one entry, serviceType 'Gateway'; SupportedIpAddressTypes (xmlName supportedIpAddressTypeSet, @length max 2) holds both ipv4 and ipv6; ServiceId is an endpoint-service id in AWS's vpce-svc-<hex> form; BaseEndpointDnsNames/PrivateDnsName/PrivateDnsNames are the DNS names a consumer resolves for the service.
Emulator: serviceType carries two entries ([{Gateway},{Interface}]) for a gateway-only service; supportedIpAddressTypeSet omits its second entry 'ipv6'; serviceId is minted in a shape that does not match AWS's endpoint-service id form; and baseEndpointDnsNameSet, privateDnsName and privateDnsNameSet.privateDnsName all announce real AWS hostnames that the emulator does not serve (flagged host_not_own).
Why: a client filtering endpoint services by type sees S3 offered as both Gateway and Interface and can create the wrong endpoint type; an IPv6 client sees the service as IPv4-only; and anything that resolves the advertised DNS names leaves the emulator for real AWS.
Fix: the endpoint-service catalog behind DescribeVpcEndpointServices (per-service type list and supported IP address types) and the id/DNS-name minting in its serializer, which must name the emulator's own host.
Traces: ec2-gateway-vpc-endpoint-lifecycle#task#3. Related open issues that do NOT carry this step's replay key: "EC2 static describe catalogs serve synthesized hardware and endpoint-service data" (20261108T143229Z), "ec2 DescribeInstanceTypes pages c9gd-m6gd and r9g-z1d plus DescribeVpcEndpointServices serve derived catalog rows" (20261109T233303Z).

Cause 4 of 5: CreateApplicationStatusCheck omits the service-applied defaults and lower-cases protocol (fixture ec2-instance-event-window-and-application-status-check-lifecycle, step task#5).
AWS: Smithy ApplicationStatusCheckResponseObject models deviceIndex (Integer, "The index of the network device used for the health check. The value is greater than or equal to 0."), ipVersion (IpVersionEnum: ipv4|ipv6), ipScope (IpScopeEnum: private) and protocol (NetworkProtocolEnum). The recording shows AWS filling the omitted request members with its defaults in the create response - deviceIndex 0, ipScope 'private', ipVersion 'ipv4' - and returning protocol upper-cased as 'HTTP' (the model's NetworkProtocolEnum lists http/https, so the upper-case response value is recorded behavior only). applicationStatusCheckId is minted in AWS's asc-<hex> id form.
Emulator: deviceIndex, ipScope and ipVersion are absent from the created object, protocol is returned as 'http', and applicationStatusCheckId is minted as a zero-padded decimal sequence that does not match AWS's id shape.
Why: a client reading back the check it just created cannot tell which device, IP scope or IP version it monitors, and any case-sensitive comparison of protocol against AWS's value fails; id-shape validation on the returned id fails too.
Fix: the CreateApplicationStatusCheck handler (apply and persist the service defaults for deviceIndex/ipScope/ipVersion), the ApplicationStatusCheck serializer (emit those members, render protocol as AWS does) and the id minting for application-status-check ids.
Traces: ec2-instance-event-window-and-application-status-check-lifecycle#task#5.

Cause 5 of 5: DeleteApplicationStatusCheck echoes the same incomplete object (same fixture, step task#6).
AWS: the delete response returns the same ApplicationStatusCheckResponseObject projection, so deviceIndex 0, ipScope 'private', ipVersion 'ipv4' and protocol 'HTTP' appear there too.
Emulator: deviceIndex, ipScope and ipVersion are absent and protocol is 'http' - the stored record never held the defaults, so the delete path repeats the create path's gap.
Why: a client that confirms a deletion by inspecting the returned object sees a different resource than AWS returns.
Fix: same component as cause 4 - the shared ApplicationStatusCheck state/serializer read by the DeleteApplicationStatusCheck handler.
Traces: ec2-instance-event-window-and-application-status-check-lifecycle#task#6.

## Evidence
Cause 1 - fixture ec2-account-and-instance-type-metadata step task#2, DescribeRegions, expected 200 / actual 200:
  $.DescribeRegionsResponse.regionInfo.item.geographySet: missing (expected {'item': {'name': 'United States of America'}}, got None)
  $.DescribeRegionsResponse.regionInfo.item.regionEndpoint: host_not_own (emulator returned a real AWS endpoint host it does not serve)
  Smithy: Region.Geography -> xmlName geographySet (RegionGeographyList); Region.Endpoint -> xmlName regionEndpoint.

Cause 2 - fixture ec2-flowlog-idformat-iaminstanceprofile-describes step task#5, DescribeIdentityIdFormat, expected 200 / actual 200:
  $.DescribeIdentityIdFormatResponse.statusSet.item.deadline: missing (expected a timestamp, got None)
  Smithy: IdFormat.Deadline -> xmlName deadline, DateTime.

Cause 3 - fixture ec2-gateway-vpc-endpoint-lifecycle step task#3, DescribeVpcEndpointServices, expected 200 / actual 200:
  serviceDetailSet.item.serviceType.item: type_mismatch (expected {'serviceType': 'Gateway'}, got [{'serviceType': 'Gateway'}, {'serviceType': 'Interface'}])
  serviceDetailSet.item.supportedIpAddressTypeSet.item[1]: missing (expected 'ipv6', got None)
  serviceDetailSet.item.serviceId: value_mismatch against the expected id shape (got 'vpce-svc-fffbb61c')
  serviceDetailSet.item.baseEndpointDnsNameSet.item[0]/[1], privateDnsName, privateDnsNameSet.item.privateDnsName: host_not_own (emulator returned real AWS S3 endpoint hostnames)
  Smithy: ServiceDetail.ServiceType (list), SupportedIpAddressTypes (@length max 2), ServiceId, BaseEndpointDnsNames, PrivateDnsName(s).

Cause 4 - same corpus, fixture ec2-instance-event-window-and-application-status-check-lifecycle step task#5, CreateApplicationStatusCheck, expected 200 / actual 200:
  applicationStatusCheck.deviceIndex: missing (expected '0', got None)
  applicationStatusCheck.ipScope: missing (expected 'private', got None)
  applicationStatusCheck.ipVersion: missing (expected 'ipv4', got None)
  applicationStatusCheck.protocol: value_mismatch (expected 'HTTP', got 'http')
  applicationStatusCheck.applicationStatusCheckId: value_mismatch against the expected id shape (got 'asc-00000000000000081')
  Smithy: ApplicationStatusCheckResponseObject.{DeviceIndex,IpScope,IpVersion,Protocol}; IpScopeEnum ['private']; IpVersionEnum ['ipv4','ipv6']; NetworkProtocolEnum ['http','https'] (recorded AWS response value 'HTTP').

Cause 5 - same fixture step task#6, DeleteApplicationStatusCheck, expected 200 / actual 200:
  applicationStatusCheck.deviceIndex missing (expected '0'); ipScope missing (expected 'private'); ipVersion missing (expected 'ipv4'); protocol expected 'HTTP', got 'http'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Cause 1: replay fixture ec2-account-and-instance-type-metadata step task#2 (DescribeRegions) - regionInfo.item has no geographySet and regionEndpoint names a real AWS host rather than the emulator's own.
3. Cause 2: replay fixture ec2-flowlog-idformat-iaminstanceprofile-describes step task#5 (DescribeIdentityIdFormat) - statusSet.item carries no deadline.
4. Cause 3: replay fixture ec2-gateway-vpc-endpoint-lifecycle step task#3 (DescribeVpcEndpointServices for the S3 gateway service) - serviceType lists both Gateway and Interface, supportedIpAddressTypeSet lacks ipv6, and the DNS name members advertise real AWS hostnames.
5. Causes 4 and 5: replay fixture ec2-instance-event-window-and-application-status-check-lifecycle steps task#5 and task#6 (Create/DeleteApplicationStatusCheck) - deviceIndex, ipScope and ipVersion are absent and protocol is 'http' instead of 'HTTP'.
