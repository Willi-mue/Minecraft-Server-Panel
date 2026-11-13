# Custom data packs

Sources of the server's own data packs (written for Minecraft 26.3, `min_format`/`max_format` 121).
The panel's library is `DATA_DIR/datapacks/` (`./data/datapacks/`, not versioned), so copy a pack
there after changing it; the panel then shows it as "update" in worlds that have an older copy.

```powershell
Copy-Item -Recurse -Force datapacks\harvest data\datapacks\   # PowerShell
```
```bash
rm -rf data/datapacks/harvest && cp -r datapacks/harvest data/datapacks/   # bash
```

The folder name is the pack's id in the world (`file/<folder>`): renaming it makes Minecraft treat it
as a new pack, so the display name lives in `pack.mcmeta` instead. Besides the usual `pack` section,
each `pack.mcmeta` has a `panel` section (`title`, `category`) that only the panel reads; it shows the
packs grouped by category with their `pack.png`.

| Folder | Name | Category | What it does |
|---|---|---|---|
| `harvest` | Erntehelfer | Komfort | Right-click a ripe crop (wheat, carrots, potatoes, beetroots, nether wart) with a hoe, within block reach: drops what breaking it with that hoe would (Fortune counts) and replants it with a seed from the inventory (off hand, then hotbar left to right, then inventory top left to bottom right; nothing taken in creative). Without a seed it is only harvested. Hold the button and sweep over the field; unripe crops do not block the view. Not in adventure mode. Before switching it off: `/function harvest:uninstall`. |
| `sit` | Sitzplätze | Bauen & Deko | Right-click a stair (bottom half) or bottom slab with an empty hand: sit down, facing away from the stair's backrest. Sneak to stand up. Needs room above. Hitting the seat with a fist steps aside for 2 s so stairs can still be broken by hand. Before switching it off: `/function sit:uninstall`. |
| `frames` | Unsichtbare Rahmen | Bauen & Deko | Sneak + right-click an item frame (or glow item frame) with a glass pane: it turns invisible (uses up one pane, not in creative; the frame keeps it: shown again → back to the player, emptied or broken → it drops); again → visible. The click's own rotation of the item is undone. An empty frame cannot be hidden (the pane comes back), and empty invisible frames become visible by themselves. The item sits 1/16 deeper in an invisible frame: that is how the Minecraft client draws it. `/function frames:uninstall` makes the loaded frames visible. |
| `deaths` | Todesbote | Spaß | On death, everybody gets a random funny line matching the cause (last damage within 10 s: fall, fire, lava, drowning, explosion, projectile, freezing, void, starving, cactus, zombies, players, warden, suffocation, elytra, magic, else a general one); the player who died gets the place (click = copy coordinates, `[TP]` = teleport command for operators). The vanilla death message stays. |
| `nightly` | Nachtbilanz | Spaß | At dawn (overworld clock leaves 13000–23000, also when the night is slept away) the chat shows who killed the most mobs, died most often and walked the farthest during the night (ties: all names; no message when nothing happened). Counters reset at dusk. `/trigger nachtbilanz` (or `/trigger lastnight`) shows the last report again plus the player's own values; the names are stored as text (`set_name` item modifier resolves the selector), so it works when those players are offline. Night is a `time_check` predicate on the 26.x `minecraft:overworld` clock. |
| `graves` | Grabsteine | Komfort | On death, the items that just dropped (age ≤ 3 ticks, within 8 blocks) go into a grave at the death point: a `marker` holding owner UUID + items, a stone (`block_display`), the player's head with skin (`fill_player_head`), a name sign (`text_display`) and an `interaction` entity. Right-click by the owner hands all items back (dropped at their feet, picked up at once; free armor slots get the matching piece again via `#minecraft:*_armor` tags, elytra if no chestplate — the drops do not remember their slots); others get "not your grave". Graves do not despawn. No items (keepInventory, lava, void) = no grave. Before switching it off: `/function graves:uninstall` (loaded graves drop their items). |

## Languages

Chat messages are English by default; each player can switch with `/trigger deutsch` and back with
`/trigger english`. The choice is the shared score `lang.de` (0 English, 1 German): every pack with
chat messages creates the same objectives and has the same small `lang` function, so it works with any
combination of packs switched on. Messages are sent twice, to `@a[scores={lang.de=0}]` and
`@a[scores={lang.de=1}]`. Use only characters the Minecraft font has (no emoji outside the basic
plane like 👣 or 🪦; ☠ ⚔ ☀ ☾ ⚰ ➜ are fine).

## Tools

- `tools/gen_deaths.py` writes `deaths/data/` (messages, damage-type tags, one hurt advancement per
  cause). Add or change lines in `CAUSES` / `OTHER` there, then run
  `python -I datapacks/tools/gen_deaths.py datapacks/deaths`.
- `tools/gen_icons.py` draws every `pack.png` (16×16 pixel art, background colour = category). Run
  `python -I datapacks/tools/gen_icons.py` after changing a design.

## How the clicks are detected

`harvest`: vanilla fires no advancement trigger for a hoe on a crop (the interaction does nothing;
checked in the 26.3 bytecode: `item_used_on_block`, `default_block_use` and `any_block_use` all need a
consuming result). While a player looks at a crop with a hoe in the main hand, the hoe gets a
`consumable` component that never finishes (silent, no slowdown), so the right-click counts as using
the item (`using_item` trigger). Looking elsewhere removes it again, so shields, off-hand torches and
food keep working. Spawn protection is not checked.

`sit`: an empty hand on a stair fires no trigger either. While a player looks at a free seat with an
empty main hand, an invisible `interaction` entity covers its lower half; clicking it fires
`player_interacted_with_entity`. The seat is an `item_display` the player rides; it is removed when
nobody sits on it or the block is gone.

`frames`: the click rotates the item (or puts the pane into an empty frame) before the pack hears of
it, so the tick remembers the state of the frame a sneaking player with a pane aims at and the
`player_interacted_with_entity` reward restores it. A `marker` on the frame stands for the pane it holds.

## Testing

Test on a throwaway vanilla server, never on the live world: the panel image has Java 25, e.g.
`docker run --rm -i --network none -v <dir>:/srv -w /srv --entrypoint java minecraft-server-panel -jar server.jar nogui`
with `pause-when-empty-seconds=-1` and the commands piped in. A broken predicate or other registry file
stops the whole server from starting, so boot-test every change.
