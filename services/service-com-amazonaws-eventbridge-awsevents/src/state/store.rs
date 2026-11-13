// SPDX-License-Identifier: CC0-1.0

//! The STORE's own methods: how every operation of this service reaches a
//! resource this emulator holds, and the clock-owed transitions a read may
//! finish.
//!
//! Moved out of `crate::state` unchanged, so one file holds one concern: the
//! records themselves and the sentences a refusal is worded with stay where the
//! module is declared ([`crate::state`]), and the accessors over them live here.

use super::*;

impl ComAmazonawsEventbridgeAwseventsState {
    /// How many REGIONS this store holds a bus record in.
    ///
    /// The keys are ordered by Region first ([`BusKey`]), so the Regions are the
    /// runs of equal Region in one walk of them.
    fn bus_region_count(&self) -> usize {
        let mut regions = 0usize;
        let mut last: Option<&str> = None;
        for key in self.buses.keys() {
            if last != Some(key.region.as_str()) {
                regions += 1;
                last = Some(key.region.as_str());
            }
        }
        regions
    }

    /// Whether this store may hold a bus in `region`.
    ///
    /// A Region it already holds one in always may; a NEW Region only while the
    /// store is below [`MAX_BUS_REGIONS`]. This is what bounds the store as a
    /// whole: the Region half of a bus's identity comes from the caller's own
    /// credential scope ([`request_region`]), so the per-Region bus and rule
    /// quotas bound each Region but not how many Regions a caller can open.
    pub fn may_hold_bus_in(&self, region: &str) -> bool {
        self.buses
            .keys()
            .any(|key| key.region == region)
            || self.bus_region_count() < MAX_BUS_REGIONS
    }

    /// Materialise the `default` event bus of `region`, if this store does not
    /// hold a record of it yet and [`Self::may_hold_bus_in`] admits the Region.
    ///
    /// Called only where a bus of `region` is WRITTEN
    /// ([`Self::bus_mut`], [`Self::insert_bus`]): a read resolves the Region's
    /// implicit default bus without writing anything
    /// ([`Self::implicit_default_bus_in`]), so no reporting operation of this
    /// service changes what a later caller sees.
    fn ensure_default_bus(&mut self, region: &str) {
        if !self.may_hold_bus_in(region) {
            return;
        }
        self.buses
            .entry(BusKey::new(region, DEFAULT_EVENT_BUS_NAME))
            .or_insert_with(|| EventBus::new(DEFAULT_EVENT_BUS_NAME, None));
    }

    /// The `default` bus of a Region this store holds no record of one for.
    ///
    /// The account's default bus is created with the account ("Events generated
    /// by Amazon Web Services services go to your account's default event
    /// bus"), in every Region, so it exists before any call is made and no call
    /// creates it — which Regions a run addresses, though, is only known once a
    /// request names one. A Region nothing has written yet therefore has a
    /// default bus carrying no creation instant ([`EventBus::created`]), no
    /// configuration and no rule, which is what this answers: one canonical
    /// record, so a READ of such a Region needs to write nothing at all.
    fn implicit_default_bus_in(&self, region: &str, name: &str) -> Option<&EventBus> {
        (name == DEFAULT_EVENT_BUS_NAME
            && !self
                .buses
                .contains_key(&BusKey::new(region, DEFAULT_EVENT_BUS_NAME)))
        .then_some(&self.implicit_default_bus)
    }

    /// The bus `name` names IN `region`, for a REPORTING operation: nothing is
    /// created, so a read leaves the store exactly as it found it — a Region
    /// whose `default` bus nothing has written yet answers the implicit one
    /// ([`Self::implicit_default_bus_in`]).
    pub fn bus(&self, region: &str, name: &str) -> Option<&EventBus> {
        self.buses
            .get(&BusKey::new(region, name))
            .or_else(|| self.implicit_default_bus_in(region, name))
    }

    /// The bus `name` names in `region`, mutably. Still creates no CUSTOM bus: a
    /// bus comes into existence through `CreateEventBus`, never through a rule
    /// write — only the Region's implicit `default` bus is materialised, because
    /// a write has to have a record to write to.
    pub fn bus_mut(&mut self, region: &str, name: &str) -> Option<&mut EventBus> {
        self.ensure_default_bus(region);
        self.buses.get_mut(&BusKey::new(region, name))
    }

