//! Map-item style colors: Minecraft's 61 map colors ("MapColor") with 4 brightness levels,
//! and a block name -> map color mapping. No textures / client assets are needed.

/// Map color ids (index into [`BASE`]). 0 = no color (transparent, look through).
pub const NONE: u8 = 0;
pub const GRASS: u8 = 1;
pub const SAND: u8 = 2;
pub const FIRE: u8 = 4;
pub const ICE: u8 = 5;
pub const METAL: u8 = 6;
pub const PLANT: u8 = 7;
pub const SNOW: u8 = 8;
pub const CLAY: u8 = 9;
pub const DIRT: u8 = 10;
pub const STONE: u8 = 11;
pub const WATER: u8 = 12;
pub const WOOD: u8 = 13;
pub const QUARTZ: u8 = 14;
pub const ORANGE: u8 = 15;
pub const MAGENTA: u8 = 16;
pub const LIGHT_BLUE: u8 = 17;
pub const YELLOW: u8 = 18;
pub const LIGHT_GREEN: u8 = 19;
pub const PINK: u8 = 20;
pub const GRAY: u8 = 21;
pub const LIGHT_GRAY: u8 = 22;
pub const CYAN: u8 = 23;
pub const PURPLE: u8 = 24;
pub const BLUE: u8 = 25;
pub const BROWN: u8 = 26;
pub const GREEN: u8 = 27;
pub const RED: u8 = 28;
pub const BLACK: u8 = 29;
pub const GOLD: u8 = 30;
pub const DIAMOND: u8 = 31;
pub const LAPIS: u8 = 32;
pub const EMERALD: u8 = 33;
pub const PODZOL: u8 = 34;
pub const NETHER: u8 = 35;
pub const TERRACOTTA_WHITE: u8 = 36;
pub const TERRACOTTA_LIGHT_GRAY: u8 = 44;
pub const TERRACOTTA_CYAN: u8 = 45;
pub const TERRACOTTA_GRAY: u8 = 43;
pub const TERRACOTTA_BROWN: u8 = 48;
pub const CRIMSON_NYLIUM: u8 = 52;
pub const CRIMSON_STEM: u8 = 53;
pub const CRIMSON_HYPHAE: u8 = 54;
pub const WARPED_NYLIUM: u8 = 55;
pub const WARPED_STEM: u8 = 56;
pub const WARPED_HYPHAE: u8 = 57;
pub const WARPED_WART_BLOCK: u8 = 58;
pub const DEEPSLATE: u8 = 59;
pub const RAW_IRON: u8 = 60;
pub const GLOW_LICHEN: u8 = 61;

