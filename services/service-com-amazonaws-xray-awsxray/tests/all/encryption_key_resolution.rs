// SPDX-License-Identifier: CC0-1.0

//! Which KEY the encryption configuration reports, end to end over the wire,
//! with this emulator's own KMS mounted beside X-Ray on one shared engine.
//!
//! The rule: `EncryptionConfig$KeyId` is "the ID of the KMS key used for
//! encryption" (`smithy_query(query="shape",shape="EncryptionConfig")`), so a
//! stored configuration reports the ARN of the KEY the submitted selector
//! resolves to — the `…:key/<key id>` ARN `kms:DescribeKey` answers for it —
//! whichever of the three selector forms `PutEncryptionConfig$KeyId` admits the
//! request used. An alias is not that key: it names whatever key it currently
//! targets, and two aliases may target one key.
//!
//! The rows differ along what the rule generalises over: the FORM of the
//! selector (alias name, alias ARN, key id, key ARN), whether the key is
//! customer managed or one of the Amazon Web Services managed keys a predefined
//! `alias/aws/…` resolves to, and which alias of a key with two was used — the
//! ARN reported is the key's in every case, never the selector re-rendered.

use serde_json::{json, Value};

use super::xray_wire::{Answer, Xray, DEFAULT_REGION};

/// The region every call below is signed for; a KMS key is regional, so it is
/// also the region the key is resolved in.
const REGION: &str = DEFAULT_REGION;

/// One emulator with X-Ray and KMS mounted on the SAME engine, so the key the
/// configuration names is the key KMS itself holds — the shared wire harness
/// ([`super::xray_wire`]) with the other service mounted beside this one.
fn emulator() -> Xray {
    Xray::beside(service_com_amazonaws_kms_trentservice::EmulatedService::new)
}

/// A member of the `EncryptionConfig` document an answer carries, or
/// `Value::Null`.
fn config(answer: &Answer, member: &str) -> Value {
    answer
        .get("EncryptionConfig")
        .get(member)
        .cloned()
        .unwrap_or(Value::Null)
}

