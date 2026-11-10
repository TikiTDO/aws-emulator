Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive lambda__s3.layer-content-survives-deletion-of-its-source-object lambda__s3.layer-location-downloads-the-published-archive
Issue-Key: lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive|_fetch#12|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: lambda__s3.layer-content-survives-deletion-of-its-source-object|_fetch#15|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: lambda__s3.layer-location-downloads-the-published-archive|_fetch#9|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# GetLayerVersion publishes Content.Location on an awslambda-*-layers S3 host the emulator does not serve

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLayerVersion

## Description
Cause 1 of 1: the layer archive link a layer version publishes is a real AWS S3 hostname, so the archive can never be downloaded from the emulator.

AWS: Smithy LayerVersionContentOutput.Location is documented as "A link to the layer archive in Amazon S3 that is valid for 10 minutes" — a URL the caller can GET to retrieve the published archive bytes. The behavioral requirement is that the announced address is one the publishing service answers on; the archive must still be downloadable from a layer version whose source S3 object has since been deleted, and the newest version's location must serve the newest archive.

Emulator: publishes `https://awslambda-<region>-layers.s3.<region>.amazonaws.com/snapshots/<account>/<layer>-<version>`, a host it does not serve and which does not resolve to the emulator; the emulator is reachable only on its own host and port. Every attempt to fetch the layer archive therefore never reaches it.

Why: a client that downloads a layer archive — to verify its contents, to rebuild from a published version, or to copy a layer between accounts — cannot fetch anything, and the documented "survives deletion of its source object" guarantee is unobservable.

Fix: the GetLayerVersion / GetLayerVersionByArn content-location serializer — presign against the emulator's own host and port for the object store it actually serves, keeping AWS's URL shape.

Traces: lambda__s3.layer-cli-latest-version-location-query-downloads-the-newest-archive, lambda__s3.layer-content-survives-deletion-of-its-source-object, lambda__s3.layer-location-downloads-the-published-archive.

## Evidence
All three failing checks die on the published location, before any archive bytes are read:
simparity.published.PublishedUrlRejected: endpoint_url was given https://awslambda-<region>-layers.s3.<region>.amazonaws.com/snapshots/<account>/<layer>-<n> (published by the emulator): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<own host>:<port>/<resource path>
(layer names, version numbers and the emulator's port are per-run values; the defect is announcing a host other than the one the emulator is reached on.)

## Steps to Reproduce
1. PublishLayerVersion with Content from an S3 object (and publish a second version).
2. Call GetLayerVersion (or GetLayerVersionByArn) and read Content.Location.
3. GET that URL — the host does not resolve to the emulator, so the archive is never retrieved.
