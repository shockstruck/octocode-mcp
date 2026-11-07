//! Strict YAML-to-raw-rule adapter. Both passes use the original input and the
//! parser's finite default budgets; compilation remains in the parent module.
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt;

#[derive(Default, Deserialize)]
#[serde(from = "Option<RawRuleFields>")]
pub(super) struct RawRule {
    pub(super) kind: Option<String>,
    pub(super) pattern: Option<String>,
    pub(super) regex: Option<String>,
    pub(super) has: Option<Box<RawRule>>,
    pub(super) inside: Option<Box<RawRule>>,
    pub(super) all: Option<Vec<RawRule>>,
    pub(super) any: Option<Vec<RawRule>>,
    pub(super) not: Option<Box<RawRule>>,
    pub(super) stop_by: Option<RawStopBy>,
}

// A null raw rule historically reached the compiler as an empty matcher. Only
// this wrapper is nullable: the document's rule field is still required, and
// outer Options (has: null, all: null) retain their usual None semantics.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRuleFields {
    #[serde(default, deserialize_with = "strict_string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "strict_string")]
    pattern: Option<String>,
    #[serde(default, deserialize_with = "strict_string")]
    regex: Option<String>,
    has: Option<Box<RawRule>>,
    inside: Option<Box<RawRule>>,
    all: Option<Vec<RawRule>>,
    any: Option<Vec<RawRule>>,
    not: Option<Box<RawRule>>,
    #[serde(rename = "stopBy", default, deserialize_with = "strict_stop_by")]
    stop_by: Option<RawStopBy>,
}

impl From<Option<RawRuleFields>> for RawRule {
    fn from(value: Option<RawRuleFields>) -> Self {
        match value {
            Some(v) => Self {
                kind: v.kind,
                pattern: v.pattern,
                regex: v.regex,
                has: v.has,
                inside: v.inside,
                all: v.all,
                any: v.any,
                not: v.not,
                stop_by: v.stop_by,
            },
            None => Self::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RawStopBy {
    End,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRuleDocument {
    rule: RawRule,
}

struct StringOnly(String);

impl<'de> Deserialize<'de> for StringOnly {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct StringVisitor;
        impl Visitor<'_> for StringVisitor {
            type Value = StringOnly;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StringOnly(value.into()))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(StringOnly(value))
            }
        }
        // deserialize_string would coerce plain YAML numbers and booleans.
        d.deserialize_any(StringVisitor)
    }
}

fn strict_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<StringOnly>::deserialize(d).map(|value| value.map(|value| value.0))
}

fn strict_stop_by<'de, D: Deserializer<'de>>(d: D) -> Result<Option<RawStopBy>, D::Error> {
    struct Payload(Option<String>);
    impl<'de> Deserialize<'de> for Payload {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct PayloadVisitor;
            impl Visitor<'_> for PayloadVisitor {
                type Value = Payload;
                fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    f.write_str("end or a tagged unit variant")
                }
                fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                    Ok(Payload(None))
                }
                fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                    Ok(Payload(Some(value.into())))
                }
            }
            d.deserialize_any(PayloadVisitor)
        }
    }
    // The old Value-to-enum path rejected {end: null} but accepted !end null.
    // Capture the public tag before Option can discard a tagged null payload.
    let serde_saphyr::Tagged(Payload(value), tag) =
        serde_saphyr::Tagged::<Payload>::deserialize(d)?;
    match (tag.as_deref(), value.as_deref()) {
        (None | Some("tag:yaml.org,2002:null"), None) => Ok(None),
        (None | Some("tag:yaml.org,2002:str"), Some("end")) | (Some("!end"), None) => {
            Ok(Some(RawStopBy::End))
        }
        _ => Err(de::Error::custom("expected stopBy: end or !end null")),
    }
}

struct RootPresence(bool);

impl<'de> Deserialize<'de> for RootPresence {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct RootVisitor;
        impl<'de> Visitor<'de> for RootVisitor {
            type Value = RootPresence;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a rule mapping or null")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(RootPresence(false))
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut present = false;
                while let Some(serde_saphyr::Tagged(StringOnly(key), tag)) =
                    map.next_key::<serde_saphyr::Tagged<StringOnly>>()?
                {
                    // An application-tagged key is not the old Value::String
                    // discriminator. Standard !!str remains a string key.
                    present |= key == "rule"
                        && matches!(tag.as_deref(), None | Some("tag:yaml.org,2002:str"));
                    map.next_value::<IgnoredAny>()?;
                }
                Ok(RootPresence(present))
            }
        }
        d.deserialize_any(RootVisitor)
    }
}

fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
        ignore_binary_tag_for_string: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
    }
}