/// Base RGB of every map color, as defined by Minecraft.
pub const BASE: [[u8; 3]; 62] = [
    [0, 0, 0],       // 0 none
    [127, 178, 56],  // 1 grass
    [247, 233, 163], // 2 sand
    [199, 199, 199], // 3 wool
    [255, 0, 0],     // 4 fire
    [160, 160, 255], // 5 ice
    [167, 167, 167], // 6 metal
    [0, 124, 0],     // 7 plant
    [255, 255, 255], // 8 snow
    [164, 168, 184], // 9 clay
    [151, 109, 77],  // 10 dirt
    [112, 112, 112], // 11 stone
    [64, 64, 255],   // 12 water
    [143, 119, 72],  // 13 wood
    [255, 252, 245], // 14 quartz
    [216, 127, 51],  // 15 orange
    [178, 76, 216],  // 16 magenta
    [102, 153, 216], // 17 light blue
    [229, 229, 51],  // 18 yellow
    [127, 204, 25],  // 19 light green
    [242, 127, 165], // 20 pink
    [76, 76, 76],    // 21 gray
    [153, 153, 153], // 22 light gray
    [76, 127, 153],  // 23 cyan
    [127, 63, 178],  // 24 purple
    [51, 76, 178],   // 25 blue
    [102, 76, 51],   // 26 brown
    [102, 127, 51],  // 27 green
    [153, 51, 51],   // 28 red
    [25, 25, 25],    // 29 black
    [250, 238, 77],  // 30 gold
    [92, 219, 213],  // 31 diamond
    [74, 128, 255],  // 32 lapis
    [0, 217, 58],    // 33 emerald
    [129, 86, 49],   // 34 podzol
    [112, 2, 0],     // 35 nether
    [209, 177, 161], // 36 terracotta white
    [159, 82, 36],   // 37 terracotta orange
    [149, 87, 108],  // 38 terracotta magenta
    [112, 108, 138], // 39 terracotta light blue
    [186, 133, 36],  // 40 terracotta yellow
    [103, 117, 53],  // 41 terracotta light green
    [160, 77, 78],   // 42 terracotta pink
    [57, 41, 35],    // 43 terracotta gray
    [135, 107, 98],  // 44 terracotta light gray
    [87, 92, 92],    // 45 terracotta cyan
    [122, 73, 88],   // 46 terracotta purple
    [76, 62, 92],    // 47 terracotta blue
    [76, 50, 35],    // 48 terracotta brown
    [76, 82, 42],    // 49 terracotta green
    [142, 60, 46],   // 50 terracotta red
    [37, 22, 16],    // 51 terracotta black
    [189, 48, 49],   // 52 crimson nylium
    [148, 63, 97],   // 53 crimson stem
    [92, 25, 29],    // 54 crimson hyphae
    [22, 126, 134],  // 55 warped nylium
    [58, 142, 140],  // 56 warped stem
    [86, 44, 62],    // 57 warped hyphae
    [20, 180, 133],  // 58 warped wart block
    [100, 100, 100], // 59 deepslate
    [216, 175, 147], // 60 raw iron
    [127, 167, 150], // 61 glow lichen
];

/// Brightness levels used on maps: LOW, NORMAL, HIGH, LOWEST.
pub const LOW: u8 = 0;
pub const NORMAL: u8 = 1;
pub const HIGH: u8 = 2;
const MULT: [u32; 4] = [180, 220, 255, 135];

pub fn shaded(color: u8, brightness: u8) -> [u8; 3] {
    scale(BASE[color as usize], brightness)
}

/// Real block colour with the map's relief shading, flat ground (NORMAL) unchanged.
pub fn relief(rgb: [u8; 3], brightness: u8) -> [u8; 3] {
    let m = MULT[brightness as usize & 3];
    rgb.map(|c| (c as u32 * m / MULT[NORMAL as usize]).min(255) as u8)
}

/// Any colour with one of the map brightness levels.
pub fn scale(rgb: [u8; 3], brightness: u8) -> [u8; 3] {
    let m = MULT[brightness as usize & 3];
    rgb.map(|c| (c as u32 * m / 255) as u8)
}

const DYES: [(&str, u8, u8); 16] = [
    // (dye, map color, terracotta map color)
    ("white", SNOW, 36),
    ("orange", ORANGE, 37),
    ("magenta", MAGENTA, 38),
    ("light_blue", LIGHT_BLUE, 39),
    ("yellow", YELLOW, 40),
    ("lime", LIGHT_GREEN, 41),
    ("pink", PINK, 42),
    ("light_gray", LIGHT_GRAY, 44),
    ("gray", GRAY, 43),
    ("cyan", CYAN, 45),
    ("purple", PURPLE, 46),
    ("blue", BLUE, 47),
    ("brown", BROWN, 48),
    ("green", GREEN, 49),
    ("red", RED, 50),
    ("black", BLACK, 51),
];

/// Wood types -> color of their planks / stripped logs (top view).
const WOODS: [(&str, u8); 12] = [
    ("pale_oak", QUARTZ),
    ("dark_oak", BROWN),
    ("oak", WOOD),
    ("spruce", PODZOL),
    ("birch", SAND),
    ("jungle", DIRT),
    ("acacia", ORANGE),
    ("mangrove", RED),
    ("cherry", TERRACOTTA_WHITE),
    ("bamboo", YELLOW),
    ("crimson", CRIMSON_STEM),
    ("warped", WARPED_STEM),
];

