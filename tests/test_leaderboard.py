import json
import zipfile

from app import config, instances, leaderboard


def _world(data, players):
    """players: name -> (uuid, done advancement ids, custom stats)."""
    server = data / "server"
    w = server / "world" / "players"
    (w / "stats").mkdir(parents=True)
    (w / "advancements").mkdir(parents=True)
    cache = []
    for name, (uuid, done, custom) in players.items():
        cache.append({"name": name, "uuid": uuid})
        advs = {a: {"criteria": {}, "done": True} for a in done}
        advs["minecraft:recipes/misc/charcoal"] = {"done": True}  # recipes never count
        advs["minecraft:story/smelt_iron"] = {"done": False}       # not finished
        (w / "advancements" / f"{uuid}.json").write_text(json.dumps(advs))
        (w / "stats" / f"{uuid}.json").write_text(json.dumps({"stats": {
            "minecraft:custom": custom, "minecraft:mined": {"minecraft:stone": 10, "minecraft:dirt": 5}}}))
    (server / "usercache.json").write_text(json.dumps(cache))


def _jar(data, version="1.21.4", lang=None):
    (data / "cache").mkdir(exist_ok=True)
    path = data / "cache" / f"client-{version}.jar"
    with zipfile.ZipFile(path, "w") as z:
        adv = lambda key, frame=None: json.dumps({"display": {
            "title": {"translate": f"advancements.{key}.title"},
            "description": {"translate": f"advancements.{key}.description"}, **({"frame": frame} if frame else {})}})
        z.writestr("data/minecraft/advancement/story/mine_stone.json", adv("story.mine_stone"))
        z.writestr("data/minecraft/advancement/story/smelt_iron.json", adv("story.smelt_iron"))
        z.writestr("data/minecraft/advancement/nether/all_effects.json", adv("nether.all_effects", "challenge"))
        z.writestr("data/minecraft/advancement/end/levitate.json", adv("end.levitate", "challenge"))
        z.writestr("data/minecraft/advancement/recipes/misc/charcoal.json", adv("x"))
        z.writestr("data/minecraft/advancement/technical.json", json.dumps({"criteria": {}}))  # no display
        z.writestr("assets/minecraft/lang/en_us.json", json.dumps(lang or {
            "advancements.story.mine_stone.title": "Stone Age",
            "advancements.story.mine_stone.description": "Mine Stone with your new Pickaxe",
            "advancements.nether.all_effects.title": "How Did We Get Here?"}))
    (data / "settings.json").write_text(json.dumps({"version": version, "world_version": version}))
    return path


A = "11111111-1111-1111-1111-111111111111"
B = "22222222-2222-2222-2222-222222222222"


def test_catalog_reads_titles_frames_and_skips_recipes(data):
    cat = leaderboard.catalog(_jar(data))
    assert set(cat) == {"minecraft:story/mine_stone", "minecraft:story/smelt_iron",
                        "minecraft:nether/all_effects", "minecraft:end/levitate"}
    assert cat["minecraft:story/mine_stone"] == {"title": "Stone Age", "description": "Mine Stone with your new Pickaxe",
                                                 "frame": "task", "category": "story"}
    assert cat["minecraft:nether/all_effects"]["frame"] == "challenge"
    assert cat["minecraft:story/smelt_iron"]["title"] == "Smelt iron"  # no translation: from the id


def test_tiers_records_and_what_is_public(data):
    _jar(data)
    _world(data, {
        "Alex": (A, ["minecraft:story/mine_stone", "minecraft:nether/all_effects", "minecraft:end/levitate"],
                 {"minecraft:play_time": 72000 * 3, "minecraft:deaths": 2, "minecraft:walk_one_cm": 250_000}),
        "Steve": (B, ["minecraft:story/mine_stone", "minecraft:not/in_this_version"], {"minecraft:play_time": 72000}),
    })
    leaderboard._cache.clear()
    r = leaderboard.build(instances.MAIN)
    assert r["total"] == 4
    alex, steve = r["players"]  # best first
    assert (alex["name"], alex["tier"], alex["share"]) == ("Alex", "S", 0.75)
    assert (steve["name"], steve["tier"], steve["share"]) == ("Steve", "B", 0.25)  # unknown id dropped
    st = alex["stats"]
    assert (st["play_time"], st["deaths"], st["walked"], st["mined"], st["advancements"], st["challenges"]) == (3.0, 2, 2500, 15, 3, 2)
    assert set(st) >= {r["key"] for r in r["records"]}  # every record has a value
    assert alex["done"] == ["minecraft:end/levitate", "minecraft:nether/all_effects", "minecraft:story/mine_stone"]
    # only what a tier list needs: no uuids beyond the skin reference, no positions
    assert set(alex) == {"name", "skin", "done", "stats", "share", "tier", "killed", "killed_by"}
    assert alex["skin"] is None  # no skin downloaded
    assert [t["tier"] for t in r["tiers"]] == ["S", "A", "B", "C", "D"]