    /// Every bus of `region`, in name order — which is what lets
    /// `ListEventBuses` resume from a page token. The Region's implicit
    /// `default` bus is among them, in its place in that order, whether or not
    /// anything has written a record of it.
    pub fn buses_in(&self, region: &str) -> Vec<&EventBus> {
        let mut buses: Vec<&EventBus> = self
            .buses
            .iter()
            .filter(|(key, _)| key.region == region)
            .map(|(_, bus)| bus)
            .chain(self.implicit_default_bus_in(region, DEFAULT_EVENT_BUS_NAME))
            .collect();
        buses.sort_by(|left, right| left.name.cmp(&right.name));
        buses
    }

    /// How many buses `region` holds, which is what the per-Region bus quota is
    /// charged against — the Region's `default` bus included, since it is one of
    /// the buses the account holds there.
    pub fn bus_count_in(&self, region: &str) -> usize {
        self.buses_in(region).len()
    }

    /// Write `bus` into `region` under its own name, beside the record of that
    /// Region's `default` bus — a Region this store holds a bus in holds the
    /// default bus it has as well.
    ///
    /// Stores nothing for a Region [`Self::may_hold_bus_in`] does not admit,
    /// which is the bound on the store as a whole; `CreateEventBus` charges that
    /// bound before it gets here, and reads the record back afterwards, so a
    /// refused create never answers as if it had stored one.
    pub fn insert_bus(&mut self, region: &str, bus: EventBus) {
        if !self.may_hold_bus_in(region) {
            return;
        }
        self.ensure_default_bus(region);
        let key = BusKey::new(region, &bus.name);
        self.buses.insert(key, bus);
    }

    /// Remove the bus `name` names in `region`, answering the record that was
    /// removed — and `None` when that Region held none under that name.
    pub fn remove_bus(&mut self, region: &str, name: &str) -> Option<EventBus> {
        self.buses.remove(&BusKey::new(region, name))
    }

    /// The rule `name` on bus `bus_name` in `region`, if both exist.
    pub fn rule(&self, region: &str, bus_name: &str, name: &str) -> Option<&RuleRecord> {
        self.bus(region, bus_name)?.rules.get(name)
    }

    /// The rule `name` on bus `bus_name` in `region`, mutably, if both exist.
    pub fn rule_mut(
        &mut self,
        region: &str,
        bus_name: &str,
        name: &str,
    ) -> Option<&mut RuleRecord> {
        self.bus_mut(region, bus_name)?.rules.get_mut(name)
    }

    /// The rule `name` on the bus `bus_name` in `region`, or WHY this store
    /// cannot resolve it.
    ///
    /// `EventBusName` is "the name or ARN of the event bus associated with the
    /// rule" (`DisableRule`'s member, and every other rule-scoped operation's), so
    /// it names the SCOPE the rule is looked up in rather than a property of the
    /// rule: the scope is resolved FIRST, and a bus this store holds no record of
    /// is the missing resource whatever rule name arrived with it
    /// ([`no_such_event_bus`]). Only on a bus that EXISTS can the rule name be the
    /// thing that is missing ([`no_such_rule`]). The two call for different
    /// remedies — a caller told to create a rule on a bus that is gone acts on the
    /// wrong resource — so the order is part of the answer and not an accident of
    /// how a handler happens to look things up.
    ///
    /// The ONE composer of both refusals, so a lookup and the message it fails
    /// with cannot drift apart: every operation that resolves a rule reaches a
    /// rule and a refusal through this ([`Self::resolve_rule_mut`] for a writer,
    /// [`Self::missing_rule`] for a caller that has to judge the scope before it
    /// judges the request's own members, and [`Self::missing_tagged_resource`] for
    /// one that reached the rule through an ARN).
    pub fn resolve_rule(
        &self,
        region: &str,
        bus_name: &str,
        name: &str,
    ) -> Result<&RuleRecord, String> {
        let Some(bus) = self.bus(region, bus_name) else {
            return Err(no_such_event_bus(bus_name));
        };
        bus.rules
            .get(name)
            .ok_or_else(|| no_such_rule(bus_name, name))
    }

