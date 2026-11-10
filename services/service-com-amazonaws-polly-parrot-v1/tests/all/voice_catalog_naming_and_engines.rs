// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `DescribeVoices` reports one voice.
//!
//! Three rules, each checked over a TABLE that spans the class rather than over the
//! single voice an issue happens to quote:
//!
//! * a voice is reported under its OWN name — the spelling Amazon Polly's voice table
//!   publishes, diacritics included — and its `Id` is "the ASCII version of the voice
//!   name" (`DescribeVoices`' own documentation), which is the id `SynthesizeSpeech`
//!   takes;
//! * `SupportedEngines` is a SET, reported in the service's one engine order, so two
//!   voices never report the same engines in different orders and the order never
//!   depends on how a catalog row happens to be written;
//! * a voice's `LanguageCode`/`LanguageName` are its default language and that
//!   language's English name, one name per language for every voice that speaks it,
//!   and a bilingual voice carries the OTHER languages it speaks in
//!   `AdditionalLanguageCodes`.
//!
//! The per-voice values the tables below name come from the AWS "Available voices"
//! table linked from `SynthesizeSpeech$Engine`'s documentation, which is the catalog
//! `crate::state::VOICE_CATALOG` is seeded from; the member rules come from
//! `smithy_query(query="shape", shape="Voice")`.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_polly_parrot_v1::ops::types::VOICE_ID_VALUES;
use service_com_amazonaws_polly_parrot_v1::state::REPORTED_ENGINE_ORDER;

use crate::lexicons_and_speech::{json, voices, Polly};
use crate::voice_catalog::engines_of;

/// "The ASCII version of the voice name": the voice name with every diacritic folded
/// onto the Latin letter it is written over.
///
/// `pub(crate)` so the catalog suite checks `Voice$Id` against `Voice$Name` with the
/// same folding this one does.
pub(crate) fn ascii_version_of(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        })
        .collect()
}

/// One reported voice, by id.
async fn voice_by_id(polly: &Polly, id: &str) -> serde_json::Value {
    voices(polly, "")
        .await
        .into_iter()
        .find(|voice| voice["Id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("case {id:?}: the catalog must report this voice"))
}

/// RULE: `Voice$Name` is the voice's own name, spelled as Amazon Polly's voice table
/// spells it, and `Voice$Id` is the ASCII version of that name — so a voice whose name
/// carries diacritics is discovered under that spelling and synthesized under the ASCII
/// id, and the two agree for a voice whose name is already ASCII.
#[tokio::test]
async fn a_voice_is_named_as_its_table_spells_it_and_identified_by_the_ascii_form() {
    let polly = Polly::new();
    let all = voices(&polly, "").await;

    // Every voice whose published name carries diacritics, one per spelling the table
    // uses (acute, grave, circumflex and tilde-free Latin letters), plus ASCII-named
    // voices of three languages as the contrast rows.
    let table: [(&str, &str); 12] = [
        ("Andres", "Andrés"),
        ("Celine", "Céline"),
        ("Dora", "Dóra"),
        ("Ines", "Inês"),
        ("Lea", "Léa"),
        ("Penelope", "Penélope"),
        ("Remi", "Rémi"),
        ("Vitoria", "Vitória"),
        ("Joanna", "Joanna"),
        ("Zeina", "Zeina"),
        ("Seoyeon", "Seoyeon"),
        ("Lorenzo", "Lorenzo"),
    ];
    for (id, name) in table {
        let voice = all
            .iter()
            .find(|voice| voice["Id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {id:?}: the catalog must report this voice"));
        assert_eq!(
            voice["Name"].as_str(),
            Some(name),
            "case {id:?}: wrong Name, got {voice:?}"
        );
        assert_eq!(
            voice["Id"].as_str(),
            Some(id),
            "case {id:?}: wrong Id, got {voice:?}"
        );
        // The id is what a later call names the voice by, so it round-trips through
        // synthesis while the displayed name does not have to.
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hola",
                "VoiceId": id,
                "Engine": engines_of(voice)[0],
                "OutputFormat": "mp3",
            }))
            .await;
        assert_eq!(
            response.status(),
            200,
            "case {id:?}: synthesis must accept the reported id, body {:?}",
            String::from_utf8_lossy(response.body())
        );
    }

    // The same rule over every row of the catalog, not just the named ones.
    let mut spelled = 0usize;
    for voice in &all {
        let id = voice["Id"].as_str().expect("every voice carries an Id");
        let name = voice["Name"].as_str().expect("every voice carries a Name");
        assert_eq!(
            ascii_version_of(name),
            id,
            "case {id:?}: Id must be the ASCII version of Name, got {voice:?}"
        );
        assert!(
            id.is_ascii(),
            "case {id:?}: a voice id is the ASCII form of the name, got {voice:?}"
        );
        if !name.is_ascii() {
            spelled += 1;
            assert_ne!(
                name, id,
                "case {id:?}: a voice whose name carries diacritics must keep them"
            );
        } else {
            assert_eq!(
                name, id,
                "case {id:?}: an ASCII voice name is its own id, got {voice:?}"
            );
        }
    }
    assert_eq!(
        spelled,
        8,
        "the catalog must report every voice whose published name carries diacritics, \
         got {spelled} of them"
    );

    // An independent pin on the SIZE of the catalog: the model enumerates every voice id
    // the API accepts and ten of those are voices this catalog does not offer, so the
    // reported list is that enumeration less those ten. No catalog constant takes part
    // in this count, so a row moved between the offered and the unoffered table cannot
    // pass unnoticed by the suites that compare the response against the seed.
    const UNOFFERED_MODELED_VOICES: usize = 10;
    assert_eq!(
        all.len(),
        VOICE_ID_VALUES.len() - UNOFFERED_MODELED_VOICES,
        "the catalog must report every modelled voice but the \
         {UNOFFERED_MODELED_VOICES} it does not offer, got {} of {}",
        all.len(),
        VOICE_ID_VALUES.len()
    );
}

