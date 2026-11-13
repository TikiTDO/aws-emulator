// SPDX-License-Identifier: CC0-1.0

//! The SDI SOURCE slice: the SdiSource record, which inputs use one, and the
//! projections and the delete the SdiSource operations answer with.
//!
//! One file for the whole slice, so the operations that read and write an
//! SdiSource cannot drift into several readings of one rule. Moved here
//! unchanged from the state root, which re-exports it
//! (`pub use sdi_sources::*;`), so every `crate::state::…` path a handler
//! already uses still resolves.

use super::*;

/// One created SdiSource, as `CreateSdiSource` stored it and `UpdateSdiSource`
/// last changed it.
///
/// The members mirror the model's `SdiSource` shape, minus the two it derives:
/// `Inputs` and `State` come from what else is in the store (see
/// [`ComAmazonawsMedialiveMedialiveState::inputs_using_sdi_source`]) and from
/// whether the source has been deleted, never from a field written at create
/// time.
#[derive(Debug, Clone)]
pub struct SdiSourceRecord {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub mode: Option<String>,
    pub r#type: Option<String>,
    /// The request's `Tags`. `CreateSdiSource` accepts them but neither
    /// `SdiSource` nor `SdiSourceSummary` carries a `Tags` member
    /// (`smithy_query(query="shape", shape="SdiSource")`), so the tag
    /// operations are what report them: `ListTagsForResource` reads this map
    /// through [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
    pub tags: HashMap<String, String>,
    /// Whether `DeleteSdiSource` has deleted this source.
    ///
    /// A deleted SdiSource is NOT dropped from the account: `DELETED` is one of
    /// the three states the model gives it (`smithy_query(query="shape",
    /// shape="SdiSourceState")`: IDLE | IN_USE | DELETED), and it is terminal,
    /// so the record stays readable in that state rather than vanishing. PRIVATE,
    /// so the only way into it is
    /// [`ComAmazonawsMedialiveMedialiveState::delete_sdi_source`] and the only way
    /// to read it is [`SdiSourceRecord::is_deleted`].
    pub(super) deleted: bool,
}

impl SdiSourceRecord {
    /// Whether this source has been deleted, which is what puts it in the
    /// terminal `DELETED` state.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }
}

/// Which half of `DeleteSdiSource`'s documented precondition a source fails —
/// "The SdiSource must not be part of any SidSourceMapping and must not be
/// attached to any input" — so the refusal can say which of the two it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdiSourceInUse {
    /// At least one input of this account ingests from the source.
    AttachedToInput,
    /// At least one node of this account maps a card port to the source
    /// through its `SdiSourceMappings`.
    PartOfSdiSourceMapping,
}

