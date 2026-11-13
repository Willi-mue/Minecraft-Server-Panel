"""Draws the pack.png of each custom data pack (16x16 pixel art, scaled to 128x128).

Run from the repo root: python -I datapacks/tools/gen_icons.py
The background colour stands for the pack's category (see CATEGORY_COLORS).
"""
from pathlib import Path

from PIL import Image

CATEGORY_COLORS = {  # background, border
    "Komfort": ((92, 158, 75), (52, 98, 42)),
    "Bauen & Deko": ((74, 123, 183), (40, 72, 115)),
    "Spaß": ((142, 91, 181), (86, 50, 115)),
}

PALETTE = {
    "y": (240, 206, 92), "Y": (196, 150, 40), "g": (88, 140, 52),
    "b": (122, 82, 50), "B": (84, 54, 32),
    "o": (70, 44, 24), "w": (176, 134, 82), "r": (196, 52, 52), "R": (140, 30, 30),
    "f": (150, 100, 56), "c": (190, 228, 240), "W": (255, 255, 255),
    "s": (236, 232, 222), "k": (40, 30, 40),
    "G": (150, 152, 150), "D": (88, 90, 88),
}

ICONS = {
    "harvest": ("Komfort", [
        "................",
        "..........y.....",
        ".........yYy....",
        "..y......yYy....",
        ".yYy.....yYy..y.",
        ".yYy......g..yYy",
        ".yYy..y...g..yYy",
        "..g..yYy..g..yYy",
        "..g..yYy..g...g.",
        "..g..yYy..g...g.",
        "..g...g...g...g.",
        "..g...g...g...g.",
        "..g...g...g...g.",
        "bbbbbbbbbbbbbbbb",
        "BbBbBbBbBbBbBbBb",
        "BBBBBBBBBBBBBBBB",
    ]),
    "sit": ("Bauen & Deko", [
        "................",
        ".ooooooo........",
        ".owwwwwo........",
        ".owwwwwo........",
        ".owwwwwo........",
        ".owwwwwo........",
        ".owwwwwo........",
        ".owwwwworrrrrr..",
        ".owwwwwoRRRRRR..",
        ".oooooooooooooo.",
        ".owwwwwwwwwwwwo.",
        ".owwwwwwwwwwwwo.",
        ".owwwwwwwwwwwwo.",
        ".owwwwwwwwwwwwo.",
        ".owwwwwwwwwwwwo.",
        ".oooooooooooooo.",
    ]),
    "frames": ("Bauen & Deko", [
        "................",
        ".oooooooooooooo.",
        ".offffffffffffo.",
        ".ofccccccccccfo.",
        ".ofcWccccccccfo.",
        ".ofcWWcccccccfo.",
        ".ofccWWccccccfo.",
        ".ofcccWWcccccfo.",
        ".ofccccWWccccfo.",
        ".ofcccccWWcccfo.",
        ".ofccccccWWccfo.",
        ".ofcccccccWccfo.",
        ".ofccccccccccfo.",
        ".offffffffffffo.",
        ".oooooooooooooo.",
        "................",
    ]),
    "graves": ("Komfort", [
        "................",
        ".....DDDDDD.....",
        "....DGGGGGGD....",
        "...DGGGkkGGGD...",
        "...DGGGkkGGGD...",
        "...DGkkkkkkGD...",
        "...DGkkkkkkGD...",
        "...DGGGkkGGGD...",
        "...DGGGkkGGGD...",
        "...DGGGkkGGGD...",
        "...DGGGGGGGGD...",
        "...DGGGGGGGGD...",
        "..gDDDDDDDDDDg..",
        "gggggggggggggggg",
        "bbbbbbbbbbbbbbbb",
        "BBBBBBBBBBBBBBBB",
    ]),
    "nightly": ("Spaß", [
        "................",
        "......yyyy......",
        "....yyyyy....W..",
        "...yyyy.........",
        "..yyyy.......W..",
        "..yyy...........",
        ".yyyy...........",
        ".yyy.......W....",
        ".yyy............",
        ".yyyy...........",
        "..yyy..........W",
        "..yyyy.......yy.",
        "...yyyy....yyy..",
        "....yyyyyyyyy...",
        "......yyyyy.....",
        "................",
    ]),
    "deaths": ("Spaß", [
        "................",
        "....ssssssss....",
        "...ssssssssss...",
        "..ssssssssssss..",
        "..ssssssssssss..",
        "..sskksssskkss..",
        "..skkkksskkkks..",
        "..skkkksskkkks..",
        "..sskksssskkss..",
        "..ssssskksssss..",
        "...sssskkssss...",
        "....ssssssss....",
        "....skskksks....",
        "....ssssssss....",
        ".....s.ss.s.....",
        "................",
    ]),
}

SCALE = 8


def draw(category, rows):
    bg, border = CATEGORY_COLORS[category]
    img = Image.new("RGBA", (16, 16), (0, 0, 0, 0))
    px = img.load()
    for y in range(16):
        for x in range(16):
            corner = (x in (0, 15)) and (y in (0, 15))
            if corner:
                continue
            edge = x in (0, 15) or y in (0, 15)
            px[x, y] = (*(border if edge else bg), 255)
    for y, row in enumerate(rows):
        assert len(row) == 16, (category, y, row)
        for x, ch in enumerate(row):
            if ch != ".":
                px[x, y] = (*PALETTE[ch], 255)
    return img.resize((16 * SCALE, 16 * SCALE), Image.NEAREST)


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    for pack, (category, rows) in ICONS.items():
        draw(category, rows).save(root / pack / "pack.png")
        print("wrote", pack, category)
