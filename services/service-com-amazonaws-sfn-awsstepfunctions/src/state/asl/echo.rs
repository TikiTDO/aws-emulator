// SPDX-License-Identifier: CC0-1.0

//! How a document a walk REPORTS is encoded.
//!
//! One concern, held in one file: a JSON document has many encodings, and two of them
//! differ in more than the whitespace between their tokens. A number is parsed into a
//! binary double, so an integer with more digits than a double holds — an id, an
//! amount, a counter past 2^53 — comes back from a parsed copy in that double's own
//! shortest form rather than with the digits it was written with, which no client can
//! detect from the response.
//!
//! So a document this service was GIVEN AS TEXT is reported in that text rather than
//! in a re-encoding of a parsed copy, and only a document the definition BUILT is
//! reported in the encoding the walk produced it in. Two kinds of text are given:
//!
//! * the execution INPUT, which is a caller's own bytes — reported byte for byte,
//!   their whitespace and member order included, so a payload a caller hashes, signs
//!   or diffs against what it sent comes back unchanged;
//! * every DATA document written inside the DEFINITION — a `Pass` state's `Result`,
//!   a literal member of a payload template, an item of a `Map`'s `Items` — reported
//!   with the insignificant whitespace between its tokens dropped and nothing else
//!   about it changed ([`::service_common::json_document::minified`], this
//!   workspace's one statement of that rule), so its member order and every number
//!   literal it carries are the ones the definition wrote. A member that CONFIGURES a
//!   state rather than being data it puts out ([`DATA_MEMBERS`]) is not remembered at
//!   all.

use super::*;

use serde_json::value::RawValue;

/// The members of a state whose value is a DATA document — one a state can carry
/// through, or whose literal members become part of one.
///
/// Every other member CONFIGURES the state rather than being data it puts out: a
/// timeout, a backoff rate, a comment, a `Choice` comparator's operand. Their text is
/// not remembered, because substituting it for a document a LATER state computed to
/// the same value would report a literal that state never wrote.
const DATA_MEMBERS: &[&str] = &[
    "Arguments",
    "Items",
    "ItemSelector",
    "Output",
    "Parameters",
    "Result",
    "ResultSelector",
];

/// The members a state nests another state list under: a `Map`'s processor, in either
/// spelling, and a `Parallel`'s ARRAY of branches.
const NESTED_MEMBERS: &[&str] = &["ItemProcessor", "Iterator", "Branches"];

/// The most documents one walk remembers the text of.
///
/// Emulator bound, NOT an AWS quota: the definition is a caller's, bounded only at the
/// modelled 1 MiB, and every document nested inside one is a document of its own, so
/// the number of them is bounded here. A walk past this reports the documents it did
/// not remember in the encoding it produced them in, exactly as it reports a document
/// the definition built.
pub(super) const MAX_SUPPLIED_DOCUMENTS: usize = 512;

/// The most bytes of definition text one walk SCANS looking for them.
///
/// Emulator bound, for the same reason, and on the work rather than on what is kept: a
/// document nested inside another is scanned with it, so the cost is the product of the
/// definition's size and how deeply it nests rather than its size alone, and most nodes
/// are scanned without being kept. This keeps that product inside a small multiple of
/// one definition's own modelled ceiling.
pub(super) const MAX_SUPPLIED_SCAN_BYTES: usize = 4_194_304;

/// The documents a walk was GIVEN as text, each with the bytes it is REPORTED in.
///
/// Keyed by the CANONICAL encoding of the document — what `serde_json` answers for the
/// parsed value — so answering for one document is a single lookup however many the
/// definition writes. A document whose canonical encoding IS the text it is reported
/// in is not held at all: re-encoding it answers the same bytes, so there is nothing
/// to put back.
///
/// Two documents of one definition can share a canonical encoding and differ in the
/// text they were written as — two numbers no double tells apart, two member orders —
/// and then neither may be put back, because reporting one's text for the other would
/// answer a literal that state never wrote. Such a key is POISONED: the entry is
/// dropped and the document is reported as the walk produced it. The caller's own input
/// is remembered last and OVERRIDES whatever the definition wrote for the same value:
/// those bytes are the ones the caller sent.
#[derive(Clone, Debug, Default)]
pub(super) struct Supplied {
    texts: BTreeMap<String, String>,
    /// The canonical encodings two documents were written as differently, which are
    /// reported as the walk produced them.
    poisoned: BTreeSet<String>,
    /// How many bytes of definition text have been scanned, against
    /// [`MAX_SUPPLIED_SCAN_BYTES`].
    scanned: usize,
}

