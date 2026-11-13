# Local image publication

The ECR endpoint serves a Docker Registry V2 data plane beside the JSON API.
Use the endpoint/repository URI returned by this emulator, and the credential
issued by its `GetAuthorizationToken`. The registry stores and serves actual
manifest, configuration and layer bytes; an image in a build daemon's cache is
not publication to this registry.

Supported publication operations:

- `POST /v2/<repository>/blobs/uploads/` starts an upload (202). A `digest`
  query and body can complete a single-request upload (201).
- `PATCH .../uploads/<id>` appends an ordered chunk (202).
- `PUT .../uploads/<id>?digest=sha256:...` validates and commits all bytes,
  including any final body (201). A mismatch preserves the prior prefix.
- `GET .../uploads/<id>` reports the offset (204); `DELETE` cancels (204).
- `PUT /v2/<repository>/manifests/<tag-or-digest>` publishes a Docker schema2
  or OCI image/index manifest (201). Referenced content must already exist.
  Native retries of identical content succeed; immutable tags cannot move.

GET/HEAD manifests and blobs use the same store as `BatchGetImage` and
`DescribeImages`. Pulls can address a digest independently of mutable tags.
Cross-repository mount requests may fall back to a regular upload. Repository
creation remains a control-plane operation. Upload sessions are tied to their
repository and authenticated principal; credentials and current policies are
checked before mutation.

## Deliberate limits

This is not complete ECR/OCI conformance. In particular, **native manifest pushes
do not publish EventBridge image-action notifications**: the native routing hook
is synchronous. JSON `PutImage` retains its existing EventBridge behavior.
No event delivery is fabricated. The shared in-memory bounds remain 128MiB per
layer, 512MiB aggregate content/uploads/manifests and 1000 concurrent uploads;
past capacity is an explicit refusal, not metadata-only success. State is not a
persistent external registry and does not survive emulator process loss.

Registry addressing, host reachability and TLS trust must be configured for the
actual consuming machine. A loopback endpoint or the publisher's local image
cache is not evidence another computer can pull. No remote host is contacted or
host process launched by this data plane.

The ignored `registry_v2_push_real_docker_push_then_cacheless_pull` witness is
explicitly opt-in (`EMULATOR_ECR_DOCKER_TEST=1`). It uses a tiny generic imported
tar, a temporary Docker config and a fixed local socket; it pushes, verifies
bytes independently over HTTP, removes its exact local tags, then pulls by
digest. It does not prune unrelated images or prove a large application image
fits the in-memory bounds. Ordinary tests never execute Docker.

### Production HTTP body ceiling

The production HTTPS server separately buffers at most **16MiB per request body**
(`vendor/server/src/lib.rs`, `MAX_PRODUCTION_BODY_BYTES`). A larger body is
refused with HTTP 413 before service dispatch; sending it with HTTP chunked
transfer encoding does not bypass that total-body limit. The 128MiB layer bound
above is a stored/upload-session bound, not a promise that one HTTP request can
carry a layer that large. A client must split larger content into separate
Registry V2 PATCH requests within the server ceiling; this service cannot force
an unmodified Docker publisher to do so. Actual Docker/CDK request sizes and
application layer sizes still require validation before claiming application fit.

The ephemeral HTTP test server used by the ignored witness does not apply this
production-server body ceiling. A tiny witness passing there is not production
large-layer/streaming proof. The server's extra-route seam also receives already
buffered requests; it is not an existing streaming bypass for registry uploads.
