# Fonts

## Inter

`Inter.ttf` is the regular weight of Inter, used for every glyph PushOS draws
on the Push 2 display.

- Source: <https://github.com/google/fonts/tree/main/ofl/inter>
- Licence: SIL Open Font Licence 1.1, reproduced in `Inter-OFL.txt`. Inter
  declares no Reserved Font Name, so a modified copy may keep the name.

PushOS rasterises one weight. Typographic hierarchy on the display comes from
size, colour and layout rather than from a second weight.

### How this file was made

The upstream file is a variable font, 856 KB. PushOS's rasteriser draws its
default instance by character and reads no variation, layout or glyph-name
tables, so those were 670 KB of the program that never did anything. This copy
is that same default instance with them removed, 186 KB:

```python
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

font = TTFont("Inter[opsz,wght].ttf")
static = instancer.instantiateVariableFont(
    font, {"wght": 400, "opsz": 14}, updateFontNames=False
)
for tag in ("GPOS", "GSUB", "GDEF"):
    del static[tag]
static["post"].formatType = 3.0
static.save("Inter.ttf")
```

Every glyph is kept, so every script Inter covers still draws. Checked when it
was made: all 2,849 characters map to the same glyphs, none of the 2,933 glyph
outlines or advances changed, and every screen in `pushos-ui`'s preview example
renders byte for byte the same as with the variable font.
