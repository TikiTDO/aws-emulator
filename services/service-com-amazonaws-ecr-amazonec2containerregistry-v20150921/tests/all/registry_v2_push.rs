// SPDX-License-Identifier: CC0-1.0

//! Content publication through the native API, followed by an independent read
//! path. No Docker daemon/cache, production account or host filesystem required.
use crate::wire::{emulator_with_engine, layer_digest, ok};
use bytes::Bytes;
use serde_json::{Value, json};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state;
use smithy_server_runtime::MultiServiceRouter;

const OCI: &str = "application/vnd.oci.image.manifest.v1+json";

async fn request(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    token: &str,
    content_type: &str,
    body: &[u8],
    range: Option<&str>,
) -> http::Response<Bytes> {
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost:9297")
        .header("authorization", token)
        .header("content-type", content_type);
    if let Some(range) = range {
        request = request.header("content-range", range);
    }
    router
        .dispatch(
            request
                .body(Bytes::copy_from_slice(body))
                .expect("fixture request"),
        )
        .await
}

async fn setup(
    repo: &str,
) -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    String,
) {
    let (router, engine) = emulator_with_engine();
    ok(
        &router,
        "repository",
        "CreateRepository",
        &json!({"repositoryName":repo}),
    )
    .await;
    let issue = ok(
        &router,
        "registry login",
        "GetAuthorizationToken",
        &json!({}),
    )
    .await;
    let token = format!(
        "Basic {}",
        issue["authorizationData"][0]["authorizationToken"]
            .as_str()
            .unwrap()
    );
    (router, engine, token)
}

async fn upload(router: &MultiServiceRouter, repo: &str, token: &str, bytes: &[u8]) -> String {
    let start = request(
        router,
        "POST",
        &format!("/v2/{repo}/blobs/uploads/"),
        token,
        "application/octet-stream",
        b"",
        None,
    )
    .await;
    assert_eq!(start.status(), 202);
    let location = start.headers()["location"].to_str().unwrap().to_string();
    assert!(location.starts_with(&format!("/v2/{repo}/blobs/uploads/")));
    let digest = layer_digest(bytes);
    let response = request(
        router,
        "PUT",
        &format!("{location}?digest={digest}"),
        token,
        "application/octet-stream",
        bytes,
        None,
    )
    .await;
    assert_eq!(
        response.status(),
        201,
        "{}",
        String::from_utf8_lossy(response.body())
    );
    assert_eq!(response.headers()["docker-content-digest"], digest);
    digest
}

fn manifest(config: (&str, usize), layer: (&str, usize)) -> String {
    json!({"schemaVersion":2,"mediaType":OCI,
        "config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":config.0,"size":config.1},
        "layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":layer.0,"size":layer.1}]
    }).to_string()
}