    /// [`Self::resolve_rule`] for a WRITER: the same resolution, in the same
    /// order, answering the stored rule mutably.
    pub fn resolve_rule_mut(
        &mut self,
        region: &str,
        bus_name: &str,
        name: &str,
    ) -> Result<&mut RuleRecord, String> {
        let Some(bus) = self.bus_mut(region, bus_name) else {
            return Err(no_such_event_bus(bus_name));
        };
        bus.rules
            .get_mut(name)
            .ok_or_else(|| no_such_rule(bus_name, name))
    }

    /// Why this store cannot resolve the rule `name` on the bus `bus_name` in
    /// `region`, or `None` when it holds that rule.
    ///
    /// [`Self::resolve_rule`]'s answer without the rule, for a caller that has to
    /// resolve the resource BEFORE it judges the request's own members (so the
    /// rule it then writes to is fetched after those checks) and for one that
    /// reports on a rule it does not read.
    pub fn missing_rule(&self, region: &str, bus_name: &str, name: &str) -> Option<String> {
        self.resolve_rule(region, bus_name, name).err()
    }

    /// What a tagging path reports for a well-formed ARN naming a resource this
    /// store holds no record of.
    ///
    /// The message is built from the request through the resource the ARN NAMED
    /// ([`parse_tagged_resource`]), so a missing resource is identified by its
    /// kind and its own name — a rule by name and by the bus it was sought on, a
    /// bus by name — in exactly the words every other reader of that kind uses,
    /// rather than by echoing the ARN text back at the caller. The tag paths
    /// therefore report the same refusal for a missing rule as `DescribeRule`
    /// does, the precedence between the rule and its bus included
    /// ([`Self::missing_rule`]), whether the caller reached them through this
    /// service or through the tagging API that resolves an `events` ARN here.
    pub fn missing_tagged_resource(&self, resource: &TaggedResource) -> String {
        match resource {
            TaggedResource::EventBus { name, .. } => no_such_event_bus(name),
            TaggedResource::Rule {
                region,
                event_bus_name,
                name,
            } => self
                .missing_rule(region, event_bus_name, name)
                .unwrap_or_else(|| no_such_rule(event_bus_name, name)),
        }
    }

    /// The tag set of the resource an ARN named ([`parse_tagged_resource`]), for
    /// a REPORTING operation. `None` when this store holds no such resource,
    /// which a reader answers with the modelled missing-resource error rather
    /// than with an empty tag set.
    pub fn tags_of(&self, resource: &TaggedResource) -> Option<&Vec<Tag>> {
        match resource {
            TaggedResource::EventBus { region, name } => {
                self.bus(region, name).map(|bus| &bus.tags)
            }
            TaggedResource::Rule {
                region,
                event_bus_name,
                name,
            } => self
                .rule(region, event_bus_name, name)
                .map(|rule| &rule.tags),
        }
    }

    /// The same tag set, mutably: the ONE place a tag write reaches, so a tag
    /// write and a tag read can never resolve one ARN to two different records.
    /// Creates nothing — a resource comes into existence through its own create
    /// operation, never through a tag write.
    pub fn tags_of_mut(&mut self, resource: &TaggedResource) -> Option<&mut Vec<Tag>> {
        match resource {
            TaggedResource::EventBus { region, name } => {
                self.bus_mut(region, name).map(|bus| &mut bus.tags)
            }
            TaggedResource::Rule {
                region,
                event_bus_name,
                name,
            } => self
                .rule_mut(region, event_bus_name, name)
                .map(|rule| &mut rule.tags),
        }
    }

    /// Append one routed event to the log, evicting the oldest records first so
    /// the log never holds more than [`MAX_DELIVERED_EVENTS`] records or
    /// [`MAX_DELIVERED_BYTES`] bytes of event text.
    ///
    /// The ONE place the log grows: the eviction happens before the append, so
    /// both bounds hold however many events a caller publishes, and whatever
    /// size each one is.
    pub fn record_delivery(&mut self, event: DeliveredEvent) {
        let size = event.retained_bytes();
        while self.delivered.len() >= MAX_DELIVERED_EVENTS
            || (!self.delivered.is_empty()
                && self.delivered_bytes.saturating_add(size) > MAX_DELIVERED_BYTES)
        {
            let Some(evicted) = self.delivered.pop_front() else {
                break;
            };
            self.delivered_bytes = self
                .delivered_bytes
                .saturating_sub(evicted.retained_bytes());
        }
        self.delivered_bytes = self.delivered_bytes.saturating_add(size);
        self.delivered.push_back(event);
    }

