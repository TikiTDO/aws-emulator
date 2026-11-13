// SPDX-License-Identifier: CC0-1.0

use super::*;

const KEY: &str = "EMULATORSTREAMTEST";
const SECRET: &str = "disposable-stream-fixture-secret";

fn authenticator() -> &'static SigV4Authenticator {
    Box::leak(Box::new(SigV4Authenticator {
        access_key_id: KEY.into(),
        secret_access_key: SECRET.into(),
    }))
}

// Fixture producer is the native SDK signer, not the incremental verifier.
fn signed(body: &[u8], marker: Option<&str>, headers: &[(&str, &str)]) -> Parts {
    let mut request = http::Request::builder()
        .method("PUT")
        .uri("https://127.0.0.1:8443/bucket/key%20one")
        .header("host", "127.0.0.1:8443")
        .body(())
        .unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    if let Some(marker) = marker {
        request
            .headers_mut()
            .insert(X_AMZ_CONTENT_SHA256, marker.parse().unwrap());
    }
    let credentials =
        aws_credential_types::Credentials::new(KEY, SECRET, None, None, "stream-test");
    let identity = credentials.into();
    let params = aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region("us-east-1")
        .name("s3")
        .time(SystemTime::now())
        .settings(signing_settings("s3", None))
        .build()
        .unwrap()
        .into();
    let payload = marker.map_or(SignableBody::Bytes(body), |m| {
        SignableBody::Precomputed(m.into())
    });
    let header_pairs = request
        .headers()
        .iter()
        .map(|(n, v)| (n.as_str(), v.to_str().unwrap()));
    let signable =
        SignableRequest::new("PUT", request.uri().to_string(), header_pairs, payload).unwrap();
    let (instructions, _) = sign(signable, &params).unwrap().into_parts();
    instructions.apply_to_request_http1x(&mut request);
    request.into_parts().0
}

fn drive(
    parts: &Parts,
    body: &[u8],
    frame_size: usize,
) -> Result<(StreamAuthentication, Vec<u8>), StreamValidationError> {
    let mut stream = begin(
        authenticator(),
        parts,
        "s3",
        Arc::new(CredentialStore::default()),
    )
    .map_err(|e| match e {
        StreamAuthenticationError::Denied(e) => StreamValidationError::Authentication(e),
        StreamAuthenticationError::Unsupported => framing("unsupported test input"),
    })?;
    let mut decoded = Vec::new();
    for frame in body.chunks(frame_size) {
        let mut remaining = frame;
        while !remaining.is_empty() {
            let step = stream.consume(remaining)?;
            assert!(step.consumed > 0 && step.consumed <= remaining.len());
            if let Some(data) = step.payload {
                decoded.extend_from_slice(data);
            }
            remaining = &remaining[step.consumed..];
        }
    }
    Ok((stream.finish(None)?, decoded))
}

#[test]
fn plain_hash_and_unsigned_preserve_real_header_authentication() {
    let body = b"not the empty request";
    let parts = signed(body, None, &[]);
    for size in 1..=body.len() {
        let (receipt, decoded) = drive(&parts, body, size).unwrap();
        assert_eq!(decoded, body);
        assert_eq!(receipt.binding(), &PayloadSignatureBinding::Sha256);
        assert_eq!(receipt.decoded_bytes(), body.len() as u64);
        assert_eq!(
            receipt.decoded_sha256().as_slice(),
            digest::digest(&digest::SHA256, body).as_ref()
        );
    }
    assert!(matches!(
        drive(&parts, b"body was changed", 2),
        Err(StreamValidationError::Authentication(_))
    ));
    let mut unsigned = signed(body, Some("UNSIGNED-PAYLOAD"), &[]);
    assert_eq!(
        drive(&unsigned, b"another unsigned body", 3)
            .unwrap()
            .0
            .binding(),
        &PayloadSignatureBinding::Unsigned
    );
    unsigned
        .headers
        .insert("host", "wrong.example".parse().unwrap());
    assert!(matches!(
        drive(&unsigned, body, 2),
        Err(StreamValidationError::Authentication(_))
    ));
    let plain = signed(body, Some("UNSIGNED-PAYLOAD"), &[]);
    let mut stream = begin(
        authenticator(),
        &plain,
        "s3",
        Arc::new(CredentialStore::default()),
    )
    .unwrap();
    stream.consume(body).unwrap();
    let mut trailers = http::HeaderMap::new();
    trailers.insert("x-unexpected", "trailer".parse().unwrap());
    assert!(matches!(
        stream.finish(Some(&trailers)),
        Err(StreamValidationError::MalformedFraming(_))
    ));
}

