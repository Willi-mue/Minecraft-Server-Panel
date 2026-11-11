//! Game rules of a world, read and written without hard-coding the list.
//!
//! Minecraft renamed them over time, so names and values always come from the world itself:
//! - 26.x: `<world>/dimensions/minecraft/overworld/data/minecraft/game_rules.dat`, `data` compound
//!   with typed values (byte = boolean, int) and ids like `minecraft:keep_inventory`.
//! - Before: level.dat `Data.GameRules`, string values ("true", "3"), names like `keepInventory`.
//!
//! While the server runs, values are read with `gamerule <name>` and changed with
//! `gamerule <name> <value>`; while it is stopped, the file is edited.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::backup;
use crate::error::{Error, Result};
use crate::instances::Instance;
use crate::invalid;
use crate::nbt::{self, Compound, Tag};

static NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9_.\-]+:[a-z0-9_./\-]+$|^[A-Za-z0-9_]{1,64}$").unwrap());
// "Gamerule keepInventory is currently set to: false" / "... is now set to: true"
pub static ANSWER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\]: (?:System chat: )?Game ?rule ([\w:./-]+) is (?:currently|now) set to:? (\S+)",
    )
    .unwrap()
});

/// A rule value: boolean or whole number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule {
    Bool(bool),
    Int(i64),
}

impl Rule {
    pub fn text(self) -> String {
        match self {
            Rule::Bool(b) => if b { "true" } else { "false" }.into(),
            Rule::Int(n) => n.to_string(),
        }
    }

    fn json(self) -> Value {
        match self {
            Rule::Bool(b) => json!(b),
            Rule::Int(n) => json!(n),
        }
    }

    pub fn same_type(self, other: Rule) -> bool {
        matches!(
            (self, other),
            (Rule::Bool(_), Rule::Bool(_)) | (Rule::Int(_), Rule::Int(_))
        )
    }
}

// Well-known rules with a plain description; matched by `key()` (old camelCase and new snake_case ids)
const KNOWN: &[(&[&str], &str)] = &[
    (
        &["keepInventory", "keep_inventory"],
        "Keep inventory after death",
    ),
    (&["doDaylightCycle", "advance_time"], "Day/night cycle"),
    (&["doWeatherCycle", "advance_weather"], "Weather changes"),
    (&["doMobSpawning", "spawn_mobs"], "Mobs spawn"),
    (&["spawn_monsters"], "Monsters spawn"),
    (
        &["mobGriefing", "mob_griefing"],
        "Mobs change blocks (creepers, endermen)",
    ),
    (&["pvp"], "Players can hurt each other (PvP)"),
    (
        &["playersSleepingPercentage", "players_sleeping_percentage"],
        "Players that must sleep to skip the night (%)",
    ),
    (
        &["naturalRegeneration", "natural_health_regeneration"],
        "Health regenerates when fed",
    ),
    (
        &["doImmediateRespawn", "immediate_respawn"],
        "Respawn without the death screen",
    ),
    (
        &["showDeathMessages", "show_death_messages"],
        "Death messages in chat",
    ),
    (
        &["announceAdvancements", "show_advancement_messages"],
        "Advancement messages in chat",
    ),
    (&["doInsomnia", "spawn_phantoms"], "Phantoms spawn"),
    (&["doFireTick"], "Fire spreads"),
    (
        &["fire_spread_radius_around_player"],
        "Fire spreads within this distance of players (0 = never)",
    ),
    (&["tntExplodes", "tnt_explodes"], "TNT explodes"),
    (&["fallDamage", "fall_damage"], "Fall damage"),
    (
        &["randomTickSpeed", "random_tick_speed"],
        "Random tick speed (crop growth, 3 = normal)",
    ),
    (
        &["spawnRadius", "respawn_radius"],
        "Respawn radius around the world spawn",
    ),
    (
        &["doTraderSpawning", "spawn_wandering_traders"],
        "Wandering traders spawn",
    ),
    (
        &["doPatrolSpawning", "spawn_patrols"],
        "Pillager patrols spawn",
    ),
    (&["disableRaids", "raids"], "Raids"),
    (&["locatorBar", "locator_bar"], "Locator bar"),
    (
        &["sendCommandFeedback", "send_command_feedback"],
        "Command feedback in chat",
    ),
    (
        &["commandBlockOutput", "command_block_output"],
        "Command blocks report in chat",
    ),
];

fn key(name: &str) -> String {
    name.rsplit(':')
        .next()
        .unwrap_or(name)
        .replace('_', "")
        .to_lowercase()
}

