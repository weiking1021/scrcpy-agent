//! The settings of an add-on: what the user can set for it. The add-on declares them in its manifest (`settings`), as
//! values of a few kinds with limits, so the program draws the form, checks what is typed, and keeps the values; the
//! add-on reads them (`host.settings`) and is told when one changes. Nothing here is free text that is guessed
//! at: a declaration that is not exactly right makes the manifest not one (`SettingsProblem`), and a value that does
//! not fit what was declared is refused with its reason (`ValueProblem`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The most settings an add-on declares.
pub const MAX_SETTINGS: usize = 50;
pub const MAX_LABEL_CHARS: usize = 100;
pub const MAX_HINT_CHARS: usize = 300;
/// The longest a text setting may be allowed to be.
pub const MAX_TEXT_LENGTH: usize = 1000;
pub const MAX_OPTIONS: usize = 50;

/// One setting an add-on declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "SettingSpec.ts"))]
pub struct SettingSpec {
    /// A name (the rules of an add-on's id); it is what the add-on reads the setting by.
    pub id: String,
    /// What the user reads.
    pub label: String,
    /// A line under it, if the add-on gave one.
    pub hint: Option<String>,
    #[serde(flatten)]
    pub kind: SettingKind,
}

/// What a setting holds, and its limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "SettingKind.ts"))]
pub enum SettingKind {
    Boolean { default: bool },
    /// A whole number from `min` to `max`.
    Number { default: i64, min: i64, max: i64 },
    /// One line of text, at most `max_length` characters.
    Text { default: String, max_length: usize },
    /// One of a list of values.
    Choice { default: String, options: Vec<ChoiceOption> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "ChoiceOption.ts"))]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
}

/// A value of a setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "SettingValue.ts"))]
pub enum SettingValue {
    Boolean(bool),
    Number(i64),
    /// A text, or the value of the option chosen.
    Text(String),
}

/// The settings of an add-on as the user sees them: what it declared and the value of each now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddonSettingsView {
    pub specs: Vec<SettingSpec>,
    pub values: BTreeMap<String, SettingValue>,
}

/// Why a declaration of settings is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsProblem {
    NotAList,
    TooMany,
    NotAnObject,
    BadId,
    DuplicateId,
    BadLabel,
    BadHint,
    UnknownType,
    /// The default is missing or not of the kind of the setting, or not allowed by its limits.
    BadDefault,
    /// A number's `min` and `max` are missing, not whole numbers, or `min` is above `max`.
    BadRange,
    /// A text's `max_length` is missing, zero or above `MAX_TEXT_LENGTH`.
    BadLength,
    /// A choice's options are missing, too many, repeated, or one is not a name and a label.
    BadOptions,
}

/// Why a value is not one of a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueProblem {
    /// Not the kind of value the setting holds.
    WrongType,
    /// A number outside the limits.
    OutOfRange,
    /// A text longer than the setting allows.
    TooLong,
    /// A text with a control character (a setting is one line).
    HasControl,
    /// Not one of the options.
    NotAnOption,
}

/// Reads the `settings` of a manifest.
pub fn parse_settings(value: &Value) -> Result<Vec<SettingSpec>, SettingsProblem> {
    let list = value.as_array().ok_or(SettingsProblem::NotAList)?;
    if list.len() > MAX_SETTINGS {
        return Err(SettingsProblem::TooMany);
    }
    let mut specs: Vec<SettingSpec> = Vec::new();
    for item in list {
        let spec = parse_one(item)?;
        if specs.iter().any(|s| s.id == spec.id) {
            return Err(SettingsProblem::DuplicateId);
        }
        specs.push(spec);
    }
    Ok(specs)
}

/// A line of text for the user: a string, without the spaces around it, from 1 to `max` characters, no control character.
fn line_of(value: Option<&Value>, max: usize) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control)).then(|| text.to_string())
}