    /// A fresh, unique `EventId` in the hyphenated 8-4-4-4-12 hex form
    /// EventBridge event ids carry.
    ///
    /// Uniqueness comes from `event_seq`, a per-engine counter, so two ids
    /// minted within one nanosecond still differ; the creation instant supplies
    /// the high bits so ids from separate runs do not collide.
    pub fn next_event_id(&mut self, now: SystemTime) -> String {
        self.next_unique_id(now)
    }

    /// A fresh, unique identifier in the hyphenated 8-4-4-4-12 hex form, the
    /// ONE id mint of this store: an event id and the id segment of a
    /// connection's ARN come from the same counter, so no two resources of one
    /// run can be handed the same identifier.
    pub fn next_unique_id(&mut self, now: SystemTime) -> String {
        self.event_seq = self.event_seq.wrapping_add(1);
        let nanos = u64::try_from(
            now.duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
        let seq = self.event_seq;
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (nanos >> 32) as u32,
            ((nanos >> 16) & 0xffff) as u16,
            (nanos & 0xffff) as u16,
            ((seq >> 48) & 0xffff) as u16,
            seq & 0xffff_ffff_ffff,
        )
    }

    /// Append one target invocation to the log, evicting the oldest records
    /// first so it never holds more than [`MAX_TARGET_INVOCATIONS`].
    ///
    /// The ONE place that log grows.
    pub fn record_invocation(&mut self, invocation: TargetInvocation) {
        while self.invocations.len() >= MAX_TARGET_INVOCATIONS {
            if self.invocations.pop_front().is_none() {
                break;
            }
        }
        self.invocations.push_back(invocation);
    }

    /// The connection `name` names, for a REPORTING operation.
    pub fn connection(&self, name: &str) -> Option<&ConnectionRecord> {
        self.connections.get(name)
    }

    /// The connection `name` names, mutably. Creates nothing: a connection
    /// comes into existence through `CreateConnection`.
    pub fn connection_mut(&mut self, name: &str) -> Option<&mut ConnectionRecord> {
        self.connections.get_mut(name)
    }

    /// The name of the rule the ARN a tagging call named resolves to, when that
    /// rule is a MANAGED one — and `None` for every other resource.
    ///
    /// "You cannot modify these managed rules by using ... TagResource, or
    /// UntagResource" (`ManagedRuleException`), and a tagging call reaches a rule
    /// through an ARN ([`parse_tagged_resource`]), so the check both tag writes
    /// make lives here rather than in each of them.
    pub fn managed_rule_of(&self, resource: &TaggedResource) -> Option<String> {
        let TaggedResource::Rule {
            region,
            event_bus_name,
            name,
        } = resource
        else {
            return None;
        };
        self.rule(region, event_bus_name, name)
            .filter(|rule| rule.managed_by.is_some())
            .map(|rule| rule.name.clone())
    }

    /// The connection whose ARN is `arn`, for a REPORTING operation.
    ///
    /// A connection's ARN carries a unique id segment ([`connection_arn`]), so an
    /// ARN resolves to the connection it was minted for and NOT to a later
    /// connection of the same name — which is what makes an API destination
    /// holding the ARN of a deleted connection resolve to nothing.
    pub fn connection_by_arn(&self, arn: &str) -> Option<&ConnectionRecord> {
        self.connections
            .values()
            .find(|connection| connection.arn == arn)
    }

    /// The archive `name` names, for a REPORTING operation.
    pub fn archive(&self, name: &str) -> Option<&ArchiveRecord> {
        self.archives.get(name)
    }

    /// The archive `name` names, mutably. Creates nothing: an archive comes into
    /// existence through `CreateArchive`.
    pub fn archive_mut(&mut self, name: &str) -> Option<&mut ArchiveRecord> {
        self.archives.get_mut(name)
    }

    /// Materialise every archive change the clock has ALREADY made due at `now`,
    /// and answer how many archives one was applied to.
    ///
    /// One change is owed by the clock alone: the expiry of a captured event past
    /// the archive's `RetentionDays` ([`ArchiveRecord::expire_captured`]). It was
    /// owed at the same instant even if nobody had called, which is why a
    /// reporting operation may finish it. An archive's `ArchiveState` is NOT
    /// clock-owed: a write settles it ([`ARCHIVE_STATE_ENABLED`]).
    pub fn settle_archives(&mut self, now: SystemTime) -> usize {
        let mut settled = 0usize;
        for archive in self.archives.values_mut() {
            settled += usize::from(archive.expire_captured(now) > 0);
        }
        settled
    }

