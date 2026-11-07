use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

fn yaml_options() -> serde_saphyr::SerializerOptions {
    serde_saphyr::ser_options! {
        yaml_12: true,
        no_lang_directive: true,
        indent_step: 2,
        compact_list_indent: true,
        prefer_block_scalars: true,
        folded_wrap_chars: usize::MAX,
    }
}

/// Convert a `serde_json::Value` into a YAML string.
///
/// Mirrors the TypeScript `jsonToYamlString`:
///   - Keys can be sorted alphabetically (`sort_keys`)
///   - Priority keys appear first (`keys_priority`)
///   - Multiline strings → YAML block scalars (handled automatically by the serializer)
pub fn json_to_yaml_string_inner(
    json: serde_json::Value,
    sort_keys: bool,
    priority_keys: &[String],
) -> String {
    let ordered = OrderedJson {
        value: &json,
        sort_keys,
        priority: priority_keys,
    };
    match serde_saphyr::to_string_with_options(&ordered, yaml_options()) {
        Ok(s) => s,
        Err(_) => "# YAML conversion failed\n".to_owned(),
    }
}

/// Borrow the input so ordering does not require cloning its recursive values.
struct OrderedJson<'a> {
    value: &'a serde_json::Value,
    sort_keys: bool,
    priority: &'a [String],
}