/// The message `DeleteSdiSource` refuses a source still in use with.
///
/// It names the `SdiSourceId` the CALLER supplied and which half of the
/// documented precondition the source fails, and nothing else: the inputs
/// attached to it and the nodes mapping it are state the caller did not name
/// (they are what the source's own `Inputs` and the nodes' own
/// `SdiSourceMappings` report), so they are not interpolated, for the same
/// reason [`super::busy_input_message`] leaves the attaching channels out of an
/// input's refusal.
pub fn sdi_source_in_use_message(sdi_source_id: &str, reason: SdiSourceInUse) -> String {
    match reason {
        SdiSourceInUse::AttachedToInput => {
            format!("SdiSource {sdi_source_id} is attached to an input and cannot be deleted")
        }
        SdiSourceInUse::PartOfSdiSourceMapping => {
            format!("SdiSource {sdi_source_id} is part of an SdiSourceMapping and cannot be deleted")
        }
    }
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Whether the SdiSource store is full, so `CreateSdiSource` must refuse
    /// rather than mint another source.
    pub fn sdi_sources_full(&self) -> bool {
        self.sdi_sources.len() >= MAX_SDI_SOURCES
    }

    /// The next SdiSource id, as AWS shapes MediaLive resource ids: a numeric
    /// string.
    pub fn mint_sdi_source_id(&mut self) -> String {
        (FIRST_SDI_SOURCE_ID + self.mints.next(|mints| &mut mints.sdi_source)).to_string()
    }

    /// Record `token` as having created `sdi_source_id`.
    pub fn record_sdi_source_token(&mut self, token: String, sdi_source_id: &str) {
        self.sdi_source_tokens
            .record(token, sdi_source_id, MAX_SDI_SOURCES);
    }

    /// The SdiSource a stored `RequestId` created, if that source is still one a
    /// caller may use.
    ///
    /// LIVENESS is part of the lookup, through the same
    /// [`SdiSourceRecord::is_deleted`] every other resolution of a source
    /// applies: the delete keeps the record readable, but a token's idempotency
    /// window is over once the source it minted has reached the terminal
    /// `DELETED` state — the name it carried is already free for a new source —
    /// so the retried create mints one of its own rather than answering with a
    /// record the caller could neither use nor change.
    pub fn sdi_source_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.sdi_source_tokens.created(request_id)?;
        self.sdi_sources
            .get(id)
            .filter(|source| !source.is_deleted())
            .map(|source| source.id.as_str())
    }

    /// The id of the SdiSource whose `Name` is `name`, if the store holds one.
    ///
    /// `CreateSdiSource`'s and `UpdateSdiSource`'s `Name` carry the same
    /// documented rule as the cluster's and the network's own ("Specify a name
    /// that is unique in the AWS account… Names are case-sensitive"), so the
    /// match is EXACT (never case-folded) and spans every source this store
    /// holds, and it is what makes a create — or a rename — under a name the
    /// account already holds the modelled `ConflictException` rather than a
    /// second source nothing can tell apart by the name the model says
    /// identifies it. This is the ONE lookup both of them resolve against, so
    /// the two cannot disagree about what a stored source may be called
    /// ([`Self::network_named`] plays the same part on the network side).
    ///
    /// A deleted source is skipped: `DELETED` is terminal, so the record stays
    /// readable but the source is out of the account and the name it was created
    /// under is free for the next create.
    pub fn sdi_source_named(&self, name: &str) -> Option<&str> {
        self.sdi_sources
            .values()
            .find(|source| source.name.as_deref() == Some(name) && !source.is_deleted())
            .map(|source| source.id.as_str())
    }

    /// The inputs that are currently using the SdiSource `id` - the model's
    /// `Inputs` member ("The list of inputs that are currently using this SDI
    /// source. This list will be empty if the SdiSource has just been deleted").
    ///
    /// Derived from the input store: an input names the sources it ingests from
    /// in `CreateInput`'s `SdiSources` (`InputSdiSources`, "SDI Sources for this
    /// Input"), whose entries are SdiSource ids, so this is every input id whose
    /// list names `id`, in input-id order. A deleted source has nothing using it
    /// any more, which is what makes its `Inputs` empty.
    pub(super) fn inputs_using_sdi_source(&self, id: &str) -> Vec<String> {
        if self
            .sdi_sources
            .get(id)
            .is_some_and(SdiSourceRecord::is_deleted)
        {
            return Vec::new();
        }
        self.inputs
            .values()
            // The one cut point every derived "currently using" list of this
            // store applies ([`Self::inputs_using_security_group`]): accepting
            // an input's delete releases the sources it ingested from, so the
            // input leaves this list at that instant — the transient `DELETING`
            // step included, exactly as it leaves [`Self::channels_attaching`].
            .filter(|input| !input.is_delete_accepted())
            .filter(|input| input.sdi_sources.iter().any(|source| source == id))
            .map(|input| input.id.clone())
            .collect()
    }

    /// Which half of `DeleteSdiSource`'s documented precondition the SdiSource
    /// `id` fails, or `None` when it fails neither and the delete may go ahead.
    ///
    /// "The SdiSource must not be part of any SidSourceMapping and must not be
    /// attached to any input" is ONE precondition over TWO stores, so both are
    /// walked here: the inputs that ingest from the source
    /// ([`Self::inputs_using_sdi_source`]) and the nodes whose
    /// `SdiSourceMappings` connect a card port to it
    /// ([`Self::nodes_mapping_sdi_source`]). Either referrer makes the delete
    /// the modelled conflict, because deleting the source would leave that
    /// referrer naming a source the account no longer holds — a configuration
    /// this service's own writers refuse to create.
    ///
    /// A source already in the terminal `DELETED` state fails neither half:
    /// nothing live can reference it any more, which is what keeps a repeated
    /// delete idempotent rather than turning it into a conflict.
    pub fn sdi_source_in_use(&self, id: &str) -> Option<SdiSourceInUse> {
        if self
            .sdi_sources
            .get(id)
            .is_some_and(SdiSourceRecord::is_deleted)
        {
            return None;
        }
        if !self.inputs_using_sdi_source(id).is_empty() {
            return Some(SdiSourceInUse::AttachedToInput);
        }
        if !self.nodes_mapping_sdi_source(id).is_empty() {
            return Some(SdiSourceInUse::PartOfSdiSourceMapping);
        }
        None
    }

    /// The full `SdiSource` description of one stored source, or `None` when no
    /// source has that id.
    ///
    /// The ONE projection every operation that reports an SdiSource
    /// (`CreateSdiSource`, `DescribeSdiSource`, `UpdateSdiSource`,
    /// `DeleteSdiSource`) answers from, so none of them can drift from the
    /// others. `Tags` is always a map — the source's stored tag set, empty when
    /// it carries none — exactly as every other taggable MediaLive resource
    /// projection reports it ([`Self::input_description`],
    /// [`Self::channel_description`]): an absent member would make a caller that
    /// iterates the returned source's tags dereference null instead of reading
    /// an empty map.
    pub fn sdi_source_description(&self, id: &str) -> Option<SdiSource> {
        let source = self.sdi_sources.get(id)?;
        let inputs = self.inputs_using_sdi_source(id);
        Some(SdiSource {
            arn: Some(source.arn.clone()),
            id: Some(source.id.clone()),
            state: Some(sdi_source_state_for(source, &inputs).to_string()),
            inputs: Some(inputs),
            mode: source.mode.clone(),
            name: source.name.clone(),
            tags: Some(source.tags.clone()),
            r#type: source.r#type.clone(),
        })
    }

    /// Delete the SdiSource `id` names and answer it as the delete left it, or
    /// `None` when the store holds no source with that id.
    ///
    /// The source moves to the terminal `DELETED` state and STAYS in the
    /// account, so `DescribeSdiSource` and `ListSdiSources` keep reporting it
    /// there rather than losing it. Idempotent — deleting an already deleted
    /// source changes nothing.
    ///
    /// A token never outlives the resource it minted: the source's idempotency
    /// window is over the moment it reaches that terminal state, so the tokens
    /// that created it are dropped here and a create retried with one of them
    /// mints a new source instead of answering with the deleted one — the rule
    /// the channel, input, network and cluster tokens follow when their own
    /// deletes come due.
    ///
    /// The state write and the projection live together here, so the response
    /// reports the source that was stored rather than one a handler assembled.
    pub fn delete_sdi_source(&mut self, id: &str) -> Option<SdiSource> {
        self.sdi_sources.get_mut(id)?.deleted = true;
        self.sdi_source_tokens.forget(id);
        self.sdi_source_description(id)
    }

    /// The `SdiSourceSummary` projection of one stored source. The summary shape
    /// carries exactly the `SdiSource` members
    /// (`smithy_query(query="shape", shape="SdiSourceSummary")`), so it is the
    /// same stored record read through the list shape.
    pub fn sdi_source_summary(&self, source: &SdiSourceRecord) -> SdiSourceSummary {
        let inputs = self.inputs_using_sdi_source(&source.id);
        SdiSourceSummary {
            arn: Some(source.arn.clone()),
            id: Some(source.id.clone()),
            state: Some(sdi_source_state_for(source, &inputs).to_string()),
            inputs: Some(inputs),
            mode: source.mode.clone(),
            name: source.name.clone(),
            r#type: source.r#type.clone(),
        }
    }
}