    /// The replay `name` names, for a REPORTING operation.
    pub fn replay(&self, name: &str) -> Option<&ReplayRecord> {
        self.replays.get(name)
    }

    /// The replay `name` names, mutably. Creates nothing: a replay comes into
    /// existence through `StartReplay`.
    pub fn replay_mut(&mut self, name: &str) -> Option<&mut ReplayRecord> {
        self.replays.get_mut(name)
    }

    /// Materialise every replay change the clock has ALREADY made due at `now`,
    /// and answer how many replays one was applied to.
    ///
    /// Each change is owed by the clock alone: the non-terminal state a call left
    /// the replay in moves on [`REPLAY_TRANSITION`] after that call, along the
    /// progression [`next_replay_state`] lays out
    /// ([`ReplayRecord::settle`]). It was owed at the same instant even if nobody
    /// had called, which is why a reporting operation may finish it.
    pub fn settle_replays(&mut self, now: SystemTime) -> usize {
        let mut settled = 0usize;
        for replay in self.replays.values_mut() {
            settled += usize::from(replay.settle(now));
        }
        settled
    }

    /// The API destination `name` names, for a REPORTING operation.
    pub fn api_destination(&self, name: &str) -> Option<&ApiDestinationRecord> {
        self.api_destinations.get(name)
    }

    /// The API destination `name` names, mutably. Creates nothing: a destination
    /// comes into existence through `CreateApiDestination`.
    pub fn api_destination_mut(&mut self, name: &str) -> Option<&mut ApiDestinationRecord> {
        self.api_destinations.get_mut(name)
    }

    /// Remove the API destination `name` names, answering the record that was
    /// removed — and `None` when this store held none under that name, which is
    /// what makes a second delete a missing-resource refusal rather than a
    /// second removal.
    pub fn remove_api_destination(&mut self, name: &str) -> Option<ApiDestinationRecord> {
        self.api_destinations.remove(name)
    }

    /// The `ApiDestinationState` of `destination`, as the destination holds it.
    ///
    /// "The destination endpoint must support the authorization type specified
    /// for the connection" (`CreateApiDestination$ConnectionArn`): a destination
    /// can be invoked only while its connection authorizes the invocation, so
    /// the state is the authorization of the connection it invokes through
    /// ([`api_destination_state_of`]) — taken when the destination is written or
    /// repointed ([`Self::api_destination_state_on`]) and again when that
    /// connection's own transition settles under it
    /// ([`Self::settle_connections`]), which is what makes a
    /// `DeauthorizeConnection` reach the destinations invoking through it.
    ///
    /// It is NOT recomputed on a read, because the reference a destination holds
    /// can outlive the connection: deleting a connection an API destination
    /// still names is permitted and does not revoke that destination, which goes
    /// on reporting the state it had with a now-dangling `ConnectionArn`. A read
    /// that re-derived the state would report a revocation the service never
    /// performed.
    pub fn api_destination_state(&self, destination: &ApiDestinationRecord) -> String {
        destination.state.clone()
    }

    /// The `ApiDestinationState` a destination invoking through the connection
    /// `connection_arn` is WRITTEN with.
    ///
    /// `ApiDestinationState` is modelled `ACTIVE | INACTIVE` and has no value for
    /// "settling", so what the destination takes is the authorization its
    /// connection SETTLES on ([`ConnectionRecord::settled_state`]) rather than
    /// the transient it is passing through: a connection on its way to holding
    /// authorization leaves the destination `ACTIVE`, and one that ends up with
    /// none — never obtained, stripped, on its way out, or no such connection at
    /// all — makes it `INACTIVE`.
    pub fn api_destination_state_on(&self, connection_arn: &str) -> String {
        self.connection_by_arn(connection_arn)
            .and_then(ConnectionRecord::settled_state)
            .map_or_else(
                || API_DESTINATION_STATE_INACTIVE.to_string(),
                api_destination_state_of,
            )
    }