pub(super) fn parse(input: &str) -> Result<RawRule, String> {
    let presence: RootPresence = serde_saphyr::from_str_with_options(input, options())
        .map_err(|err| format!("invalid rule YAML: {err}"))?;
    if presence.0 {
        serde_saphyr::from_str_with_options::<RawRuleDocument>(input, options()).map(|doc| doc.rule)
    } else {
        serde_saphyr::from_str_with_options(input, options())
    }
    .map_err(|err| format!("invalid rule YAML: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_typed_and_binary_tags_preserve_source_text() {
        for field in ["kind", "pattern", "regex"] {
            for primitive in ["123", "1.5", "true", "false", ".inf", ".nan"] {
                for input in [
                    format!("{field}: {primitive}"),
                    format!("has: {{{field}: {primitive}}}"),
                ] {
                    assert!(parse(&input).is_err(), "{input}");
                }
            }
        }
        for (value, expected) in [
            ("no", "no"),
            ("0123", "0123"),
            ("1_234", "1_234"),
            ("'123'", "123"),
            ("!!str 123", "123"),
            ("!!binary aGVsbG8=", "aGVsbG8="),
            ("!custom identifier", "identifier"),
        ] {
            let raw = parse(&format!("rule: {{kind: {value}}}")).expect("string rule");
            assert_eq!(raw.kind.as_deref(), Some(expected));
        }
    }

    #[test]
    fn root_presence_is_not_nullness_or_a_tagged_key_spelling() {
        for input in ["", "null", "rule: null", "all: [null]"] {
            assert!(parse(input).is_ok(), "raw null staging: {input}");
        }
        assert!(parse("has: null").expect("outer option").has.is_none());
        assert_eq!(
            parse("!!str rule: {kind: identifier}")
                .expect("string key")
                .kind
                .as_deref(),
            Some("identifier")
        );
        assert_eq!(
            parse("!custom kind: identifier")
                .expect("tagged bare field")
                .kind
                .as_deref(),
            Some("identifier")
        );
        for input in [
            "!custom rule: {kind: identifier}",
            "rule: null\nkind: identifier",
            "kind: identifier\nrule: null",
            "rule: null\nrule: null",
            "rule: []",
            "kind: identifier\n---\nkind: identifier",
            "[]",
            "hello",
        ] {
            assert!(
                parse(input).is_err_and(|err| err.starts_with("invalid rule YAML:")),
                "{input}"
            );
        }
    }

    #[test]
    fn duplicates_unknown_fields_and_merges_stay_errors() {
        for body in [
            "kind: null\nkind: identifier",
            "unknown: x",
            "has: {unknown: x}",
            "has: {kind: x, kind: y}",
            "<<: {kind: identifier}",
            "'<<': {kind: identifier}",
            "!!merge <<: {kind: identifier}",
            "!!str <<: {kind: identifier}",
            "!custom <<: {kind: identifier}",
        ] {
            for input in [
                body.to_owned(),
                format!("rule:\n  {}", body.replace('\n', "\n  ")),
            ] {
                assert!(parse(&input).is_err(), "{input}");
            }
        }
    }

    #[test]
    fn stop_by_retains_string_and_tagged_unit_forms_only() {
        for value in ["end", "!!str end", "!end null", "!end ~", "!end"] {
            assert!(
                parse(&format!("stopBy: {value}")).expect("end").stop_by == Some(RawStopBy::End)
            );
        }
        for value in [
            "{end: null}",
            "{end: {}}",
            "!end {}",
            "!end end",
            "!custom end",
            "true",
            "123",
        ] {
            assert!(parse(&format!("stopBy: {value}")).is_err(), "{value}");
        }
        assert!(parse("stopBy: null")
            .expect("null option")
            .stop_by
            .is_none());
    }

    #[test]
    fn aliases_and_finite_budgets_apply_to_both_passes() {
        assert!(parse("has: &r {kind: identifier}\ninside: *r").is_ok());
        assert!(parse("has: &r {kind: identifier, has: *r}").is_err());
        for (within, over, budget) in [
            (
                "has: {kind: identifier}",
                "has: {has: {has: {kind: identifier}}}",
                serde_saphyr::budget! {max_depth: 3},
            ),
            (
                "all: [&r {kind: identifier}, *r]",
                "all: [&r {kind: identifier}, *r, *r]",
                serde_saphyr::budget! {max_aliases: 1},
            ),
        ] {
            let mut opts = options();
            opts.budget = budget;
            assert!(
                serde_saphyr::from_str_with_options::<RootPresence>(within, opts.clone()).is_ok()
            );
            assert!(serde_saphyr::from_str_with_options::<RawRule>(within, opts.clone()).is_ok());
            assert!(
                serde_saphyr::from_str_with_options::<RootPresence>(over, opts.clone()).is_err(),
                "root budget: {over}"
            );
            assert!(serde_saphyr::from_str_with_options::<RawRule>(over, opts).is_err());
        }
        // IgnoredAny need not retain anchor payloads in the presence pass.
        // Exercise retained events on the typed pass, which replays aliases.
        let mut opts = options();
        opts.budget = serde_saphyr::budget! {max_recorded_anchor_events: 3};
        assert!(serde_saphyr::from_str_with_options::<RawRule>(
            "kind: &k identifier\npattern: *k",
            opts.clone()
        )
        .is_ok());
        assert!(serde_saphyr::from_str_with_options::<RawRule>(
            "has: &r {kind: identifier}\ninside: *r",
            opts
        )
        .is_err());
    }
}