fn parse_one(item: &Value) -> Result<SettingSpec, SettingsProblem> {
    use SettingsProblem::*;
    let object = item.as_object().ok_or(NotAnObject)?;
    let id = match object.get("id") {
        Some(Value::String(id)) if super::buttons::valid_name(id) => id.clone(),
        _ => return Err(BadId),
    };
    let label = line_of(object.get("label"), MAX_LABEL_CHARS).ok_or(BadLabel)?;
    let hint = match object.get("hint") {
        None | Some(Value::Null) => None,
        Some(other) => Some(line_of(Some(other), MAX_HINT_CHARS).ok_or(BadHint)?),
    };
    let default = object.get("default");
    let kind = match object.get("type").and_then(Value::as_str) {
        Some("boolean") => SettingKind::Boolean { default: default.and_then(Value::as_bool).ok_or(BadDefault)? },
        Some("number") => {
            let (min, max) = match (object.get("min").and_then(Value::as_i64), object.get("max").and_then(Value::as_i64)) {
                (Some(min), Some(max)) if min <= max => (min, max),
                _ => return Err(BadRange),
            };
            let default = default.and_then(Value::as_i64).filter(|d| (min..=max).contains(d)).ok_or(BadDefault)?;
            SettingKind::Number { default, min, max }
        }
        Some("text") => {
            let max_length = object
                .get("max_length")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| (1..=MAX_TEXT_LENGTH).contains(n))
                .ok_or(BadLength)?;
            let default = default.and_then(Value::as_str).ok_or(BadDefault)?;
            if default.chars().count() > max_length || default.chars().any(char::is_control) {
                return Err(BadDefault);
            }
            SettingKind::Text { default: default.to_string(), max_length }
        }
        Some("choice") => {
            let list = object.get("options").and_then(Value::as_array).ok_or(BadOptions)?;
            if list.is_empty() || list.len() > MAX_OPTIONS {
                return Err(BadOptions);
            }
            let mut options: Vec<ChoiceOption> = Vec::new();
            for entry in list {
                let entry = entry.as_object().ok_or(BadOptions)?;
                let value = match entry.get("value") {
                    Some(Value::String(value)) if super::buttons::valid_name(value) => value.clone(),
                    _ => return Err(BadOptions),
                };
                let label = line_of(entry.get("label"), MAX_LABEL_CHARS).ok_or(BadOptions)?;
                if options.iter().any(|o| o.value == value) {
                    return Err(BadOptions);
                }
                options.push(ChoiceOption { value, label });
            }
            let default = default.and_then(Value::as_str).filter(|d| options.iter().any(|o| o.value == *d)).ok_or(BadDefault)?;
            SettingKind::Choice { default: default.to_string(), options }
        }
        _ => return Err(UnknownType),
    };
    Ok(SettingSpec { id, label, hint, kind })
}

/// Whether `value` is a value of the setting, and if it is, as the program holds it.
pub fn validate_value(spec: &SettingSpec, value: &Value) -> Result<SettingValue, ValueProblem> {
    match &spec.kind {
        SettingKind::Boolean { .. } => value.as_bool().map(SettingValue::Boolean).ok_or(ValueProblem::WrongType),
        SettingKind::Number { min, max, .. } => {
            let n = value.as_i64().ok_or(ValueProblem::WrongType)?;
            if (*min..=*max).contains(&n) { Ok(SettingValue::Number(n)) } else { Err(ValueProblem::OutOfRange) }
        }
        SettingKind::Text { max_length, .. } => {
            let text = value.as_str().ok_or(ValueProblem::WrongType)?;
            if text.chars().count() > *max_length {
                Err(ValueProblem::TooLong)
            } else if text.chars().any(char::is_control) {
                Err(ValueProblem::HasControl)
            } else {
                Ok(SettingValue::Text(text.to_string()))
            }
        }
        SettingKind::Choice { options, .. } => {
            let text = value.as_str().ok_or(ValueProblem::WrongType)?;
            if options.iter().any(|o| o.value == text) { Ok(SettingValue::Text(text.to_string())) } else { Err(ValueProblem::NotAnOption) }
        }
    }
}