    /// Every member `DescribeApiDestination` reports, read back from the stored
    /// record, with the state derived from the connection
    /// ([`Self::api_destination_state`]).
    pub fn to_api_destination_output(
        &self,
        destination: &ApiDestinationRecord,
    ) -> DescribeApiDestinationOutput {
        DescribeApiDestinationOutput {
            api_destination_arn: Some(destination.arn.clone()),
            name: Some(destination.name.clone()),
            description: destination.description.clone(),
            api_destination_state: Some(self.api_destination_state(destination)),
            connection_arn: Some(destination.connection_arn.clone()),
            invocation_endpoint: Some(destination.invocation_endpoint.clone()),
            http_method: Some(destination.http_method.clone()),
            invocation_rate_limit_per_second: Some(destination.invocation_rate_limit_per_second),
            creation_time: Some(to_wire_timestamp(destination.created)),
            last_modified_time: Some(to_wire_timestamp(destination.last_modified)),
        }
    }

    /// The `ApiDestination` summary a LISTING reports, projected from the SAME
    /// stored record [`Self::to_api_destination_output`] reports — state included,
    /// so a listing and a read of one destination cannot disagree.
    pub fn to_api_destination_summary(
        &self,
        destination: &ApiDestinationRecord,
    ) -> ApiDestinationSummary {
        ApiDestinationSummary {
            api_destination_arn: Some(destination.arn.clone()),
            name: Some(destination.name.clone()),
            api_destination_state: Some(self.api_destination_state(destination)),
            connection_arn: Some(destination.connection_arn.clone()),
            invocation_endpoint: Some(destination.invocation_endpoint.clone()),
            http_method: Some(destination.http_method.clone()),
            invocation_rate_limit_per_second: Some(destination.invocation_rate_limit_per_second),
            creation_time: Some(to_wire_timestamp(destination.created)),
            last_modified_time: Some(to_wire_timestamp(destination.last_modified)),
        }
    }

    /// The global endpoint `name` names, for a REPORTING operation.
    pub fn endpoint(&self, name: &str) -> Option<&EndpointRecord> {
        self.endpoints.get(name)
    }

    /// The global endpoint `name` names as a reader addressing `home_region`
    /// sees it, for a REPORTING operation.
    ///
    /// `HomeRegion` is "the primary Region of the endpoint you want to get
    /// information about" (`DescribeEndpoint$HomeRegion`) — it SELECTS which
    /// endpoint the call is about rather than filtering the answer — so a
    /// reader that names a Region the endpoint was not created in reads nothing,
    /// exactly as it would for a name no endpoint carries. A reader that names
    /// no Region is asking about the endpoint of that name wherever it lives.
    pub fn endpoint_in_home_region(
        &self,
        name: &str,
        home_region: Option<&str>,
    ) -> Option<&EndpointRecord> {
        self.endpoint(name)
            .filter(|record| home_region.is_none_or(|region| record.home_region == region))
    }

    /// The global endpoint `name` names, mutably, for a WRITING operation.
    /// Creates nothing: an endpoint comes into existence through
    /// `CreateEndpoint`, never through a write to one that does not exist.
    pub fn endpoint_mut(&mut self, name: &str) -> Option<&mut EndpointRecord> {
        self.endpoints.get_mut(name)
    }

    /// Remove the global endpoint `name` names, answering the record that was
    /// removed — and `None` when this store held none under that name.
    ///
    /// Mirrors [`Self::remove_api_destination`]: the removal is keyed by the
    /// endpoint's name, which is its identity, so the name holds nothing
    /// afterwards and is free for another create.
    pub fn remove_endpoint(&mut self, name: &str) -> Option<EndpointRecord> {
        self.endpoints.remove(name)
    }

    /// Materialise every global-endpoint transition the clock has ALREADY made
    /// due at `now`, and answer how many were applied.
    ///
    /// The change is owed by the clock alone: an endpoint answered
    /// [`ENDPOINT_STATE_CREATING`] or [`ENDPOINT_STATE_UPDATING`] reaches
    /// [`ENDPOINT_STATE_ACTIVE`] [`ENDPOINT_TRANSITION`] after the write that
    /// left it there, and was owed at the same instant even if nobody had
    /// called — which is why a reporting operation may finish it.
    pub fn settle_endpoints(&mut self, now: SystemTime) -> usize {
        let mut settled = 0usize;
        for endpoint in self.endpoints.values_mut() {
            let Some(due) = endpoint.active_due.filter(|due| *due <= now) else {
                continue;
            };
            // The state moves to the instant the transition fell DUE, not to
            // the instant it was noticed, so two reads agree.
            endpoint.active_due = None;
            endpoint.state = ENDPOINT_STATE_ACTIVE.to_string();
            endpoint.last_modified = due;
            settled += 1;
        }
        settled
    }