#[test]
fn actual_unsigned_sha256_trailer_form_is_partition_independent_and_terminal() {
    let data = b"the asset bytes";
    let checksum = aws_smithy_types::base64::encode(digest::digest(&digest::SHA256, data).as_ref());
    let body = format!(
        "{:x}\r\n{}\r\n0\r\nx-amz-checksum-sha256:{checksum}\r\n\r\n",
        data.len(),
        std::str::from_utf8(data).unwrap()
    );
    let parts = signed(
        &[],
        Some("STREAMING-UNSIGNED-PAYLOAD-TRAILER"),
        &[
            ("content-encoding", "aws-chunked"),
            ("x-amz-decoded-content-length", "15"),
            ("x-amz-trailer", "x-amz-checksum-sha256"),
        ],
    );
    for size in 1..=body.len() {
        let (receipt, decoded) = drive(&parts, body.as_bytes(), size).unwrap();
        assert_eq!(decoded, data);
        assert_eq!(receipt.entity_bytes(), body.len() as u64);
        assert_eq!(receipt.binding(), &PayloadSignatureBinding::Unsigned);
    }
    let bad = body.replace(&checksum, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
    assert!(matches!(
        drive(&parts, bad.as_bytes(), 1),
        Err(StreamValidationError::Integrity(
            StreamIntegrityFailure::ChecksumMismatch
        ))
    ));
    assert!(drive(&parts, &body.as_bytes()[..body.len() - 1], 2).is_err());
    assert!(drive(&parts, format!("{body}extra").as_bytes(), 3).is_err());
    let duplicate = body.replace(
        "\r\n\r\n",
        &format!("\r\nx-amz-checksum-sha256:{checksum}\r\n\r\n"),
    );
    assert!(drive(&parts, duplicate.as_bytes(), 4).is_err());
    let wrong_length = signed(
        &[],
        Some("STREAMING-UNSIGNED-PAYLOAD-TRAILER"),
        &[
            ("content-encoding", "aws-chunked"),
            ("x-amz-decoded-content-length", "16"),
            ("x-amz-trailer", "x-amz-checksum-sha256"),
        ],
    );
    assert!(matches!(
        drive(&wrong_length, body.as_bytes(), 5),
        Err(StreamValidationError::Integrity(
            StreamIntegrityFailure::DecodedLengthMismatch
        ))
    ));
}

fn sdk_chunk(signed: &Signature, previous: &str, bytes: &[u8]) -> String {
    with_signing_params(signed, SECRET, (), |params| {
        aws_sigv4::sign::v4::sign_chunk(&bytes::Bytes::copy_from_slice(bytes), previous, &params)
            .ok()
            .map(|s| s.signature().to_owned())
    })
    .unwrap()
}

#[test]
fn chained_chunks_and_signed_trailer_use_sdk_oracles() {
    let data = b"signed asset";
    for trailers in [false, true] {
        let marker = if trailers {
            "STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER"
        } else {
            "STREAMING-AWS4-HMAC-SHA256-PAYLOAD"
        };
        let mut headers = vec![("content-encoding", "aws-chunked")];
        if trailers {
            headers.push(("x-amz-trailer", "x-amz-checksum-sha256"));
        }
        let parts = signed(&[], Some(marker), &headers);
        let parsed = Signature::of(&parts).unwrap();
        let first = sdk_chunk(&parsed, &parsed.value, data);
        let zero = sdk_chunk(&parsed, &first, &[]);
        let mut body = format!(
            "{:x};chunk-signature={first}\r\n{}\r\n0;chunk-signature={zero}\r\n",
            data.len(),
            std::str::from_utf8(data).unwrap()
        );
        if trailers {
            let checksum =
                aws_smithy_types::base64::encode(digest::digest(&digest::SHA256, data).as_ref());
            let mut fields = aws_smithy_runtime_api::http::Headers::new();
            fields.insert("x-amz-checksum-sha256", checksum.clone());
            let trailer_sig = with_signing_params(&parsed, SECRET, (), |params| {
                aws_sigv4::sign::v4::sign_trailer(&fields, &zero, &params)
                    .ok()
                    .map(|s| s.signature().to_owned())
            })
            .unwrap();
            body.push_str(&format!(
                "x-amz-checksum-sha256:{checksum}\r\nx-amz-trailer-signature:{trailer_sig}\r\n"
            ));
        }
        body.push_str("\r\n");
        for size in [1, 2, 7, 64, 8192] {
            let (receipt, decoded) = drive(&parts, body.as_bytes(), size).unwrap();
            assert_eq!(decoded, data);
            assert_eq!(receipt.binding(), &PayloadSignatureBinding::SignedChunks);
        }
        if trailers {
            // Data checksum, data signatures and zero-chunk signature remain correct.
            let start =
                body.find("x-amz-trailer-signature:").unwrap() + "x-amz-trailer-signature:".len();
            let mut bad = body.clone();
            let replacement = if &bad[start..start + 1] == "0" {
                "1"
            } else {
                "0"
            };
            bad.replace_range(start..start + 1, replacement);
            assert!(matches!(
                drive(&parts, bad.as_bytes(), 7),
                Err(StreamValidationError::Authentication(_))
            ));
        }
        let bad = body.replace(&zero, &"0".repeat(64));
        assert!(matches!(
            drive(&parts, bad.as_bytes(), 3),
            Err(StreamValidationError::Authentication(_))
        ));
        let bad = body.replace("signed asset", "SIGNED asset");
        assert!(matches!(
            drive(&parts, bad.as_bytes(), 5),
            Err(StreamValidationError::Authentication(_))
        ));
    }
}

#[test]
fn fixed_crc32_vector_and_failure_latch_are_real() {
    let parts = signed(
        &[],
        Some("STREAMING-UNSIGNED-PAYLOAD-TRAILER"),
        &[
            ("content-encoding", "aws-chunked"),
            ("x-amz-trailer", "x-amz-checksum-crc32"),
        ],
    );
    let data = b"123456789";
    let positive = b"9\r\n123456789\r\n0\r\nx-amz-checksum-crc32:y/Q5Jg==\r\n\r\n";
    assert_eq!(drive(&parts, positive, 1).unwrap().1, data);
    let mut stream = begin(
        authenticator(),
        &parts,
        "s3",
        Arc::new(CredentialStore::default()),
    )
    .unwrap();
    assert!(stream.consume(b"not-hex\r\n").is_err());
    assert!(stream.consume(positive).is_err());
    assert!(stream.finish(None).is_err());
}

#[test]
fn huge_declared_chunk_does_not_allocate_and_anonymous_does_not_claim_signature() {
    let parts = signed(
        &[],
        Some("STREAMING-UNSIGNED-PAYLOAD-TRAILER"),
        &[
            ("content-encoding", "aws-chunked"),
            ("x-amz-trailer", "x-amz-checksum-sha256"),
        ],
    );
    let mut stream = Stream::new(&parts, false).unwrap();
    assert!(stream.consume(b"ffffffffffffff\r\n").is_ok());
    assert!(stream.line.capacity() <= MAX_LINE_BYTES);
    assert!(stream.consume(b"small fragment").is_ok());
    assert!(Box::new(stream).finish(None).is_err());
    let plain = signed(b"body", Some("UNSIGNED-PAYLOAD"), &[]);
    let mut anonymous = begin_uncredentialed_stream(&plain).unwrap();
    anonymous.consume(b"body").unwrap();
    assert_eq!(
        anonymous.finish(None).unwrap().binding(),
        &PayloadSignatureBinding::Unsigned
    );
    let signed_chunks = signed(
        &[],
        Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD"),
        &[("content-encoding", "aws-chunked")],
    );
    assert!(matches!(
        begin_uncredentialed_stream(&signed_chunks),
        Err(StreamAuthenticationError::Unsupported)
    ));
}

#[test]
fn large_single_chunk_uses_incremental_native_signature_relation() {
    let data = vec![0x5a; 64 * 1024];
    let parts = signed(
        &[],
        Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD"),
        &[("content-encoding", "aws-chunked")],
    );
    let parsed = Signature::of(&parts).unwrap();
    let first = sdk_chunk(&parsed, &parsed.value, &data);
    let zero = sdk_chunk(&parsed, &first, &[]);
    let mut body = format!("{:x};chunk-signature={first}\r\n", data.len()).into_bytes();
    body.extend_from_slice(&data);
    body.extend_from_slice(format!("\r\n0;chunk-signature={zero}\r\n\r\n").as_bytes());
    let (receipt, decoded) = drive(&parts, &body, 997).unwrap();
    assert_eq!(decoded, data);
    assert_eq!(receipt.decoded_bytes(), 64 * 1024);
}

#[test]
fn native_checksum_algorithms_have_fixed_known_value_controls() {
    let data = b"123456789";
    // Published standard check values, rendered big-endian/base64 as the native SDK does.
    for (name, value) in [
        ("x-amz-checksum-crc32", "y/Q5Jg=="),
        ("x-amz-checksum-crc32c", "4waSgw=="),
    ] {
        let parts = signed(data, Some("UNSIGNED-PAYLOAD"), &[(name, value)]);
        let receipt = drive(&parts, data, 2).unwrap().0;
        assert_eq!(receipt.checksums(), &[(name, value.to_owned())]);
    }
}

#[test]
fn presigned_s3_matches_native_signer_and_existing_buffered_verdict() {
    let mut request = http::Request::builder()
        .method("PUT")
        .uri("https://127.0.0.1:8443/bucket/key%20one")
        .header("host", "127.0.0.1:8443")
        .body(())
        .unwrap();
    let credentials = aws_credential_types::Credentials::new(KEY, SECRET, None, None, "query-test");
    let identity = credentials.into();
    let mut settings = signing_settings("s3", Some(600));
    settings.signature_location = SignatureLocation::QueryParams;
    let params = aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region("us-east-1")
        .name("s3")
        .time(SystemTime::now())
        .settings(settings)
        .build()
        .unwrap()
        .into();
    let headers = request
        .headers()
        .iter()
        .map(|(n, v)| (n.as_str(), v.to_str().unwrap()));
    let signable = SignableRequest::new(
        "PUT",
        request.uri().to_string(),
        headers,
        SignableBody::UnsignedPayload,
    )
    .unwrap();
    let (instructions, _) = sign(signable, &params).unwrap().into_parts();
    instructions.apply_to_request_http1x(&mut request);
    let (mut parts, _) = request.into_parts();
    assert!(parts.headers.get(http::header::AUTHORIZATION).is_none());
    let body = b"nonempty presigned payload";
    authenticator()
        .verify(&parts, body, "s3", &CredentialStore::default())
        .unwrap();
    let (receipt, actual) = drive(&parts, body, 3).unwrap();
    assert_eq!(actual, body);
    assert_eq!(receipt.binding(), &PayloadSignatureBinding::Unsigned);
    parts.uri = parts
        .uri
        .to_string()
        .replace("/bucket/", "/elsewhere/")
        .parse()
        .unwrap();
    assert!(
        authenticator()
            .verify(&parts, body, "s3", &CredentialStore::default())
            .is_err()
    );
    assert!(matches!(
        drive(&parts, body, 3),
        Err(StreamValidationError::Authentication(_))
    ));
}