fn known(name: &str) -> Option<(usize, &'static str)> {
    let k = key(name);
    KNOWN
        .iter()
        .enumerate()
        .find(|(_, (names, _))| names.iter().any(|n| key(n) == k))
        .map(|(i, (_, l))| (i, *l))
}

fn world(inst: &Instance) -> PathBuf {
    inst.server_dir().join(backup::level_name(inst))
}

fn new_file(inst: &Instance) -> PathBuf {
    world(inst).join("dimensions/minecraft/overworld/data/minecraft/game_rules.dat")
}

/// Where this world keeps its rules: (file, root name, root, path to the rules compound).
fn source(inst: &Instance) -> Result<Option<(PathBuf, String, Compound, bool)>> {
    let new = new_file(inst);
    if new.is_file() {
        let (name, root) = nbt::load(&new)?;
        if root.compound("data").is_some() {
            return Ok(Some((new, name, root, true)));
        }
    }
    let old = world(inst).join("level.dat");
    if old.is_file() {
        let (name, root) = nbt::load(&old)?;
        if root
            .compound("Data")
            .and_then(|d| d.compound("GameRules"))
            .is_some()
        {
            return Ok(Some((old, name, root, false)));
        }
    }
    Ok(None)
}

fn rules_of(root: &Compound, new: bool) -> Option<&Compound> {
    if new {
        root.compound("data")
    } else {
        root.compound("Data")?.compound("GameRules")
    }
}

fn rules_of_mut(root: &mut Compound, new: bool) -> &mut Compound {
    if new {
        root.compound_mut("data")
    } else {
        root.compound_mut("Data").compound_mut("GameRules")
    }
}

fn typed(tag: &Tag) -> Option<Rule> {
    match tag {
        Tag::Byte(v) => Some(Rule::Bool(*v != 0)),
        Tag::Int(_) | Tag::Short(_) | Tag::Long(_) => tag.as_i64().map(Rule::Int),
        Tag::String(s) => parse_value(s),
        _ => None,
    }
}

pub fn parse_value(text: &str) -> Option<Rule> {
    let text = text.trim().trim_end_matches('.');
    match text.to_lowercase().as_str() {
        "true" => Some(Rule::Bool(true)),
        "false" => Some(Rule::Bool(false)),
        _ => text.parse::<i64>().ok().map(Rule::Int),
    }
}

/// name -> value as stored in the world (as of its last save), in file order. Empty without a world.
pub fn read(inst: &Instance) -> Vec<(String, Rule)> {
    let Ok(Some((_, _, root, new))) = source(inst) else {
        return vec![];
    };
    rules_of(&root, new)
        .map(|rules| {
            rules
                .iter()
                .filter_map(|(n, t)| typed(t).map(|v| (n.clone(), v)))
                .collect()
        })
        .unwrap_or_default()
}