/// A KMS client over this emulator's own in-process callback — the same door a
/// caller of `kms:DescribeKey` goes through, and the one the X-Ray handler
/// resolves its selector over.
fn kms(emulator: &Xray) -> ::aws_sdk_kms::Client {
    let callback = emulator.callback();
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(REGION))
        .endpoint_url(format!("https://kms.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}

/// `PutEncryptionConfig(Type=KMS, KeyId=<selector>)`.
async fn put_kms(emulator: &Xray, key_id: &str) -> Answer {
    emulator
        .call(
            "/PutEncryptionConfig",
            &json!({ "Type": "KMS", "KeyId": key_id }),
        )
        .await
}

/// `GetEncryptionConfig`, whose request models no members.
async fn get_config(emulator: &Xray) -> Answer {
    emulator.call("/EncryptionConfig", &json!({})).await
}

/// The ARN `kms:DescribeKey` answers for `selector`: `KeyMetadata$Arn`, the one
/// identifier of the key itself.
async fn described_key_arn(emulator: &Xray, selector: &str) -> String {
    let described = kms(emulator)
        .describe_key()
        .key_id(selector)
        .send()
        .await
        .unwrap_or_else(|error| panic!("KMS must describe {selector:?}: {error:?}"));
    described
        .key_metadata()
        .and_then(::aws_sdk_kms::types::KeyMetadata::arn)
        .unwrap_or_else(|| panic!("KMS must answer an Arn for {selector:?}"))
        .to_string()
}

/// Read the stored configuration until the transition the clock owes it is
/// reported, and answer the document then.
///
/// A configuration reports `UPDATING` for a span of the service's own choosing,
/// so the condition is waited FOR rather than for a duration: the deadline is
/// generous and the poll looks again until the status is the terminal one.
async fn active_config(emulator: &Xray, case: &str) -> Answer {
    let deadline = ::std::time::Instant::now() + ::std::time::Duration::from_secs(30);
    loop {
        let read = get_config(emulator).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the configuration must succeed, got {read:?}"
        );
        if config(&read, "Status") == json!("ACTIVE") {
            return read;
        }
        assert!(
            ::std::time::Instant::now() < deadline,
            "case {case:?}: the configuration never became ACTIVE, last read {read:?}"
        );
        ::std::thread::sleep(::std::time::Duration::from_millis(25));
    }
}

#[tokio::test]
async fn a_stored_configuration_reports_the_key_its_selector_resolves_to() {
    let emulator = emulator();

    // A customer managed key with two aliases of its own, so the SAME key is
    // selected four ways: by either alias name, by its key id and by its ARN.
    let created = kms(&emulator)
        .create_key()
        .description("the key the configuration names")
        .send()
        .await
        .expect("KMS must create a key");
    let metadata = created
        .key_metadata()
        .expect("a created key carries metadata")
        .clone();
    let key_id = metadata.key_id().to_string();
    let key_arn = metadata
        .arn()
        .expect("a created key carries an ARN")
        .to_string();
    for alias in ["alias/primary", "alias/secondary"] {
        kms(&emulator)
            .create_alias()
            .alias_name(alias)
            .target_key_id(&key_id)
            .send()
            .await
            .unwrap_or_else(|error| panic!("KMS must create {alias:?}: {error:?}"));
    }
    let alias_arn = format!(
        "arn:aws:kms:{REGION}:{}:alias/primary",
        emulator_server::emulator_account_id()
    );

    // (case, the selector the request submits)
    let rows: Vec<(&str, String)> = vec![
        ("an alias name", "alias/primary".to_string()),
        ("the other alias of the same key", "alias/secondary".to_string()),
        ("the full ARN of an alias", alias_arn),
        ("a key id", key_id.clone()),
        ("the full ARN of a key", key_arn.clone()),
        // A predefined Amazon Web Services alias: KMS associates it with an
        // Amazon Web Services managed key, whose ARN is what the configuration
        // reports — never the alias.
        ("a predefined service alias", "alias/aws/xray".to_string()),
        (
            "another predefined service alias",
            "alias/aws/s3".to_string(),
        ),
    ];

    for (case, selector) in rows {
        // What the key's own ARN is, read from KMS: the identifier the
        // configuration has to report for this selector.
        let expected = described_key_arn(&emulator, &selector).await;
        assert!(
            expected.contains(":key/"),
            "case {case:?}: KMS must answer a key ARN, got {expected:?}"
        );

        let put = put_kms(&emulator, &selector).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the put must succeed, got {put:?}"
        );
        assert_eq!(
            config(&put, "KeyId"),
            json!(expected),
            "case {case:?}: the put must answer the resolved key ARN, got {:?}",
            config(&put, "KeyId")
        );

        // And the same ARN is what a reader is answered once the configuration
        // has settled: the resolution is stored, not re-derived per read.
        let settled = active_config(&emulator, case).await;
        assert_eq!(
            config(&settled, "KeyId"),
            json!(expected),
            "case {case:?}: the stored configuration must report the key ARN, got {:?}",
            config(&settled, "KeyId")
        );
        assert_eq!(
            config(&settled, "Type"),
            json!("KMS"),
            "case {case:?}: the stored type must be KMS, got {:?}",
            config(&settled, "Type")
        );
        // The selector the request used is not what is reported, unless it
        // already WAS the key's ARN.
        if selector != key_arn {
            assert_ne!(
                config(&settled, "KeyId"),
                json!(selector),
                "case {case:?}: the selector must not be reported back"
            );
        }
    }

    // The two aliases of one key resolve to ONE key, which is why an alias is
    // not an identity: reporting either alias back would make two
    // configurations of the same key look like two different keys.
    assert_eq!(
        described_key_arn(&emulator, "alias/primary").await,
        described_key_arn(&emulator, "alias/secondary").await,
        "both aliases must resolve to the same key"
    );
}

#[tokio::test]
async fn a_selector_no_kms_resolves_keeps_the_form_it_was_submitted_in() {
    // The fail-safe direction, with KMS mounted and holding no such key: nothing
    // resolved it, so the configuration reports the qualified form of the
    // selector the request submitted rather than a key's identity nothing
    // stands behind. Whether such a request is answered at all, or refused, is
    // the subject of its own open issue
    // (20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive),
    // so an answer that is not a 200 leaves this rule with nothing to say.
    let emulator = emulator();
    let account = emulator_server::emulator_account_id();

    // (the selector submitted, the qualified form of it)
    let rows: Vec<(String, String)> = vec![
        (
            "alias/never-created".to_string(),
            format!("arn:aws:kms:{REGION}:{account}:alias/never-created"),
        ),
        (
            format!("arn:aws:kms:{REGION}:{account}:alias/never-created"),
            format!("arn:aws:kms:{REGION}:{account}:alias/never-created"),
        ),
        (
            "ae4aa6d4-9a4d-489d-9a47-54ff6d789845".to_string(),
            format!("arn:aws:kms:{REGION}:{account}:key/ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
    ];

    for (selector, qualified) in rows {
        let put = put_kms(&emulator, &selector).await;
        // Either answer is a rule this case can state: a 200 reports the
        // qualified selector and nothing else, and a refusal is the modelled
        // one. What no answer may do is report a key.
        if put.status == 200 {
            assert_eq!(
                config(&put, "KeyId"),
                json!(qualified),
                "selector {selector:?}: expected the qualified selector {qualified:?}, got {:?}",
                config(&put, "KeyId")
            );
        } else {
            assert_eq!(
                (put.status, put.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "selector {selector:?}: a selector KMS does not resolve is either stored as \
                 submitted or refused as invalid, got {put:?}"
            );
        }
    }
}