impl Supplied {
    /// Remember `text` as the bytes a CALLER supplied the document in, which is what
    /// that document is reported in.
    ///
    /// The caller's bytes are authoritative, so this REPLACES whatever a definition
    /// document of the same value left behind — including the absence of an entry,
    /// where the caller's own bytes are already the canonical encoding.
    pub(super) fn supplied(&mut self, text: &str) {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let canonical = value.to_string();
        self.poisoned.remove(&canonical);
        self.texts.remove(&canonical);
        if canonical == text {
            return;
        }
        self.texts.insert(canonical, text.to_string());
    }

    /// Remember every DATA document the DEFINITION writes, state by state: the
    /// [`DATA_MEMBERS`] of each state it declares, and of every state nested inside
    /// one of them ([`NESTED_MEMBERS`]).
    ///
    /// The walk is STRUCTURAL rather than over every node, so a member that configures
    /// a state is never mistaken for a document a state puts out.
    pub(super) fn definition(&mut self, text: &str, depth: usize) {
        if depth > MAX_DEFINITION_DEPTH || self.scanned >= MAX_SUPPLIED_SCAN_BYTES {
            return;
        }
        // The scan is charged HERE as well as in [`Supplied::written`]: a definition
        // whose states declare no data member at all still costs one parse of every
        // nested state list, so the allowance has to cover this descent too.
        self.scanned = self.scanned.saturating_add(text.len());
        let Ok(document) = serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(text) else {
            return;
        };
        let Some(states) = document.get("States") else {
            return;
        };
        let Ok(states) = serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(states.get())
        else {
            return;
        };
        for state in states.values() {
            let Ok(members) = serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(state.get())
            else {
                continue;
            };
            for (member, value) in &members {
                if DATA_MEMBERS.contains(&member.as_str()) {
                    self.written(value.get(), 0);
                } else if NESTED_MEMBERS.contains(&member.as_str()) {
                    match serde_json::from_str::<Vec<Box<RawValue>>>(value.get()) {
                        // `Branches` is an array of state lists.
                        Ok(branches) => {
                            for branch in &branches {
                                self.definition(branch.get(), depth + 1);
                            }
                        }
                        // A `Map`'s processor is one.
                        Err(_) => self.definition(value.get(), depth + 1),
                    }
                }
            }
        }
    }

    /// Remember one data document — itself, and recursively each member of an object
    /// and each item of an array it carries — as the definition writes it.
    ///
    /// Bounded by [`MAX_SUPPLIED_DOCUMENTS`], [`MAX_SUPPLIED_SCAN_BYTES`] and
    /// [`MAX_DEFINITION_DEPTH`]: the text is a caller's definition and one of its
    /// documents can be a subtree of another, so neither how many are remembered, how
    /// much text is scanned for them, nor how deeply the descent goes is left to the
    /// definition.
    fn written(&mut self, text: &str, depth: usize) {
        if depth > MAX_DEFINITION_DEPTH
            || self.texts.len() >= MAX_SUPPLIED_DOCUMENTS
            || self.scanned >= MAX_SUPPLIED_SCAN_BYTES
        {
            return;
        }
        self.scanned = self.scanned.saturating_add(text.len());
        self.remember(text, &::service_common::json_document::minified(text));
        let opening = text.trim_start().chars().next();
        match opening {
            Some('{') => {
                if let Ok(members) = serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(text)
                {
                    for member in members.values() {
                        self.written(member.get(), depth + 1);
                    }
                }
            }
            Some('[') => {
                if let Ok(items) = serde_json::from_str::<Vec<Box<RawValue>>>(text) {
                    for item in &items {
                        self.written(item.get(), depth + 1);
                    }
                }
            }
            _ => {}
        }
    }