/// RULE: `SupportedEngines` is the set of engines the voice is offered on, reported in
/// the service's one engine order — the same order for a voice with one engine and for
/// a voice with all of them — and a filter on an engine selects exactly the voices
/// whose reported set carries it.
#[tokio::test]
async fn supported_engines_are_reported_in_one_order_for_every_voice() {
    let polly = Polly::new();
    let all = voices(&polly, "").await;

    let rank = |engine: &str| {
        REPORTED_ENGINE_ORDER
            .iter()
            .position(|known| *known == engine)
            .unwrap_or_else(|| panic!("engine {engine:?} is outside the reported order"))
    };

    // Rows spanning the class: voices reported with one, two and three engines, and
    // one of each engine value the catalog offers as the head of its own list.
    let table: [(&str, &[&str]); 6] = [
        ("Zeina", &["standard"]),
        ("Hiujin", &["neural"]),
        ("Ivy", &["neural", "standard"]),
        ("Lorenzo", &["generative"]),
        ("Danielle", &["generative", "neural"]),
        ("Vicki", &["generative", "neural", "standard"]),
    ];
    for (id, expected) in table {
        let voice = all
            .iter()
            .find(|voice| voice["Id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {id:?}: the catalog must report this voice"));
        assert_eq!(
            engines_of(voice),
            expected,
            "case {id:?}: wrong SupportedEngines, got {voice:?}"
        );
    }

    // The ordering is a property of every row, whatever engines it carries.
    for voice in &all {
        let id = voice["Id"].as_str().expect("every voice carries an Id");
        let engines = engines_of(voice);
        assert!(
            !engines.is_empty(),
            "case {id:?}: a reported voice is offered on at least one engine, got {voice:?}"
        );
        let ranks: Vec<usize> = engines.iter().map(|engine| rank(engine)).collect();
        assert!(
            ranks.windows(2).all(|pair| pair[0] < pair[1]),
            "case {id:?}: engines {engines:?} are not in the reported order \
             {REPORTED_ENGINE_ORDER:?}"
        );
    }

    // And the engine filter selects on exactly those sets: one filtered read per
    // engine, compared with what the unfiltered catalog reported.
    for engine in REPORTED_ENGINE_ORDER {
        let expected: BTreeSet<String> = all
            .iter()
            .filter(|voice| engines_of(voice).iter().any(|e| e == engine))
            .map(|voice| voice["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        let filtered = voices(&polly, &format!("?Engine={engine}")).await;
        let reported: BTreeSet<String> = filtered
            .iter()
            .map(|voice| voice["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {engine:?}: the engine filter must select the voices the catalog \
             reports that engine for"
        );
        for voice in &filtered {
            let id = voice["Id"].as_str().unwrap_or_default();
            assert_eq!(
                engines_of(voice),
                all.iter()
                    .find(|row| row["Id"].as_str() == Some(id))
                    .map(engines_of)
                    .unwrap_or_default(),
                "case {engine:?}: voice {id:?} must report the same engines filtered \
                 as unfiltered"
            );
        }
    }
}

/// RULE: a voice's `LanguageCode` is the language it is offered for and `LanguageName`
/// that language's name in English — one name per language, for every voice that speaks
/// it — and a bilingual voice reports the OTHER languages it speaks in
/// `AdditionalLanguageCodes` rather than as its default one.
#[tokio::test]
async fn a_voice_reports_the_language_and_engines_its_table_publishes() {
    let polly = Polly::new();
    let all = voices(&polly, "").await;

    // (id, language code, language name, additional codes, engines) for rows spanning
    // the class: the German-speaking locales that share a language name stem, the two
    // bilingual language pairs the model documents, voices offered on one engine and on
    // three, and voices this issue did not quote (Lorenzo, Zhiyu, Hala, Aditi).
    type Row = (&'static str, &'static str, &'static str, &'static [&'static str], &'static [&'static str]);
    let table: [Row; 13] = [
        ("Vicki", "de-DE", "German", &[], &["generative", "neural", "standard"]),
        ("Hannah", "de-AT", "Austrian German", &[], &["generative", "neural"]),
        ("Sabrina", "de-CH", "Swiss Standard German", &[], &["generative", "neural"]),
        ("Aditi", "en-IN", "Indian English", &["hi-IN"], &["standard"]),
        ("Kajal", "en-IN", "Indian English", &["hi-IN"], &["generative", "neural"]),
        ("Hala", "ar-AE", "Gulf Arabic", &["arb"], &["neural"]),
        ("Zayd", "ar-AE", "Gulf Arabic", &["arb"], &["neural"]),
        ("Zeina", "arb", "Arabic", &[], &["standard"]),
        ("Adriano", "it-IT", "Italian", &[], &["neural"]),
        ("Lorenzo", "it-IT", "Italian", &[], &["generative"]),
        ("Zhiyu", "cmn-CN", "Chinese Mandarin", &[], &["neural", "standard"]),
        ("Brian", "en-GB", "British English", &[], &["generative", "neural", "standard"]),
        ("Geraint", "en-GB-WLS", "Welsh English", &[], &["standard"]),
    ];
    for (id, code, language_name, additional, engines) in table {
        let voice = all
            .iter()
            .find(|voice| voice["Id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {id:?}: the catalog must report this voice"));
        assert_eq!(
            (
                voice["LanguageCode"].as_str(),
                voice["LanguageName"].as_str(),
            ),
            (Some(code), Some(language_name)),
            "case {id:?}: wrong default language, got {voice:?}"
        );
        assert_eq!(
            voice.get("AdditionalLanguageCodes").cloned(),
            (!additional.is_empty()).then(|| serde_json::json!(additional)),
            "case {id:?}: wrong AdditionalLanguageCodes, got {voice:?}"
        );
        assert_eq!(
            engines_of(voice),
            engines,
            "case {id:?}: wrong SupportedEngines, got {voice:?}"
        );
    }

    // One language, one English name: a voice that disagreed with another about its own
    // language's name would mis-bucket a client grouping the catalog by language.
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    for voice in &all {
        let id = voice["Id"].as_str().expect("every voice carries an Id");
        let code = voice["LanguageCode"]
            .as_str()
            .expect("every voice carries a LanguageCode")
            .to_string();
        let name = voice["LanguageName"]
            .as_str()
            .expect("every voice carries a LanguageName")
            .to_string();
        if let Some(first) = named.get(&code) {
            assert_eq!(
                &name, first,
                "case {id:?}: language {code:?} is named {name:?} here and {first:?} by \
                 another voice"
            );
        } else {
            named.insert(code, name);
        }
    }

    // A bilingual voice's default language is one language and its additional codes are
    // others, so the filtered views of the two differ: the voice is reported for its
    // additional language only when the request admits additional languages.
    for id in ["Aditi", "Kajal", "Hala", "Zayd"] {
        let voice = voice_by_id(&polly, id).await;
        let default_code = voice["LanguageCode"].as_str().unwrap_or_default().to_string();
        let additional: Vec<String> = voice["AdditionalLanguageCodes"]
            .as_array()
            .expect("a bilingual voice carries additional codes")
            .iter()
            .map(|code| code.as_str().unwrap_or_default().to_string())
            .collect();
        for code in &additional {
            assert_ne!(
                code, &default_code,
                "case {id:?}: the default language must not repeat as an additional one"
            );
            let narrow = voices(&polly, &format!("?LanguageCode={code}")).await;
            let wide = voices(
                &polly,
                &format!("?LanguageCode={code}&IncludeAdditionalLanguageCodes=true"),
            )
            .await;
            let found = |rows: &[serde_json::Value]| {
                rows.iter().any(|row| row["Id"].as_str() == Some(id))
            };
            assert!(
                !found(&narrow) && found(&wide),
                "case {id:?}: must be reported for {code:?} only with \
                 IncludeAdditionalLanguageCodes"
            );
        }
    }

    // The catalog reports the same voice record to a language-filtered read as to an
    // unfiltered one — the filter narrows the list, it does not rewrite a row.
    let one_language = voices(&polly, "?LanguageCode=de-CH").await;
    assert_eq!(
        one_language.len(),
        1,
        "exactly the Swiss Standard German voices must answer that filter, got \
         {one_language:?}"
    );
    assert_eq!(
        one_language[0],
        voice_by_id(&polly, "Sabrina").await,
        "a filtered read must report the same record as an unfiltered one"
    );

    // A filtered read is still a complete, untruncated listing.
    let response = polly.describe_voices("?LanguageCode=de-CH").await;
    assert_eq!(
        json(&response).get("NextToken"),
        None,
        "a filtered catalog read owes no continuation token, body {:?}",
        json(&response)
    );
}