/// The id to use in `/gamerule` (the default namespace may be left out).
pub fn command_name(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// Rules for the UI: well-known ones first (with a label), then the rest alphabetically.
pub fn describe(rules: &[(String, Rule)]) -> Value {
    let mut out: Vec<(bool, usize, String, Value)> = rules
        .iter()
        .map(|(n, v)| {
            let k = known(n);
            let entry = json!({
                "name": n, "command": command_name(n), "value": v.json(),
                "type": if matches!(v, Rule::Bool(_)) { "bool" } else { "int" },
                "label": k.map(|(_, l)| l),
            });
            (
                k.is_none(),
                k.map(|(i, _)| i).unwrap_or(0),
                n.to_lowercase(),
                entry,
            )
        })
        .collect();
    out.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    Value::Array(out.into_iter().map(|e| e.3).collect())
}

/// Check a change against the rules the world has. Returns the value to set.
pub fn validate(rules: &[(String, Rule)], name: &str, value: &Value) -> Result<Rule> {
    let current = rules.iter().find(|(n, _)| n == name).map(|(_, v)| *v);
    let Some(current) = current.filter(|_| NAME_RE.is_match(name)) else {
        return invalid!("Unknown game rule: {name}");
    };
    match current {
        Rule::Bool(_) => value
            .as_bool()
            .map(Rule::Bool)
            .ok_or_else(|| Error::Invalid(format!("{name}: true or false"))),
        Rule::Int(_) => match value.as_i64() {
            Some(n) if value.is_i64() && (i32::MIN as i64..=i32::MAX as i64).contains(&n) => {
                Ok(Rule::Int(n))
            }
            _ => invalid!("{name}: a whole number"),
        },
    }
}

/// Change one rule in the world file (server stopped). Keeps the stored type.
pub fn write_offline(inst: &Instance, name: &str, value: Rule) -> Result<()> {
    let Some((path, root_name, mut root, new)) = source(inst)? else {
        return invalid!("The world doesn't exist yet: start the server once");
    };
    let rules = rules_of_mut(&mut root, new);
    let tag = match rules.get(name) {
        None => return invalid!("Unknown game rule: {name}"),
        Some(Tag::String(_)) => Tag::String(value.text()),
        Some(Tag::Byte(_)) => Tag::Byte(matches!(value, Rule::Bool(true)) as i8),
        Some(Tag::Short(_)) => Tag::Short(int_of(value) as i16),
        Some(Tag::Long(_)) => Tag::Long(int_of(value)),
        Some(_) => Tag::Int(int_of(value) as i32),
    };
    rules.insert(name, tag);
    nbt::save(&path, &root_name, &root)?;
    Ok(())
}

fn int_of(v: Rule) -> i64 {
    match v {
        Rule::Bool(b) => b as i64,
        Rule::Int(n) => n,
    }
}

/// Answers of `gamerule <name>` lines: name -> value.
pub fn answers(lines: &[String]) -> HashMap<String, Rule> {
    let mut found = HashMap::new();
    for line in lines {
        if let Some(c) = ANSWER_RE.captures(line) {
            if let Some(v) = parse_value(&c[2]) {
                found.insert(c[1].to_string(), v);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_world(inst: &Instance) {
        let mut rules = Compound::default();
        rules.insert("keepInventory", Tag::String("false".into()));
        rules.insert("randomTickSpeed", Tag::String("3".into()));
        rules.insert("zzzCustom", Tag::String("true".into()));
        let mut data = Compound::default();
        data.insert("GameRules", Tag::Compound(rules));
        let root = Compound(vec![("Data".into(), Tag::Compound(data))]);
        let p = world(inst).join("level.dat");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        nbt::save(&p, "", &root).unwrap();
    }

    #[test]
    fn old_level_dat() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        assert!(read(&inst).is_empty());
        old_world(&inst);
        let rules = read(&inst);
        assert_eq!(rules[0], ("keepInventory".into(), Rule::Bool(false)));
        let d = describe(&rules);
        assert_eq!(d[0]["name"], json!("keepInventory"));
        assert_eq!(d[0]["label"], json!("Keep inventory after death"));
        assert_eq!(d[2]["label"], Value::Null);
        assert_eq!(
            validate(&rules, "randomTickSpeed", &json!(5)).unwrap(),
            Rule::Int(5)
        );
        assert!(validate(&rules, "randomTickSpeed", &json!(true)).is_err());
        assert!(validate(&rules, "keepInventory", &json!(1)).is_err());
        assert!(validate(&rules, "nope", &json!(1)).is_err());
        write_offline(&inst, "keepInventory", Rule::Bool(true)).unwrap();
        assert_eq!(read(&inst)[0].1, Rule::Bool(true));
        let (_, root) = nbt::load(&world(&inst).join("level.dat")).unwrap();
        assert_eq!(
            root.compound("Data")
                .unwrap()
                .compound("GameRules")
                .unwrap()
                .string("keepInventory"),
            Some("true")
        );
    }

    #[test]
    fn new_typed_file() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let mut data = Compound::default();
        data.insert("minecraft:keep_inventory", Tag::Byte(0));
        data.insert("minecraft:random_tick_speed", Tag::Int(3));
        let root = Compound(vec![("data".into(), Tag::Compound(data))]);
        std::fs::create_dir_all(new_file(&inst).parent().unwrap()).unwrap();
        nbt::save(&new_file(&inst), "", &root).unwrap();
        write_offline(&inst, "minecraft:random_tick_speed", Rule::Int(10)).unwrap();
        write_offline(&inst, "minecraft:keep_inventory", Rule::Bool(true)).unwrap();
        let rules = read(&inst);
        assert_eq!(
            rules,
            vec![
                ("minecraft:keep_inventory".into(), Rule::Bool(true)),
                ("minecraft:random_tick_speed".into(), Rule::Int(10))
            ]
        );
        assert_eq!(command_name("minecraft:keep_inventory"), "keep_inventory");
    }

    #[test]
    fn console_answers() {
        let lines = vec![
            "[12:00:00 INFO]: Gamerule keepInventory is currently set to: false".to_string(),
            "[12:00:00] [Server thread/INFO]: System chat: Game rule keep_inventory is now set to: true".to_string(),
        ];
        let a = answers(&lines);
        assert_eq!(a["keepInventory"], Rule::Bool(false));
        assert_eq!(a["keep_inventory"], Rule::Bool(true));
    }
}