    /// Hold `reported` as the encoding of the document `text` holds, where that is not
    /// already the encoding a re-serialization answers and no other document of this
    /// definition was written as different text for the same value.
    fn remember(&mut self, text: &str, reported: &str) {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let canonical = value.to_string();
        if canonical == reported || self.poisoned.contains(&canonical) {
            return;
        }
        match self.texts.get(&canonical) {
            // Written twice, as the same text: one document, held once.
            Some(held) if held == reported => {}
            // Written twice, as DIFFERENT text: neither may be put back.
            Some(_) => {
                self.texts.remove(&canonical);
                self.poisoned.insert(canonical);
            }
            None => {
                self.texts.insert(canonical, reported.to_string());
            }
        }
    }

    /// `value` as this service REPORTS it: the bytes it was GIVEN in where this walk
    /// was given it, and the encoding the walk produced it in otherwise.
    pub(super) fn echo(&self, value: &Value) -> String {
        let canonical = value.to_string();
        match self.texts.get(&canonical) {
            Some(reported) => reported.clone(),
            None => canonical,
        }
    }
}

/// `value` as this service REPORTS it ([`Supplied::echo`]).
///
/// The ONE implementation of that rule, so every document this module reports follows
/// it: a payload a caller hashes, signs or diffs against what it sent comes back byte
/// for byte, a document the definition WROTE keeps its member order and its number
/// literals, and a document the definition BUILT is reported as built.
pub(super) fn echoed_payload(value: &Value, supplied: &Supplied) -> String {
    supplied.echo(value)
}

thread_local! {
    /// The documents the walk running on THIS thread was given as text.
    ///
    /// Empty on a thread no walk installed any on, where every document is reported in
    /// the encoding it was produced in: the same primitives also validate a definition
    /// at create time, which was given no execution data at all.
    static GIVEN: ::std::cell::RefCell<Supplied> =
        ::std::cell::RefCell::new(Supplied::default());
}

/// The documents ONE walk was given as text, installed on the thread that walk runs on
/// for as long as it lasts — exactly as the work it may do is ([`WorkAllowance`]).
///
/// It is AMBIENT for the same reason: a diagnosis about a document — a path that
/// selected nothing in it, a reference path that could not be applied to it — QUOTES
/// that document, and those diagnoses are worded by the plain functions the walk
/// reaches through several layers (a payload template's member, an intrinsic's
/// argument, a `ResultPath`'s own assignment) rather than by the walk itself. The
/// documents in force when it was installed are restored when it is dropped, on the
/// error paths and while unwinding as much as at the end of a walk, so a blocking
/// thread that carries one walk after another quotes each walk's own.
#[derive(Debug)]
pub(super) struct GivenDocuments {
    /// What was installed for the walk this one interrupted.
    restored: Supplied,
}

impl GivenDocuments {
    /// Install `supplied` as the documents the walk about to run on this thread was
    /// given.
    pub(super) fn of(supplied: &Supplied) -> Self {
        Self {
            restored: GIVEN.with(|given| given.replace(supplied.clone())),
        }
    }
}

impl Drop for GivenDocuments {
    fn drop(&mut self) {
        let restored = std::mem::take(&mut self.restored);
        GIVEN.with(|given| given.replace(restored));
    }
}

/// `value` as the walk running on this thread reports it: the bytes it was GIVEN in
/// where that walk was given it, and the encoding the walk produced it in otherwise
/// ([`echoed_payload`]).
///
/// What a diagnosis QUOTES a document in, for the diagnoses worded where the walk's own
/// record of them is not in scope ([`GivenDocuments`]).
pub(super) fn reported_document(value: &Value) -> String {
    GIVEN.with(|given| given.borrow().echo(value))
}