impl Serialize for OrderedJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let sort_keys = self.sort_keys;
        let priority = self.priority;
        let child = |value| OrderedJson {
            value,
            sort_keys,
            priority,
        };
        match self.value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<String> = map.keys().cloned().collect();
                if sort_keys || !priority.is_empty() {
                    keys.sort_by(|a, b| {
                        let ai = priority.iter().position(|k| k == a);
                        let bi = priority.iter().position(|k| k == b);
                        match (ai, bi) {
                            (Some(x), Some(y)) => x.cmp(&y),
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => {
                                if sort_keys {
                                    a.cmp(b)
                                } else {
                                    std::cmp::Ordering::Equal
                                }
                            }
                        }
                    });
                }
                let mut mapping = serializer.serialize_map(Some(keys.len()))?;
                for key in keys {
                    if let Some(val) = map.get(&key) {
                        // Key spelling belongs to the library; preserve identity and order.
                        mapping.serialize_entry(&key, &child(val))?;
                    }
                }
                mapping.end()
            }
            serde_json::Value::Array(arr) => {
                let mut sequence = serializer.serialize_seq(Some(arr.len()))?;
                for value in arr {
                    sequence.serialize_element(&child(value))?;
                }
                sequence.end()
            }
            serde_json::Value::String(s) => {
                // Observe supported scalar rendering rather than duplicating YAML's
                // resolver/escaping rules. At most two extra scalar serializations;
                // controls rejected by SingleQuoted retain normal escaping.
                let normal = serde_saphyr::to_string_with_options(s, yaml_options())
                    .map_err(serde::ser::Error::custom)?;
                let single = serde_saphyr::SingleQuoted(s);
                if normal.starts_with('"')
                    && serde_saphyr::to_string_with_options(&single, yaml_options()).is_ok()
                {
                    single.serialize(serializer)
                } else {
                    s.serialize(serializer)
                }
            }
            value => value.serialize(serializer),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn emit(v: serde_json::Value) -> String {
        json_to_yaml_string_inner(v, false, &[])
    }

    #[test]
    fn keys_sorted_when_sort_keys_enabled() {
        let out = json_to_yaml_string_inner(json!({"z": 1, "a": 2}), true, &[]);
        let a = out.find("a:").expect("a key present");
        let z = out.find("z:").expect("z key present");
        assert!(a < z);
    }

    #[test]
    fn priority_keys_emitted_first() {
        let out = json_to_yaml_string_inner(
            json!({"c": 3, "a": 1, "b": 2}),
            false,
            &["b".to_owned(), "c".to_owned()],
        );
        let b = out.find("b:").expect("b present");
        let c = out.find("c:").expect("c present");
        let a = out.find("a:").expect("a present");
        assert!(b < c && c < a);
    }

    #[test]
    fn multiline_strings_use_block_scalars_with_sorted_keys() {
        let out = json_to_yaml_string_inner(
            json!({"z": "last", "a": "first\nsecond line", "b": 42, "c": true}),
            true,
            &[],
        );
        let a = out.find("a:").expect("a");
        let b = out.find("b:").expect("b");
        let c = out.find("c:").expect("c");
        let z = out.find("z:").expect("z");
        assert!(a < b && b < c && c < z, "keys not sorted: {out}");
        assert!(
            out.contains("|-"),
            "multiline must use a block scalar: {out}"
        );
    }

    /// Emission contract captured empirically from the serde_yaml 0.9 output
    /// that every MCP tool response is built on. Any YAML-crate change must
    /// reproduce these byte-exactly.
    #[test]
    fn emission_contract_locked_across_serializer_changes() {
        assert_eq!(emit(json!({"v": "no"})), "v: no\n");
        assert_eq!(emit(json!({"v": "true"})), "v: 'true'\n");
        assert_eq!(emit(json!({"v": "123"})), "v: '123'\n");
        assert_eq!(emit(json!({"v": "null"})), "v: 'null'\n");
        assert_eq!(emit(json!({"v": ""})), "v: ''\n");
        assert_eq!(emit(json!({"v": "a: b"})), "v: 'a: b'\n");
        assert_eq!(emit(json!({"v": "- item"})), "v: '- item'\n");
        assert_eq!(emit(json!({"v": "#comment"})), "v: '#comment'\n");
        assert_eq!(
            emit(json!({"v": "line1\nline2"})),
            "v: |-\n  line1\n  line2\n"
        );
        assert_eq!(
            emit(json!({"v": "café ünïcode 中文"})),
            "v: café ünïcode 中文\n"
        );
        assert_eq!(emit(json!({"n": 1.5})), "n: 1.5\n");
        assert_eq!(emit(json!({"n": 0})), "n: 0\n");
        assert_eq!(
            emit(json!({"a": [1, "x"], "b": {"c": null}})),
            "a:\n- 1\n- x\nb:\n  c: null\n"
        );
    }

    #[test]
    fn scalar_style_probe_preserves_quotes_controls_and_chomping() {
        assert_eq!(
            emit(json!({"v": "it's: \"quoted\""})),
            "v: 'it''s: \"quoted\"'\n"
        );
        assert_eq!(
            emit(json!({"v": "line1\nline2\n\n"})),
            "v: |+\n  line1\n  line2\n\n"
        );
        assert_eq!(emit(json!({"v": "\r\t\0"})), "v: \"\\r\\t\\0\"\n");
        let long = "a long line with words ".repeat(8);
        assert_eq!(
            emit(json!({"v": long.trim_end()})),
            format!("v: {}\n", long.trim_end())
        );
    }

    #[test]
    fn quoted_keys_and_values_round_trip_without_changing_identity() {
        let keys = [
            "",
            "true",
            "123",
            "no",
            "a: b",
            "it's: quoted",
            " leading",
            "trailing ",
            "line1\nline2",
            "\r\t\0",
            "\u{85}",
            "\u{2028}",
            "\u{2029}",
        ];
        let input = serde_json::Value::Object(
            keys.into_iter()
                .map(|key| (key.to_owned(), json!([key, {"true": key}])))
                .collect(),
        );
        let output = json_to_yaml_string_inner(input.clone(), true, &["true".into()]);
        let decoded: serde_json::Value = serde_saphyr::from_str_with_options(
            &output,
            serde_saphyr::options! { strict_booleans: true },
        )
        .expect("valid YAML 1.2");
        assert_eq!(decoded, input);
        assert!(
            output.starts_with("\"true\":"),
            "priority applies to quoted keys"
        );
    }
}