    /// Whether this account may author partner event sources in `namespace`,
    /// which is what every authoring call of the partner slice is checked
    /// against (see [`Self::partner_namespaces`]).
    pub fn may_author_partner_namespace(&self, namespace: &str) -> bool {
        self.partner_namespaces.contains(namespace)
    }

    /// Why this account may not author the partner event source `name` names, or
    /// `None` when it may.
    ///
    /// The ONE check every authoring operation of the partner slice makes, and
    /// the FIRST thing each of them does: a caller that is not the namespace's
    /// registered SaaS partner is told that and nothing else — not whether the
    /// source exists, not which accounts it was offered to — so one partner
    /// cannot probe another's namespace. `name` may be a whole source name or a
    /// name prefix; both carry the namespace in the same leading segments.
    pub fn partner_authoring_refusal(&self, name: &str) -> Option<String> {
        let namespace = partner_namespace(name).unwrap_or_else(|| name.to_string());
        (!self.may_author_partner_namespace(&namespace))
            .then(|| not_entitled_to_namespace(&namespace))
    }

    /// Install the registration that makes this account the SaaS partner of
    /// `namespace`, answering whether it was not already held.
    ///
    /// The ONE way [`Self::partner_namespaces`] grows: partner registration
    /// happens outside EventBridge, so nothing a client can call reaches here.
    pub fn register_partner_namespace(&mut self, namespace: &str) -> bool {
        self.partner_namespaces.insert(namespace.to_string())
    }

    /// The partner event source `name` names, for a REPORTING operation.
    pub fn partner_source(&self, name: &str) -> Option<&PartnerEventSourceRecord> {
        self.partner_sources.get(name)
    }

    /// The partner event source `name` names, mutably. Creates nothing: a source
    /// comes into existence through `CreatePartnerEventSource`.
    pub fn partner_source_mut(&mut self, name: &str) -> Option<&mut PartnerEventSourceRecord> {
        self.partner_sources.get_mut(name)
    }

    /// The source `name` names IF it is shared with `account` — the customer-side
    /// view, which is what `DescribeEventSource` reports ("a partner event source
    /// that is shared with your account") and what `ActivateEventSource` and
    /// `DeactivateEventSource` act on.
    ///
    /// A source authored for another account is not shared with this one, so it
    /// reads exactly like a source that was never authored at all: a customer
    /// learns nothing about another customer's sources.
    pub fn event_source_shared_with(
        &self,
        name: &str,
        account: &str,
    ) -> Option<&PartnerEventSourceRecord> {
        self.partner_source(name)
            .filter(|record| record.account == account)
    }