/// Small things a map looks through (like Minecraft maps do).
const TRANSPARENT: &[&str] = &[
    "air", "cave_air", "void_air", "glass", "glass_pane", "barrier", "light", "structure_void",
    "torch", "wall_torch", "soul_torch", "soul_wall_torch", "redstone_torch", "redstone_wall_torch",
    "copper_torch", "copper_wall_torch", "lever", "tripwire", "tripwire_hook", "redstone_wire",
    "rail", "powered_rail", "detector_rail", "activator_rail", "iron_bars", "chain", "end_rod",
    "ladder", "flower_pot", "moving_piston", "string", "scaffolding", "cobweb", "repeater",
    "comparator", "skeleton_skull", "wither_skeleton_skull", "zombie_head", "player_head",
    "creeper_head", "dragon_head", "piglin_head", "skeleton_wall_skull", "zombie_wall_head",
    "player_wall_head", "creeper_wall_head", "dragon_wall_head", "piglin_wall_head",
    "wither_skeleton_wall_skull", "heavy_core",
];

/// Blocks that count as water for the depth shading (water itself and underwater plants).
pub fn is_water(name: &str) -> bool {
    matches!(
        name,
        "water" | "bubble_column" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass"
    )
}

/// Map color of a block (name without "minecraft:" namespace). Non-vanilla namespaces get
/// [`STONE`] so modded blocks are still visible.
pub fn map_color(full_name: &str) -> u8 {
    let Some(name) = full_name.strip_prefix("minecraft:") else {
        return if full_name.contains(':') { STONE } else { map_color_plain(full_name) };
    };
    map_color_plain(name)
}

fn map_color_plain(name: &str) -> u8 {
    if TRANSPARENT.contains(&name)
        || name.ends_with("_button")
        || name.ends_with("_candle")
        || name == "candle"
        || name.starts_with("potted_")
    {
        return NONE;
    }
    if is_water(name) {
        return WATER;
    }

    // Exact names and plants first ("red_sand", "blue_ice", "red_tulip" are not dyed blocks;
    // maps draw flowers green)
    if let Some(c) = exact(name) {
        return c;
    }
    if is_plant(name) {
        return PLANT;
    }

    // Colored blocks: wool, carpet, concrete, stained glass, terracotta, beds, banners, ...
    // ("light_blue"/"light_gray" are listed before "gray"/"blue" so prefixes can't clash)
    for (dye, color, terracotta) in DYES {
        if let Some(rest) = name.strip_prefix(dye).and_then(|r| r.strip_prefix('_')) {
            if rest.starts_with("sand") || rest.starts_with("nether") {
                break; // red_sandstone*, red_nether_bricks: handled by the families below
            }
            return match rest {
                "terracotta" => terracotta,
                _ => color,
            };
        }
    }

    // Wood and wood-like blocks
    for (wood, color) in WOODS {
        if name.starts_with(wood) || name.starts_with(&format!("stripped_{wood}")) {
            if name.ends_with("_leaves") {
                return match wood {
                    "cherry" => PINK,
                    "pale_oak" => LIGHT_GRAY,
                    _ => PLANT,
                };
            }
            if name.ends_with("_sapling") || name.ends_with("_propagule") {
                return PLANT;
            }
            if name.ends_with("_hyphae") {
                return if wood == "crimson" { CRIMSON_HYPHAE } else { WARPED_HYPHAE };
            }
            if name.ends_with("_nylium") {
                return if wood == "crimson" { CRIMSON_NYLIUM } else { WARPED_NYLIUM };
            }
            if name.ends_with("_fungus") || name.ends_with("_roots") {
                return if wood == "crimson" { NETHER } else { CYAN };
            }
            return color;
        }
    }
    if name.ends_with("_leaves") {
        return PLANT; // azalea etc.
    }

    // Stone families
    if name.contains("deepslate") {
        return DEEPSLATE;
    }
    if name.starts_with("tuff") || name.contains("_tuff") {
        return TERRACOTTA_GRAY;
    }
    if name.contains("blackstone") || name.contains("basalt") {
        return BLACK;
    }
    if name.starts_with("red_sandstone") || name.starts_with("smooth_red_sandstone") || name.starts_with("cut_red_sandstone") {
        return ORANGE;
    }
    if name.contains("sandstone") {
        return SAND;
    }
    if name.contains("end_stone") {
        return SAND;
    }
    if name.contains("purpur") {
        return MAGENTA;
    }
    if name.contains("quartz") && name != "nether_quartz_ore" {
        return QUARTZ;
    }
    if name.contains("nether_brick") {
        return NETHER;
    }
    if name.contains("prismarine") {
        return if name.starts_with("prismarine") && !name.starts_with("prismarine_brick") { CYAN } else { DIAMOND };
    }
    if name.contains("granite") {
        return DIRT;
    }
    if name.contains("diorite") {
        return QUARTZ;
    }
    if name.contains("mud_brick") {
        return TERRACOTTA_LIGHT_GRAY;
    }
    if name.contains("brick") && !name.contains("stone") {
        return RED; // bricks, brick stairs ...
    }
    if name.contains("copper") {
        return if name.contains("oxidized") {
            WARPED_NYLIUM
        } else if name.contains("weathered") {
            WARPED_STEM
        } else if name.contains("exposed") {
            TERRACOTTA_LIGHT_GRAY
        } else {
            ORANGE
        };
    }
    if name.ends_with("_ore") {
        return if name.starts_with("nether_") { NETHER } else { STONE };
    }
    if name.contains("coral") {
        return if name.starts_with("dead_") { GRAY } else { PINK };
    }
    if name.ends_with("_mushroom") {
        return if name.starts_with("red") { RED } else { BROWN };
    }
    if name.ends_with("_sign") || name.ends_with("_hanging_sign") || name.ends_with("_shelf") {
        return WOOD;
    }
    if is_plant(name) {
        return PLANT;
    }
    STONE
}

