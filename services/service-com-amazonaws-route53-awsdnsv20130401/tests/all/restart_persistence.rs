// SPDX-License-Identifier: CC0-1.0

//! Route 53 hosted zones, record sets, and tags survive a restart through
//! `service_common::persist`.

use std::sync::Arc;

use service_com_amazonaws_route53_awsdnsv20130401::state::ComAmazonawsRoute53Awsdnsv20130401State as Route53State;
use service_common::persist::Persisted;
use smithy_server_runtime::MultiServiceRouter;

use super::hosted_zone_records::call;

fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

#[tokio::test]
async fn hosted_zones_records_and_tags_survive_restart() {
    let (before, engine) = router_with_engine();
    let created = call(
        &before,
        "POST",
        "/2013-04-01/hostedzone",
        r#"<CreateHostedZoneRequest xmlns="https://route53.amazonaws.com/doc/2013-04-01/">
            <Name>app.test.</Name>
            <CallerReference>persist-zone-1</CallerReference>
        </CreateHostedZoneRequest>"#,
    )
    .await;
    assert_eq!(created.status, 201, "CreateHostedZone failed: {}", created.body);
    let zone_id = created
        .first("HostedZone/Id")
        .unwrap()
        .trim_start_matches("/hostedzone/")
        .to_string();

    let changed = call(
        &before,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        r#"<ChangeResourceRecordSetsRequest xmlns="https://route53.amazonaws.com/doc/2013-04-01/">
            <ChangeBatch>
                <Changes>
                    <Change>
                        <Action>CREATE</Action>
                        <ResourceRecordSet>
                            <Name>api.app.test.</Name>
                            <Type>A</Type>
                            <TTL>60</TTL>
                            <ResourceRecords>
                                <ResourceRecord><Value>10.89.0.42</Value></ResourceRecord>
                            </ResourceRecords>
                        </ResourceRecordSet>
                    </Change>
                </Changes>
            </ChangeBatch>
        </ChangeResourceRecordSetsRequest>"#,
    )
    .await;
    assert_eq!(changed.status, 200, "ChangeResourceRecordSets failed: {}", changed.body);

    let tagged = call(
        &before,
        "POST",
        &format!("/2013-04-01/tags/hostedzone/{zone_id}"),
        r#"<ChangeTagsForResourceRequest xmlns="https://route53.amazonaws.com/doc/2013-04-01/">
            <AddTags>
                <Tag><Key>stack</Key><Value>foundation</Value></Tag>
            </AddTags>
        </ChangeTagsForResourceRequest>"#,
    )
    .await;
    assert_eq!(tagged.status, 200, "ChangeTagsForResource failed: {}", tagged.body);

    let r53_json = serde_json::to_string(&engine.state::<Route53State>().lock().snapshot()).unwrap();

    let (after, fresh) = router_with_engine();
    *fresh.state::<Route53State>().lock() =
        Route53State::restore(Route53State::VERSION, serde_json::from_str(&r53_json).unwrap())
            .unwrap();

    let records = call(
        &after,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        "",
    )
    .await;
    assert_eq!(records.status, 200);
    assert!(records.all("ResourceRecordSet/Name").contains(&"api.app.test."));
    assert!(records.all("ResourceRecord/Value").contains(&"10.89.0.42"));

    let tags = call(
        &after,
        "GET",
        &format!("/2013-04-01/tags/hostedzone/{zone_id}"),
        "",
    )
    .await;
    assert_eq!(tags.status, 200);
    assert_eq!(tags.first("Tag/Key"), Some("stack"));
    assert_eq!(tags.first("Tag/Value"), Some("foundation"));
}