#[tokio::test]
async fn registry_v2_push_roundtrip_publishes_exact_bytes_and_api_identity() {
    let repo = "cdk-assets/nested/example";
    let (router, _, token) = setup(repo).await;
    let config =
        br#"{"architecture":"arm64","os":"linux","rootfs":{"type":"layers","diff_ids":[]}}"#;
    let layer = b"harmless layer bytes: \x00\xff, not a host file";
    let c = upload(&router, repo, &token, config).await;
    let l = upload(&router, repo, &token, layer).await;
    let document = manifest((&c, config.len()), (&l, layer.len()));
    let digest = layer_digest(document.as_bytes());
    let address = format!("/v2/{repo}/manifests/asset-tag");
    for _ in 0..2 {
        // native PUT is idempotent, unlike API ImageAlreadyExists
        let response = request(
            &router,
            "PUT",
            &address,
            &token,
            OCI,
            document.as_bytes(),
            None,
        )
        .await;
        assert_eq!(
            response.status(),
            201,
            "{}",
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(response.headers()["docker-content-digest"], digest);
    }
    // A separate read request never consults a builder's cache.
    for reference in ["asset-tag", &digest] {
        let fetched = request(
            &router,
            "GET",
            &format!("/v2/{repo}/manifests/{reference}"),
            &token,
            "",
            b"",
            None,
        )
        .await;
        assert_eq!(fetched.status(), 200);
        assert_eq!(fetched.body().as_ref(), document.as_bytes());
        assert_eq!(layer_digest(fetched.body()), digest);
    }
    for (digest, original) in [(&c, &config[..]), (&l, &layer[..])] {
        let response = request(
            &router,
            "GET",
            &format!("/v2/{repo}/blobs/{digest}"),
            &token,
            "",
            b"",
            None,
        )
        .await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.body().as_ref(), original);
        assert_eq!(layer_digest(response.body()), *digest);
    }
    let api = ok(
        &router,
        "API same image",
        "BatchGetImage",
        &json!({"repositoryName":repo,"imageIds":[{"imageTag":"asset-tag"}]}),
    )
    .await;
    assert_eq!(api["images"][0]["imageId"]["imageDigest"], digest);
    assert_eq!(api["images"][0]["imageManifest"], document);
    let described = ok(
        &router,
        "CDK existence probe",
        "DescribeImages",
        &json!({"repositoryName":repo,"imageIds":[{"imageTag":"asset-tag"}]}),
    )
    .await;
    assert_eq!(described["imageDetails"][0]["imageDigest"], digest);
}

#[tokio::test]
async fn registry_v2_push_chunks_are_ordered_and_bad_completion_preserves_prefix() {
    let (router, _, token) = setup("chunked").await;
    let start = request(
        &router,
        "POST",
        "/v2/chunked/blobs/uploads/",
        &token,
        "",
        b"",
        None,
    )
    .await;
    let location = start.headers()["location"].to_str().unwrap();
    let good = request(&router, "PATCH", location, &token, "", b"abc", Some("0-2")).await;
    assert_eq!(good.status(), 202);
    assert_eq!(good.headers()["range"], "0-2");
    let wrong = request(&router, "PATCH", location, &token, "", b"bad", Some("1-3")).await;
    assert_eq!(wrong.status(), 416);
    let bad_digest = layer_digest(b"not the content");
    let wrong = request(
        &router,
        "PUT",
        &format!("{location}?digest={bad_digest}"),
        &token,
        "",
        b"def",
        None,
    )
    .await;
    assert_eq!(wrong.status(), 400);
    let status = request(&router, "GET", location, &token, "", b"", None).await;
    assert_eq!(status.status(), 204);
    assert_eq!(status.headers()["range"], "0-2");
    let digest = layer_digest(b"abcdef");
    let completed = request(
        &router,
        "PUT",
        &format!("{location}?digest={digest}"),
        &token,
        "",
        b"def",
        Some("3-5"),
    )
    .await;
    assert_eq!(completed.status(), 201);
    assert_eq!(
        request(&router, "GET", location, &token, "", b"", None)
            .await
            .status(),
        404
    );
    let got = request(
        &router,
        "GET",
        &format!("/v2/chunked/blobs/{digest}"),
        &token,
        "",
        b"",
        None,
    )
    .await;
    assert_eq!(got.body().as_ref(), b"abcdef");
}

#[tokio::test]
async fn registry_v2_push_monolithic_mount_fallback_and_cancel() {
    let (router, _, token) = setup("whole").await;
    let digest = layer_digest(b"whole bytes");
    let response = request(
        &router,
        "POST",
        &format!("/v2/whole/blobs/uploads/?digest={digest}"),
        &token,
        "",
        b"whole bytes",
        None,
    )
    .await;
    assert_eq!(response.status(), 201);
    let response = request(
        &router,
        "POST",
        &format!("/v2/whole/blobs/uploads/?mount={digest}&from=elsewhere"),
        &token,
        "",
        b"",
        None,
    )
    .await;
    assert_eq!(response.status(), 202); // decline mount honestly, request upload
    let location = response.headers()["location"].to_str().unwrap();
    assert_eq!(
        request(&router, "DELETE", location, &token, "", b"", None)
            .await
            .status(),
        204
    );
    assert_eq!(
        request(&router, "PATCH", location, &token, "", b"x", None)
            .await
            .status(),
        404
    );
}