fn is_plant(name: &str) -> bool {
    matches!(
        name,
        "short_grass" | "grass" | "tall_grass" | "fern" | "large_fern" | "dandelion" | "poppy"
            | "blue_orchid" | "allium" | "azure_bluet" | "oxeye_daisy" | "cornflower"
            | "lily_of_the_valley" | "wither_rose" | "sunflower" | "lilac" | "rose_bush" | "peony"
            | "torchflower" | "torchflower_crop" | "pitcher_plant" | "pitcher_crop" | "pink_petals"
            | "wildflowers" | "leaf_litter" | "bush" | "firefly_bush" | "short_dry_grass"
            | "tall_dry_grass" | "open_eyeblossom" | "closed_eyeblossom" | "vine" | "cave_vines"
            | "cave_vines_plant" | "twisting_vines" | "twisting_vines_plant" | "weeping_vines"
            | "weeping_vines_plant" | "sugar_cane" | "cactus" | "cactus_flower" | "lily_pad"
            | "bamboo" | "bamboo_sapling" | "sweet_berry_bush" | "wheat" | "carrots" | "potatoes"
            | "beetroots" | "melon_stem" | "pumpkin_stem" | "attached_melon_stem"
            | "attached_pumpkin_stem" | "azalea" | "flowering_azalea" | "big_dripleaf"
            | "big_dripleaf_stem" | "small_dripleaf" | "spore_blossom" | "hanging_roots"
            | "mangrove_roots" | "chorus_plant" | "chorus_flower" | "sea_pickle"
    ) || name.ends_with("_tulip")
}

