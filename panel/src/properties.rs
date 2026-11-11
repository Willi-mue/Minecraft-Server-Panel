//! Editable subset of server.properties (comments, order and other keys are preserved).

use serde_json::{Map, Value, json};

use crate::access;
use crate::error::Result;
use crate::instances::Instance;
use crate::invalid;

/// Written to server.properties before the first start if missing (panel defaults, differ from Minecraft's)
const PRESETS: [(&str, &str); 2] = [("view-distance", "12"), ("simulation-distance", "12")];

pub enum Rule {
    Str(usize),
    Enum(&'static [&'static str]),
    Int(i64, i64),
    Bool,
}

/// key, default (what the server uses if the key is missing), constraint
pub fn fields() -> Vec<(&'static str, Value, Rule)> {
    vec![
        ("motd", json!("A Minecraft Server"), Rule::Str(120)),
        (
            "difficulty",
            json!("easy"),
            Rule::Enum(&["peaceful", "easy", "normal", "hard"]),
        ),
        (
            "gamemode",
            json!("survival"),
            Rule::Enum(&["survival", "creative", "adventure", "spectator"]),
        ),
        ("max-players", json!(20), Rule::Int(1, 1000)),
        // The two biggest performance levers: chunks sent to / ticked around each player
        ("view-distance", json!(12), Rule::Int(2, 32)),
        ("simulation-distance", json!(12), Rule::Int(2, 32)),
        ("spawn-protection", json!(16), Rule::Int(0, 1000)),
        ("allow-flight", json!(false), Rule::Bool),
        ("hardcore", json!(false), Rule::Bool),
    ]
}

/// The field descriptions for the UI: {key: {type, default, rule}}.
pub fn describe() -> Value {
    let mut out = Map::new();
    for (key, default, rule) in fields() {
        let (kind, rule) = match rule {
            Rule::Str(n) => ("str", json!(n)),
            Rule::Enum(values) => ("enum", json!(values)),
            Rule::Int(lo, hi) => ("int", json!([lo, hi])),
            Rule::Bool => ("bool", Value::Null),
        };
        out.insert(
            key.into(),
            json!({"type": kind, "default": default, "rule": rule}),
        );
    }
    Value::Object(out)
}

fn parse(rule: &Rule, default: &Value, raw: &str) -> Value {
    match rule {
        Rule::Int(..) => raw
            .trim()
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| default.clone()),
        Rule::Bool => json!(raw.trim().to_lowercase() == "true"),
        _ => json!(raw),
    }
}

pub fn get_all(inst: &Instance) -> Value {
    let mut out = Map::new();
    for (key, default, rule) in fields() {
        let value = match access::get_property(inst, key) {
            None => default,
            Some(raw) => parse(&rule, &default, &raw),
        };
        out.insert(key.into(), value);
    }
    Value::Object(out)
}

/// The value as it is written to the file, or an error.
pub fn validate(key: &str, value: &Value) -> Result<String> {
    let Some((_, _, rule)) = fields().into_iter().find(|(k, _, _)| *k == key) else {
        return invalid!("{key} can't be changed here");
    };
    match rule {
        Rule::Str(max) => match value.as_str() {
            Some(s) if s.chars().count() <= max && !s.contains(['\n', '\r']) => Ok(s.to_string()),
            _ => invalid!("{key}: text up to {max} characters, single line"),
        },
        Rule::Enum(values) => match value.as_str() {
            Some(s) if values.contains(&s) => Ok(s.to_string()),
            _ => invalid!("{key}: one of {}", values.join(", ")),
        },
        Rule::Int(lo, hi) => match value.as_i64() {
            Some(n) if value.is_i64() && lo <= n && n <= hi => Ok(n.to_string()),
            _ => invalid!("{key}: whole number from {lo} to {hi}"),
        },
        Rule::Bool => match value.as_bool() {
            Some(b) => Ok(if b { "true" } else { "false" }.into()),
            None => invalid!("{key}: true or false"),
        },
    }
}

/// Write PRESETS for keys not yet in server.properties. Returns the keys written.
pub fn apply_presets(inst: &Instance) -> std::io::Result<Vec<String>> {
    let mut written = vec![];
    for (key, value) in PRESETS {
        if access::get_property(inst, key).is_none() {
            access::set_property(inst, key, value)?;
            written.push(key.to_string());
        }
    }
    Ok(written)
}

pub fn update(inst: &Instance, values: &Map<String, Value>) -> Result<Value> {
    // all or nothing
    let checked: Vec<(String, String)> = values
        .iter()
        .map(|(k, v)| validate(k, v).map(|t| (k.clone(), t)))
        .collect::<Result<_>>()?;
    for (key, text) in checked {
        access::set_property(inst, &key, &text)?;
    }
    Ok(get_all(inst))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_validate_update() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        assert_eq!(get_all(&inst)["view-distance"], json!(12));
        assert_eq!(
            apply_presets(&inst).unwrap(),
            vec!["view-distance", "simulation-distance"]
        );
        assert!(apply_presets(&inst).unwrap().is_empty());
        access::set_property(&inst, "max-players", "abc").unwrap();
        assert_eq!(get_all(&inst)["max-players"], json!(20));

        let mut v = Map::new();
        v.insert("motd".into(), json!("Hello"));
        v.insert("hardcore".into(), json!(true));
        let all = update(&inst, &v).unwrap();
        assert_eq!(all["motd"], json!("Hello"));
        assert_eq!(all["hardcore"], json!(true));

        assert!(validate("level-seed", &json!("x")).is_err());
        assert!(validate("max-players", &json!(true)).is_err());
        assert!(validate("max-players", &json!(1.5)).is_err());
        assert!(validate("max-players", &json!(0)).is_err());
        assert!(validate("motd", &json!("a\nb")).is_err());
        assert!(validate("difficulty", &json!("insane")).is_err());
        let mut bad = Map::new();
        bad.insert("motd".into(), json!("ok"));
        bad.insert("difficulty".into(), json!("insane"));
        assert!(update(&inst, &bad).is_err());
        assert_eq!(get_all(&inst)["motd"], json!("Hello"), "all or nothing");
    }
}