def test_without_client_jar_and_unknown_players(data):
    _world(data, {"Alex": (A, ["minecraft:story/mine_stone"], {})})
    # a stats file of somebody not in usercache.json: no name, left out
    (data / "server" / "world" / "players" / "stats" / f"{B}.json").write_text("{}")
    leaderboard._cache.clear()
    r = leaderboard.build(instances.MAIN)
    assert [p["name"] for p in r["players"]] == ["Alex"]
    assert r["total"] == 1 and r["players"][0]["tier"] == "S"  # total = what players reached
    assert r["advancements"]["minecraft:story/mine_stone"]["title"] == "Mine stone"


def test_tier_boundaries():
    assert [leaderboard.tier_of(s) for s in (1, 0.6, 0.59, 0.4, 0.25, 0.1, 0.09, 0)] == ["S", "S", "A", "A", "B", "C", "D", "D"]
    assert config  # imported for the fixture's paths


def test_stats_of_movement_combat_mining_and_mobs():
    stats, killed, killed_by = leaderboard.stats_of({"stats": {
        "minecraft:custom": {"minecraft:sprint_one_cm": 123_456, "minecraft:swim_one_cm": 1000,
                             "minecraft:walk_under_water_one_cm": 500, "minecraft:aviate_one_cm": 2_000_000,
                             "minecraft:damage_dealt": 205, "minecraft:time_since_death": 72000 * 2,
                             "minecraft:fish_caught": 7, "minecraft:eat_cake_slice": 3, "minecraft:jump": 99},
        "minecraft:mined": {"minecraft:diamond_ore": 2, "minecraft:deepslate_diamond_ore": 3,
                            "minecraft:ancient_debris": 4, "minecraft:oak_log": 10, "minecraft:crimson_stem": 5,
                            "minecraft:stone": 100, "minecraft:coal_ore": 6},
        "minecraft:killed": {"minecraft:zombie": 12, "minecraft:creeper": 3},
        "minecraft:killed_by": {"minecraft:creeper": 2},
        "minecraft:broken": {"minecraft:wooden_pickaxe": 2},
    }})
    assert (stats["sprinted"], stats["swum"], stats["elytra"]) == (1235, 15, 20000)  # metres
    assert (stats["damage_dealt"], stats["alive"], stats["fish"], stats["cake"], stats["jumps"]) == (10, 2.0, 7, 3, 99)
    assert (stats["mined"], stats["diamonds"], stats["debris"], stats["logs"], stats["ores"]) == (130, 5, 4, 15, 15)
    assert stats["tools_broken"] == 2
    assert killed == {"minecraft:zombie": 12, "minecraft:creeper": 3} and killed_by == {"minecraft:creeper": 2}
    assert leaderboard.stats_of({})[0]["walked"] == 0  # no file: all zero


def test_mob_names_from_the_lang_file(data):
    _jar(data, lang={"entity.minecraft.zombie": "Zombie"})
    _world(data, {"Alex": (A, [], {})})
    stats = data / "server" / "world" / "players" / "stats" / f"{A}.json"
    stats.write_text(json.dumps({"stats": {"minecraft:killed": {"minecraft:zombie": 1, "minecraft:mooshroom": 2}}}))
    leaderboard._cache.clear()
    leaderboard._jar_cache.clear()
    r = leaderboard.build(instances.MAIN)
    assert r["mobs"]["minecraft:zombie"] == "Zombie"
    assert r["mobs"]["minecraft:mooshroom"] == "Mooshroom"  # not in the lang file: from the id
    assert r["saving"] is False  # server not running: nothing to save


def test_me_by_login_address():
    from app.server import LOGIN_RE
    inst = instances.Instance("other")
    lines = {
        "Alex": "[12:00:00] [Server thread/INFO]: Alex[/192.168.1.20:39094] logged in with entity id 54 at (x)",
        "Steve": "[12:00:01] [Server thread/INFO]: Steve[/[0:0:0:0:0:0:0:1]:5000] logged in with entity id 7 at (x)",
    }
    for name, line in lines.items():
        m = LOGIN_RE.search(line)
        assert m.group(1) == name
        leaderboard.remember_login(inst, m.group(1), m.group(2))
    assert leaderboard.me(inst, "192.168.1.20") == "Alex"
    assert leaderboard.me(inst, "::ffff:192.168.1.20") == "Alex"  # IPv4 seen through an IPv6 socket
    assert leaderboard.me(inst, "::1") == "Steve"
    assert leaderboard.me(inst, "10.0.0.9") is None and leaderboard.me(inst, None) is None
    # two players behind the same address: no guess
    leaderboard.remember_login(inst, "Herobrine", "192.168.1.20")
    assert leaderboard.me(inst, "192.168.1.20") is None
    assert leaderboard.me(instances.MAIN, "::1") is None  # per server