/// What a setting is when nothing was set.
pub fn default_of(spec: &SettingSpec) -> SettingValue {
    match &spec.kind {
        SettingKind::Boolean { default } => SettingValue::Boolean(*default),
        SettingKind::Number { default, .. } => SettingValue::Number(*default),
        SettingKind::Text { default, .. } | SettingKind::Choice { default, .. } => SettingValue::Text(default.clone()),
    }
}

/// The value of every declared setting: what was kept where it is valid, the default for the rest. What was kept for a
/// setting that is not declared (any more), or that is not valid (any more), is left out; it does not lose the others.
pub fn values_of(specs: &[SettingSpec], kept: &Value) -> BTreeMap<String, SettingValue> {
    let kept = kept.as_object();
    specs
        .iter()
        .map(|spec| {
            let value = kept
                .and_then(|kept| kept.get(&spec.id))
                .and_then(|value| validate_value(spec, value).ok())
                .unwrap_or_else(|| default_of(spec));
            (spec.id.clone(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<Vec<SettingSpec>, SettingsProblem> {
        parse_settings(&value)
    }

    fn one(spec: Value) -> Result<SettingSpec, SettingsProblem> {
        parse(json!([spec])).map(|mut list| list.remove(0))
    }

    fn number() -> Value {
        json!({ "id": "level", "type": "number", "label": "Level", "default": 5, "min": 0, "max": 10 })
    }

    fn spec_of(value: Value) -> SettingSpec {
        one(value).expect("a good declaration")
    }

    #[test]
    fn a_list_of_the_four_kinds_is_read_into_what_was_declared() {
        let list = parse(json!([
            { "id": "enabled", "type": "boolean", "label": "Enabled", "hint": "Turns it on", "default": true },
            number(),
            { "id": "name", "type": "text", "label": "Name", "default": "abc", "max_length": 20 },
            { "id": "mode", "type": "choice", "label": "Mode", "default": "fast",
              "options": [{ "value": "fast", "label": "Fast" }, { "value": "slow", "label": "Slow" }] },
        ]))
        .expect("a good list");
        assert_eq!(list.len(), 4);
        assert_eq!(
            list[0],
            SettingSpec { id: "enabled".into(), label: "Enabled".into(), hint: Some("Turns it on".into()), kind: SettingKind::Boolean { default: true } }
        );
        assert_eq!(list[1].kind, SettingKind::Number { default: 5, min: 0, max: 10 });
        assert_eq!(list[1].hint, None);
        assert_eq!(list[2].kind, SettingKind::Text { default: "abc".into(), max_length: 20 });
        assert_eq!(
            list[3].kind,
            SettingKind::Choice {
                default: "fast".into(),
                options: vec![
                    ChoiceOption { value: "fast".into(), label: "Fast".into() },
                    ChoiceOption { value: "slow".into(), label: "Slow".into() },
                ]
            }
        );
        // No settings at all is a list too.
        assert_eq!(parse(json!([])), Ok(vec![]));
    }

    #[test]
    fn a_declaration_has_the_shape_the_page_reads() {
        assert_eq!(
            serde_json::to_value(spec_of(number())).unwrap(),
            json!({ "id": "level", "label": "Level", "hint": null, "type": "number", "default": 5, "min": 0, "max": 10 })
        );
    }

    #[test]
    fn a_declaration_that_is_not_exactly_right_is_refused_with_the_reason() {
        use SettingsProblem::*;
        let with = |key: &str, value: Value| {
            let mut spec = number();
            spec[key] = value;
            one(spec)
        };
        assert_eq!(parse(json!({})), Err(NotAList));
        assert_eq!(parse(json!("x")), Err(NotAList));
        assert_eq!(parse(json!([1])), Err(NotAnObject));
        let many: Vec<Value> = (0..=MAX_SETTINGS).map(|i| json!({ "id": format!("s{i}"), "type": "boolean", "label": "L", "default": false })).collect();
        assert_eq!(parse(Value::Array(many)), Err(TooMany));
        assert_eq!(parse(json!([number(), number()])), Err(DuplicateId));
        for bad in [json!(""), json!("has space"), json!("a/b"), json!(5), Value::Null] {
            assert_eq!(with("id", bad.clone()), Err(BadId), "{bad}");
        }
        for bad in [json!(""), json!("   "), json!("two\nlines"), json!("x".repeat(MAX_LABEL_CHARS + 1)), json!(1)] {
            assert_eq!(with("label", bad.clone()), Err(BadLabel), "{bad}");
        }
        assert_eq!(with("hint", json!("x".repeat(MAX_HINT_CHARS + 1))), Err(BadHint));
        assert_eq!(with("hint", json!(3)), Err(BadHint));
        assert_eq!(with("type", json!("slider")), Err(UnknownType));
        assert_eq!(with("type", Value::Null), Err(UnknownType));
        // Numbers: whole numbers only, with limits, and a default inside them.
        for bad in [json!(2.5), json!("5"), json!(11), json!(-1), Value::Null] {
            assert_eq!(with("default", bad.clone()), Err(BadDefault), "{bad}");
        }
        assert_eq!(with("min", json!(11)), Err(BadRange));
        assert_eq!(with("max", json!("10")), Err(BadRange));
        assert_eq!(with("min", Value::Null), Err(BadRange));
        // Texts.
        let text = |extra: Value| {
            let mut spec = json!({ "id": "t", "type": "text", "label": "T", "default": "", "max_length": 5 });
            for (k, v) in extra.as_object().unwrap() {
                spec[k] = v.clone();
            }
            one(spec)
        };
        assert!(text(json!({})).is_ok());
        assert_eq!(text(json!({ "default": "toolong" })), Err(BadDefault));
        assert_eq!(text(json!({ "default": "a\nb" })), Err(BadDefault));
        assert_eq!(text(json!({ "max_length": 0 })), Err(BadLength));
        assert_eq!(text(json!({ "max_length": MAX_TEXT_LENGTH + 1 })), Err(BadLength));
        assert_eq!(text(json!({ "max_length": "5" })), Err(BadLength));
        // Choices.
        let choice = |options: Value, default: Value| {
            one(json!({ "id": "c", "type": "choice", "label": "C", "default": default, "options": options }))
        };
        let ab = json!([{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }]);
        assert!(choice(ab.clone(), json!("a")).is_ok());
        assert_eq!(choice(ab.clone(), json!("c")), Err(BadDefault));
        assert_eq!(choice(json!([]), json!("a")), Err(BadOptions));
        assert_eq!(choice(json!("x"), json!("a")), Err(BadOptions));
        assert_eq!(choice(json!([{ "value": "a", "label": "A" }, { "value": "a", "label": "B" }]), json!("a")), Err(BadOptions));
        assert_eq!(choice(json!([{ "value": "a b", "label": "A" }]), json!("a b")), Err(BadOptions));
        assert_eq!(choice(json!([{ "value": "a", "label": "" }]), json!("a")), Err(BadOptions));
        assert_eq!(choice(json!([{ "value": "a" }]), json!("a")), Err(BadOptions));
        let too_many: Vec<Value> = (0..=MAX_OPTIONS).map(|i| json!({ "value": format!("v{i}"), "label": "L" })).collect();
        assert_eq!(choice(Value::Array(too_many), json!("v0")), Err(BadOptions));
        // Booleans.
        let boolean = |default: Value| one(json!({ "id": "b", "type": "boolean", "label": "B", "default": default }));
        assert!(boolean(json!(false)).is_ok());
        assert_eq!(boolean(json!("false")), Err(BadDefault));
        assert_eq!(boolean(Value::Null), Err(BadDefault));
    }

    #[test]
    fn a_value_is_taken_only_if_it_is_of_the_kind_and_inside_the_limits_of_the_setting() {
        let level = spec_of(number());
        assert_eq!(validate_value(&level, &json!(7)), Ok(SettingValue::Number(7)));
        assert_eq!(validate_value(&level, &json!(0)), Ok(SettingValue::Number(0)));
        assert_eq!(validate_value(&level, &json!(10)), Ok(SettingValue::Number(10)));
        assert_eq!(validate_value(&level, &json!(11)), Err(ValueProblem::OutOfRange));
        assert_eq!(validate_value(&level, &json!(-1)), Err(ValueProblem::OutOfRange));
        for wrong in [json!(2.5), json!("3"), json!(true), Value::Null, json!(1e30)] {
            assert_eq!(validate_value(&level, &wrong), Err(ValueProblem::WrongType), "{wrong}");
        }
        let enabled = spec_of(json!({ "id": "e", "type": "boolean", "label": "E", "default": false }));
        assert_eq!(validate_value(&enabled, &json!(true)), Ok(SettingValue::Boolean(true)));
        for wrong in [json!("true"), json!(1), Value::Null] {
            assert_eq!(validate_value(&enabled, &wrong), Err(ValueProblem::WrongType), "{wrong}");
        }
        let name = spec_of(json!({ "id": "n", "type": "text", "label": "N", "default": "", "max_length": 5 }));
        assert_eq!(validate_value(&name, &json!("abcde")), Ok(SettingValue::Text("abcde".into())));
        assert_eq!(validate_value(&name, &json!("")), Ok(SettingValue::Text(String::new())));
        assert_eq!(validate_value(&name, &json!("abcdef")), Err(ValueProblem::TooLong));
        assert_eq!(validate_value(&name, &json!("é€😀é€")), Ok(SettingValue::Text("é€😀é€".into())), "five characters, more bytes");
        assert_eq!(validate_value(&name, &json!("a\nb")), Err(ValueProblem::HasControl));
        assert_eq!(validate_value(&name, &json!(5)), Err(ValueProblem::WrongType));
        let mode = spec_of(json!({ "id": "m", "type": "choice", "label": "M", "default": "a",
            "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] }));
        assert_eq!(validate_value(&mode, &json!("b")), Ok(SettingValue::Text("b".into())));
        assert_eq!(validate_value(&mode, &json!("c")), Err(ValueProblem::NotAnOption));
        assert_eq!(validate_value(&mode, &json!(1)), Err(ValueProblem::WrongType));
    }

    #[test]
    fn every_setting_has_a_value_what_was_kept_where_it_is_good_and_the_default_where_it_is_not() {
        let specs = vec![
            spec_of(number()),
            spec_of(json!({ "id": "enabled", "type": "boolean", "label": "E", "default": true })),
            spec_of(json!({ "id": "name", "type": "text", "label": "N", "default": "x", "max_length": 5 })),
        ];
        // Nothing kept (or what was kept is not even an object): the defaults.
        let defaults: BTreeMap<String, SettingValue> = [
            ("level".to_string(), SettingValue::Number(5)),
            ("enabled".to_string(), SettingValue::Boolean(true)),
            ("name".to_string(), SettingValue::Text("x".into())),
        ]
        .into();
        assert_eq!(values_of(&specs, &Value::Null), defaults);
        assert_eq!(values_of(&specs, &json!([1, 2])), defaults);
        // What is good is kept; what is not (the wrong kind, out of the limits) is the default for that setting alone; what is not declared is dropped.
        let kept = json!({ "level": 9, "enabled": "yes", "name": "toolongname", "gone": 1 });
        let values = values_of(&specs, &kept);
        assert_eq!(values.get("level"), Some(&SettingValue::Number(9)));
        assert_eq!(values.get("enabled"), Some(&SettingValue::Boolean(true)));
        assert_eq!(values.get("name"), Some(&SettingValue::Text("x".into())));
        assert_eq!(values.len(), 3);
        assert_eq!(default_of(&specs[0]), SettingValue::Number(5));
    }
}
