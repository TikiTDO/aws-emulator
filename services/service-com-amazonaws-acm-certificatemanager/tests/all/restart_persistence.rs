// SPDX-License-Identifier: CC0-1.0

//! ACM state AND its serving TLS key material directory survive a restart through
//! `service_common::persist`, so both ACM queries (`DescribeCertificate`, `GetCertificate`,
//! `ListTagsForResource`) and HTTPS load balancer listeners (`CertificateKeyMaterialDirectory`)
//! continue serving imported and sandbox-issued certificates after a restart.

use serde_json::json;
use service_com_amazonaws_acm_certificatemanager::state::ComAmazonawsAcmCertificatemanagerState as AcmState;
use service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory;
use service_common::persist::Persisted;

use crate::certificate_import::{generate, import_body, Blueprint};
use crate::certificate_lifecycle::{call, router_with_engine};

#[tokio::test]
async fn certificate_metadata_tags_and_serving_key_material_survive_restart() {
    let (before, engine) = router_with_engine();
    let generated = generate(&Blueprint {
        common_name: Some("api.app.test"),
        dns_names: &["api.app.test", "*.app.test"],
        ..Blueprint::default()
    });
    let mut body = import_body(&generated);
    body["Tags"] = json!([{ "Key": "tier", "Value": "edge" }]);

    let (status, imported) = call(&before, "ImportCertificate", &body).await;
    assert_eq!(status, 200, "ImportCertificate failed: {imported}");
    let arn = imported["CertificateArn"].as_str().unwrap().to_string();

    let acm_json = serde_json::to_string(&engine.state::<AcmState>().lock().snapshot()).unwrap();
    let key_dir_json = serde_json::to_string(
        &engine
            .state::<CertificateKeyMaterialDirectory>()
            .lock()
            .snapshot(),
    )
    .unwrap();

    let (after, fresh) = router_with_engine();
    *fresh.state::<AcmState>().lock() =
        AcmState::restore(AcmState::VERSION, serde_json::from_str(&acm_json).unwrap()).unwrap();
    *fresh.state::<CertificateKeyMaterialDirectory>().lock() =
        CertificateKeyMaterialDirectory::restore(
            CertificateKeyMaterialDirectory::VERSION,
            serde_json::from_str(&key_dir_json).unwrap(),
        )
        .unwrap();

    let (status, described) = call(
        &after,
        "DescribeCertificate",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(described["Certificate"]["DomainName"], "api.app.test");

    let (status, fetched) = call(&after, "GetCertificate", &json!({ "CertificateArn": arn })).await;
    assert_eq!(status, 200);
    assert_eq!(
        fetched["Certificate"].as_str().unwrap().trim(),
        generated.certificate.trim()
    );

    let (status, tags) = call(
        &after,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(tags["Tags"], json!([{ "Key": "tier", "Value": "edge" }]));

    let serving = fresh
        .state::<CertificateKeyMaterialDirectory>()
        .lock()
        .material(&arn)
        .expect("serving TLS key material must be restored");
    assert_eq!(serving.private_key_pem.trim(), generated.private_key.trim());
    assert!(serving.covers("api.app.test"));
    assert!(serving.covers("portal.app.test"));
}

// Hearth, PR 19 owner reading, finding 2: an issued certificate restored without its private key
// must be reported, not reminted under the same ARN. The gate is what boot calls.
#[tokio::test]
async fn an_issued_certificate_restored_without_its_key_is_reported() {
    use service_com_amazonaws_acm_certificatemanager::state::sandbox_ca::issued_without_key_material;

    let (router, engine) = router_with_engine();
    let generated = generate(&Blueprint {
        common_name: Some("api.app.test"),
        dns_names: &["api.app.test"],
        ..Blueprint::default()
    });
    let (status, imported) = call(&router, "ImportCertificate", &import_body(&generated)).await;
    assert_eq!(status, 200, "ImportCertificate failed: {imported}");
    let arn = imported["CertificateArn"].as_str().unwrap().to_string();
    // Stand the imported certificate in for an Amazon-issued one: the remint path is theirs.
    engine.state::<AcmState>().lock().certificates.get_mut(&arn).unwrap().cert_type =
        "AMAZON_ISSUED".to_string();

    let acm = engine.state::<AcmState>();
    let with_keys = engine.state::<CertificateKeyMaterialDirectory>();
    assert!(issued_without_key_material(&acm.lock().certificates, &with_keys.lock()).is_empty());

    let missing = CertificateKeyMaterialDirectory::default();
    assert_eq!(issued_without_key_material(&acm.lock().certificates, &missing), vec![arn]);
}