    /// The `EventSourceState` a source is in, DERIVED from the source record and
    /// the bus slice: `DELETED` once the partner deleted it, `ACTIVE` while the
    /// matching bus exists and is receiving its events, and `PENDING` otherwise —
    /// the three cases `DescribeEventSource$State` documents.
    pub fn event_source_state(&self, record: &PartnerEventSourceRecord) -> &'static str {
        if record.deleted {
            EVENT_SOURCE_STATE_DELETED
        } else if record.activated && self.matching_bus(&record.name).is_some() {
            EVENT_SOURCE_STATE_ACTIVE
        } else {
            EVENT_SOURCE_STATE_PENDING
        }
    }

    /// The partner event bus matched with the source `name`: "for partner event
    /// buses, the name must exactly match the name of the partner event source
    /// that this event bus is matched to" (`CreateEventBus$Name`), so the match
    /// is on the bus's own `event_source_name`.
    ///
    /// A partner event source is not a Region-keyed resource of this store (it
    /// is offered to an ACCOUNT), so the bus it is matched with is sought
    /// wherever that account created it rather than in one Region.
    pub fn matching_bus(&self, name: &str) -> Option<&EventBus> {
        self.buses
            .values()
            .find(|bus| bus.name == name && bus.event_source_name.as_deref() == Some(name))
    }

    /// The accounts a partner event source was offered to, as
    /// `ListPartnerEventSourceAccounts` reports them — one per source, because
    /// "each partner event source can be used by one Amazon Web Services account"
    /// (`CreatePartnerEventSource`).
    pub fn partner_source_accounts(
        &self,
        record: &PartnerEventSourceRecord,
    ) -> Vec<PartnerEventSourceAccount> {
        vec![PartnerEventSourceAccount {
            account: Some(record.account.clone()),
            creation_time: Some(to_wire_timestamp(record.created)),
            expiration_time: record.expiration().map(to_wire_timestamp),
            state: Some(self.event_source_state(record).to_string()),
        }]
    }

    /// Materialise every partner-event-source expiry the clock has ALREADY made
    /// due at `now`, and answer how many records went away.
    ///
    /// "If it remains in PENDING state for more than two weeks, it is deleted"
    /// (`DeactivateEventSource`): a source nobody matched with a bus within
    /// [`PARTNER_EVENT_SOURCE_EXPIRY`] of entering `PENDING` is gone. It was owed
    /// at the same instant even if nobody had called, which is why a reporting
    /// operation may apply it.
    pub fn settle_partner_sources(&mut self, now: SystemTime) -> usize {
        let due: Vec<String> = self
            .partner_sources
            .values()
            .filter(|record| {
                !record.deleted && record.expiration().is_some_and(|expires| expires <= now)
            })
            .map(|record| record.name.clone())
            .collect();
        for name in &due {
            self.partner_sources.remove(name);
        }
        due.len()
    }

    /// Materialise every connection transition the clock has ALREADY made due
    /// at `now`, and answer how many were applied.
    ///
    /// A connection AWS answers in `CREATING`, `DEAUTHORIZING` or `DELETING`
    /// reaches its next state on its own, [`CONNECTION_TRANSITION`] after the
    /// call that started it; the due instant is stored, never waited for. This
    /// is the only write a reporting operation of the connection slice makes: it
    /// finishes a change the clock already owed, which was owed at the same
    /// instant even if nobody had called.
    ///
    /// A connection that settles on a new authorization carries every API
    /// destination invoking through it with it ([`api_destination_state_of`]):
    /// that is how a `DeauthorizeConnection` makes its destinations `INACTIVE`
    /// and a later reauthorization makes them `ACTIVE` again. A connection that
    /// settles by being REMOVED carries none of them: deleting a connection an
    /// API destination still names is permitted and does not revoke the
    /// destination, which keeps the state it had and a dangling `ConnectionArn`.
    pub fn settle_connections(&mut self, now: SystemTime) -> usize {
        let due: Vec<String> = self
            .connections
            .values()
            .filter(|connection| {
                connection
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.due <= now)
            })
            .map(|connection| connection.name.clone())
            .collect();
        // (the ARN of a connection that settled on a state, the
        //  `ApiDestinationState` the destinations invoking through it take from
        //  it) — collected while each connection is written and applied after,
        //  so one walk of the destinations serves them all.
        let mut settled_authorizations: Vec<(String, String)> = Vec::new();
        for name in &due {
            let Some(connection) = self.connections.get_mut(name) else {
                continue;
            };
            let Some(pending) = connection.pending.take() else {
                continue;
            };
            match pending.to {
                ConnectionTransition::Removed => {
                    self.connections.remove(name);
                }
                ConnectionTransition::To { state, reason } => {
                    // The stamps move to the instant the transition fell DUE,
                    // not to the instant it was noticed, so two reads of a
                    // settled connection report the same timestamps.
                    if state == CONNECTION_STATE_AUTHORIZED {
                        connection.last_authorized = Some(pending.due);
                    }
                    connection.last_modified = pending.due;
                    connection.state = state;
                    // The reason belongs to the state the connection reaches, so
                    // a state that needs none reports none.
                    connection.state_reason = reason;
                    settled_authorizations.push((
                        connection.arn.clone(),
                        api_destination_state_of(&connection.state),
                    ));
                }
            }
        }
        for (connection_arn, state) in settled_authorizations {
            for destination in self.api_destinations.values_mut() {
                if destination.connection_arn == connection_arn {
                    destination.state = state.clone();
                }
            }
        }
        due.len()
    }
}