#[tokio::test]
async fn registry_v2_push_refuses_missing_bytes_malformed_json_and_false_digest() {
    let (router, _, token) = setup("validation").await;
    let absent = layer_digest(b"absent");
    let missing = manifest((&absent, 6), (&absent, 6));
    for (path, body) in [
        ("latest".to_string(), missing),
        ("latest".to_string(), "{}".to_string()),
        (
            layer_digest(b"different"),
            json!({"schemaVersion":2,"config":{},"layers":[]}).to_string(),
        ),
    ] {
        assert_eq!(
            request(
                &router,
                "PUT",
                &format!("/v2/validation/manifests/{path}"),
                &token,
                OCI,
                body.as_bytes(),
                None
            )
            .await
            .status(),
            400
        );
    }
    let native = request(
        &router,
        "GET",
        "/v2/validation/manifests/latest",
        &token,
        "",
        b"",
        None,
    )
    .await;
    assert_eq!(native.status(), 404);
}

#[tokio::test]
async fn registry_v2_push_index_requires_stored_children() {
    let (router, _, token) = setup("index").await;
    let bytes = b"{}";
    let digest = upload(&router, "index", &token, bytes).await;
    let child = manifest((&digest, 2), (&digest, 2));
    let child_digest = layer_digest(child.as_bytes());
    let media = "application/vnd.oci.image.index.v1+json";
    let index = json!({"schemaVersion":2,"mediaType":media,"manifests":[{"mediaType":OCI,"digest":child_digest,"size":child.len(),"platform":{"architecture":"arm64","os":"linux"}}]}).to_string();
    assert_eq!(
        request(
            &router,
            "PUT",
            "/v2/index/manifests/latest",
            &token,
            media,
            index.as_bytes(),
            None
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        request(
            &router,
            "PUT",
            &format!("/v2/index/manifests/{child_digest}"),
            &token,
            OCI,
            child.as_bytes(),
            None
        )
        .await
        .status(),
        201
    );
    assert_eq!(
        request(
            &router,
            "PUT",
            "/v2/index/manifests/latest",
            &token,
            media,
            index.as_bytes(),
            None
        )
        .await
        .status(),
        201
    );
    assert_eq!(
        request(
            &router,
            "GET",
            "/v2/index/manifests/latest",
            &token,
            "",
            b"",
            None
        )
        .await
        .body()
        .as_ref(),
        index.as_bytes()
    );
}

#[tokio::test]
async fn registry_v2_push_immutable_tag_cannot_be_replaced() {
    let (router, _, token) = setup("immutable").await;
    ok(
        &router,
        "immutable",
        "PutImageTagMutability",
        &json!({"repositoryName":"immutable","imageTagMutability":"IMMUTABLE"}),
    )
    .await;
    let a = upload(&router, "immutable", &token, b"a").await;
    let b = upload(&router, "immutable", &token, b"b").await;
    let first = manifest((&a, 1), (&a, 1));
    let second = manifest((&b, 1), (&b, 1));
    let path = "/v2/immutable/manifests/latest";
    assert_eq!(
        request(&router, "PUT", path, &token, OCI, first.as_bytes(), None)
            .await
            .status(),
        201
    );
    assert_eq!(
        request(&router, "PUT", path, &token, OCI, second.as_bytes(), None)
            .await
            .status(),
        409
    );
    assert_eq!(
        request(&router, "GET", path, &token, "", b"", None)
            .await
            .body()
            .as_ref(),
        first.as_bytes()
    );
}

#[tokio::test]
async fn registry_v2_push_credential_and_live_policy_are_required_before_mutation() {
    let (router, engine, token) = setup("private").await;
    assert_eq!(
        request(
            &router,
            "POST",
            "/v2/private/blobs/uploads/",
            "Basic invalid",
            "",
            b"",
            None
        )
        .await
        .status(),
        401
    );
    ok(&router,"explicit deny","SetRepositoryPolicy",&json!({"repositoryName":"private","force":true,"policyText":json!({"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"ecr:*","Resource":"*"}]}).to_string()})).await;
    assert_eq!(
        request(
            &router,
            "POST",
            "/v2/private/blobs/uploads/",
            &token,
            "",
            b"",
            None
        )
        .await
        .status(),
        403
    );
    assert!(
        state::service_state(&*engine)
            .unwrap()
            .lock()
            .uploads
            .is_empty()
    );
}

#[tokio::test]
async fn registry_v2_push_upload_owner_and_repository_are_not_interchangeable() {
    let (router, engine, token) = setup("first").await;
    ok(
        &router,
        "second repository",
        "CreateRepository",
        &json!({"repositoryName":"second"}),
    )
    .await;
    let started = request(
        &router,
        "POST",
        "/v2/first/blobs/uploads/",
        &token,
        "",
        b"",
        None,
    )
    .await;
    let location = started.headers()["location"].to_str().unwrap().to_string();
    assert_eq!(
        request(
            &router,
            "PATCH",
            &location.replace("/first/", "/second/"),
            &token,
            "",
            b"x",
            None
        )
        .await
        .status(),
        404
    );
    let state = state::service_state(&*engine).unwrap();
    state
        .lock()
        .uploads
        .values_mut()
        .next()
        .unwrap()
        .registry_principal = Some("a different owner".to_string());
    assert_eq!(
        request(&router, "DELETE", &location, &token, "", b"", None)
            .await
            .status(),
        404
    );
}

/// Opt-in real Docker client witness. Imports only our tiny tar (no image pull,
/// build context, command execution in a container or host credential config).
/// After push all our local tags are removed before a real digest-addressed pull.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit EMULATOR_ECR_DOCKER_TEST=1 and local Docker socket"]
async fn registry_v2_push_real_docker_push_then_cacheless_pull() {
    assert_eq!(
        std::env::var("EMULATOR_ECR_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let dir = tempfile::tempdir().expect("private disposable Docker config");
    let (router, engine, token) = setup("fixture-publication").await;
    let (endpoint, _) = emulator_server::serve_on_ephemeral_port(router).await;
    let endpoint = endpoint.replace("localhost", "127.0.0.1");
    let authority = endpoint.strip_prefix("http://").expect("fixture HTTP");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let source = format!("cdk-local-registry-witness-{}:{nonce}", std::process::id());
    let destination = format!("{authority}/fixture-publication:{nonce}");
    let auth = token.strip_prefix("Basic ").unwrap();
    // The only credential here was minted by this ephemeral test registry. Do
    // not inherit ~/.docker, AWS profiles, environment, proxies or auth helpers.
    std::fs::write(
        dir.path().join("config.json"),
        json!({"auths":{(authority):{"auth":auth}}}).to_string(),
    )
    .unwrap();
    let tar_path = dir.path().join("rootfs.tar");
    let file = std::fs::File::create(&tar_path).unwrap();
    let mut tar = tar::Builder::new(file);
    let content = b"generic registry publication witness\n";
    let mut header = tar::Header::new_gnu();
    header.set_path("publication.txt").unwrap();
    header.set_mode(0o644);
    header.set_size(content.len() as u64);
    header.set_cksum();
    tar.append(&header, &content[..]).unwrap();
    tar.finish().unwrap();
    drop(tar);

    async fn docker(config: &std::path::Path, arguments: &[&str]) -> bool {
        let mut child = std::process::Command::new("/usr/bin/docker")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", config)
            .arg("--host")
            .arg("unix:///var/run/docker.sock")
            .arg("--config")
            .arg(config)
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("local Docker client");
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().expect("observe owned client") {
                return status.success();
            }
            if start.elapsed() >= std::time::Duration::from_secs(30) {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    // Use a result block so all our exact tags are cleaned even on a failed
    // assertion. No pruning, container actions or unrelated image deletion.
    let result: Result<String, &'static str> = async {
        if !docker(
            dir.path(),
            &[
                "import",
                "--platform",
                "linux/arm64",
                tar_path.to_str().unwrap(),
                &source,
            ],
        )
        .await
        {
            return Err("fixture import failed");
        }
        if !docker(dir.path(), &["tag", &source, &destination]).await {
            return Err("fixture tag failed");
        }
        if !docker(dir.path(), &["push", &destination]).await {
            return Err("native registry push failed");
        }
        let (digest, document) = {
            let state = state::service_state(&*engine).unwrap();
            let store = state.lock();
            let repo = store.repository("fixture-publication").unwrap();
            let digest = repo
                .digest_of_tag(&nonce.to_string())
                .ok_or("missing pushed tag")?;
            let image = repo.images.get(digest).ok_or("missing pushed manifest")?;
            (digest.clone(), image.manifest.clone())
        };
        let document = document.read(&crate::wire::payload_store(&engine)).await.map_err(|_| "stored manifest read failed")?;
        // An independent HTTP client, not Docker's local content cache, must
        // receive and hash every actual manifest/config/layer byte.
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let response = client
            .get(format!(
                "{endpoint}/v2/fixture-publication/manifests/{digest}"
            ))
            .header("authorization", &token)
            .send()
            .await
            .map_err(|_| "manifest transport failed")?;
        if !response.status().is_success() {
            return Err("manifest GET failed");
        }
        let received = response.bytes().await.map_err(|_| "manifest body failed")?;
        if received.as_ref() != document.as_bytes() || layer_digest(&received) != digest {
            return Err("manifest byte identity failed");
        }
        let document: Value =
            serde_json::from_slice(&received).map_err(|_| "manifest JSON failed")?;
        let descriptors = document["layers"]
            .as_array()
            .ok_or("image layers missing")?;
        for descriptor in descriptors.iter().chain(Some(&document["config"])) {
            let digest = descriptor["digest"].as_str().ok_or("blob digest missing")?;
            let bytes = client
                .get(format!("{endpoint}/v2/fixture-publication/blobs/{digest}"))
                .header("authorization", &token)
                .send()
                .await
                .map_err(|_| "blob transport failed")?
                .error_for_status()
                .map_err(|_| "blob GET failed")?
                .bytes()
                .await
                .map_err(|_| "blob body failed")?;
            if layer_digest(&bytes) != digest
                || descriptor["size"].as_u64() != Some(bytes.len() as u64)
            {
                return Err("blob byte identity failed");
            }
            if descriptor == &document["config"] {
                let config: Value =
                    serde_json::from_slice(&bytes).map_err(|_| "config JSON failed")?;
                if config["architecture"] != "arm64" || config["os"] != "linux" {
                    return Err("image platform identity failed");
                }
            }
        }
        if !docker(dir.path(), &["image", "rm", &source, &destination]).await {
            return Err("remove own cached tags failed");
        }
        if docker(dir.path(), &["image", "inspect", &destination]).await {
            return Err("destination remained in local cache");
        }
        let pinned = format!("{authority}/fixture-publication@{digest}");
        if !docker(dir.path(), &["pull", "--platform", "linux/arm64", &pinned]).await {
            return Err("digest-addressed Docker pull failed");
        }
        if !docker(dir.path(), &["image", "inspect", &pinned]).await {
            return Err("pulled identity missing");
        }
        let _ = docker(dir.path(), &["image", "rm", &pinned]).await;
        Ok(digest)
    }
    .await;
    let _ = docker(dir.path(), &["image", "rm", &source, &destination]).await;
    assert!(
        result.is_ok(),
        "{}",
        result
            .as_ref()
            .err()
            .copied()
            .unwrap_or("unexpected failure")
    );
    eprintln!(
        "real Docker publication/digest pull checked: {}",
        result.unwrap()
    );
}