fn exact(name: &str) -> Option<u8> {
    Some(match name {
        "grass_block" => GRASS,
        "slime_block" => GRASS,
        "sand" | "suspicious_sand" | "glowstone" | "birch_planks" | "scaffolding_top" => SAND,
        "red_sand" | "terracotta" | "pumpkin" | "carved_pumpkin" | "jack_o_lantern" | "honey_block"
        | "raw_copper_block" | "acacia_planks" => ORANGE,
        "gravel" | "suspicious_gravel" | "stone" | "cobblestone" | "mossy_cobblestone"
        | "smooth_stone" | "andesite" | "polished_andesite" | "bedrock" | "furnace"
        | "blast_furnace" | "smoker" | "dispenser" | "dropper" | "observer" | "piston"
        | "sticky_piston" | "cauldron" | "hopper" | "stonecutter" | "spawner" | "trial_spawner"
        | "vault" | "lodestone" | "reinforced_deepslate" => STONE,
        "clay" => CLAY,
        "dirt" | "coarse_dirt" | "rooted_dirt" | "farmland" | "dirt_path" | "packed_mud"
        | "jukebox" | "brown_mushroom_block" => DIRT,
        "mud" => TERRACOTTA_CYAN,
        "podzol" => PODZOL,
        "mycelium" => PURPLE,
        "snow" | "snow_block" | "powder_snow" => SNOW,
        "ice" | "packed_ice" | "blue_ice" | "frosted_ice" => ICE,
        "lava" | "fire" | "redstone_block" | "tnt" => FIRE,
        "soul_fire" => LIGHT_BLUE,
        "netherrack" | "magma_block" | "nether_quartz_ore" | "nether_gold_ore" | "nether_wart" => NETHER,
        "nether_wart_block" | "shroomlight" | "red_mushroom_block" | "bricks" => RED,
        "warped_wart_block" => WARPED_WART_BLOCK,
        "soul_sand" | "soul_soil" => BROWN,
        "obsidian" | "crying_obsidian" | "coal_block" | "netherite_block" | "ancient_debris"
        | "respawn_anchor" | "sculk" | "sculk_vein" | "sculk_catalyst" | "sculk_shrieker"
        | "sculk_sensor" | "calibrated_sculk_sensor" | "end_portal" | "end_gateway"
        | "dragon_egg" => BLACK,
        "iron_block" | "anvil" | "chipped_anvil" | "damaged_anvil" | "lantern" | "soul_lantern"
        | "iron_door" | "iron_trapdoor" | "heavy_weighted_pressure_plate" | "brewing_stand"
        | "grindstone" | "lightning_rod" | "bell" => METAL,
        "gold_block" | "raw_gold_block" | "light_weighted_pressure_plate" => GOLD,
        "diamond_block" | "beacon" => DIAMOND,
        "lapis_block" => LAPIS,
        "emerald_block" => EMERALD,
        "raw_iron_block" => RAW_IRON,
        "amethyst_block" | "budding_amethyst" | "amethyst_cluster" | "large_amethyst_bud"
        | "medium_amethyst_bud" | "small_amethyst_bud" | "shulker_box" => PURPLE,
        "calcite" | "cherry_planks" => TERRACOTTA_WHITE,
        "dripstone_block" | "pointed_dripstone" => TERRACOTTA_BROWN,
        "sea_lantern" | "pale_oak_planks" | "mushroom_stem" => QUARTZ,
        "melon" => LIGHT_GREEN,
        "hay_block" | "sponge" | "wet_sponge" | "bee_nest" | "beehive" | "bamboo_block" => YELLOW,
        "moss_block" | "moss_carpet" | "pale_moss_block" | "pale_moss_carpet" | "dried_kelp_block" => GREEN,
        "glow_lichen" => GLOW_LICHEN,
        "bookshelf" | "chiseled_bookshelf" | "crafting_table" | "chest" | "trapped_chest"
        | "barrel" | "composter" | "note_block" | "loom" | "cartography_table"
        | "fletching_table" | "smithing_table" | "lectern" | "beehive_top" | "dead_bush"
        | "creaking_heart" => WOOD,
        "ender_chest" | "enchanting_table" => BLACK,
        "white_wool" => SNOW,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dyed_blocks() {
        assert_eq!(map_color("minecraft:red_wool"), RED);
        assert_eq!(map_color("minecraft:light_gray_concrete"), LIGHT_GRAY);
        assert_eq!(map_color("minecraft:light_blue_stained_glass"), LIGHT_BLUE);
        assert_eq!(map_color("minecraft:gray_terracotta"), 43);
        assert_eq!(map_color("minecraft:black_terracotta"), 51);
        assert_eq!(map_color("minecraft:terracotta"), ORANGE);
        assert_eq!(map_color("minecraft:white_concrete"), SNOW);
        assert_eq!(map_color("minecraft:magenta_glazed_terracotta"), MAGENTA);
        // look like dyed blocks, but are not
        assert_eq!(map_color("minecraft:red_sand"), ORANGE);
        assert_eq!(map_color("minecraft:red_sandstone"), ORANGE);
        assert_eq!(map_color("minecraft:blue_ice"), ICE);
        assert_eq!(map_color("minecraft:red_nether_bricks"), NETHER);
    }

    #[test]
    fn nature() {
        assert_eq!(map_color("minecraft:grass_block"), GRASS);
        assert_eq!(map_color("minecraft:oak_leaves"), PLANT);
        assert_eq!(map_color("minecraft:cherry_leaves"), PINK);
        assert_eq!(map_color("minecraft:short_grass"), PLANT);
        assert_eq!(map_color("minecraft:red_tulip"), PLANT);
        assert_eq!(map_color("minecraft:water"), WATER);
        assert_eq!(map_color("minecraft:kelp_plant"), WATER);
        assert_eq!(map_color("minecraft:sand"), SAND);
    }

    #[test]
    fn stone_and_wood() {
        assert_eq!(map_color("minecraft:spruce_planks"), PODZOL);
        assert_eq!(map_color("minecraft:dark_oak_log"), BROWN);
        assert_eq!(map_color("minecraft:stripped_birch_log"), SAND);
        assert_eq!(map_color("minecraft:oak_stairs"), WOOD);
        assert_eq!(map_color("minecraft:deepslate_diamond_ore"), DEEPSLATE);
        assert_eq!(map_color("minecraft:iron_ore"), STONE);
        assert_eq!(map_color("minecraft:stone_bricks"), STONE);
        assert_eq!(map_color("minecraft:red_sandstone_slab"), ORANGE);
        assert_eq!(map_color("minecraft:polished_blackstone_bricks"), BLACK);
        assert_eq!(map_color("minecraft:warped_hyphae"), WARPED_HYPHAE);
    }

    #[test]
    fn transparent_and_unknown() {
        assert_eq!(map_color("minecraft:air"), NONE);
        assert_eq!(map_color("minecraft:glass"), NONE);
        assert_eq!(map_color("minecraft:torch"), NONE);
        assert_eq!(map_color("minecraft:oak_button"), NONE);
        assert_eq!(map_color("somemod:fancy_block"), STONE);
        assert_eq!(map_color("minecraft:totally_new_block_2027"), STONE);
    }

    #[test]
    fn shading() {
        assert_eq!(shaded(GRASS, HIGH), [127, 178, 56]);
        assert_eq!(shaded(SNOW, LOW), [180, 180, 180]);
    }

    #[test]
    fn relief_keeps_flat_colours() {
        assert_eq!(relief([100, 150, 200], NORMAL), [100, 150, 200]);
        assert_eq!(relief([100, 150, 250], HIGH), [115, 173, 255]);
    }

    #[test]
    fn viewer_palette_matches() {
        // The 3D viewer turns shaded map colours back into base colours with its own copy of BASE
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/static/map/map3d-worker.js");
        let Ok(js) = std::fs::read_to_string(path) else { return }; // e.g. Docker build stage without app/
        let line = js.lines().find(|l| l.starts_with("const PALETTE = [")).expect("PALETTE in map3d-worker.js");
        let inner = &line[line.find('[').unwrap() + 1..line.rfind(']').unwrap()];
        let values: Vec<u8> = inner.split(',').map(|v| v.trim().parse().unwrap()).collect();
        let expected: Vec<u8> = BASE.iter().flatten().copied().collect();
        assert_eq!(values, expected, "update PALETTE in app/static/map/map3d-worker.js");
    }
}
