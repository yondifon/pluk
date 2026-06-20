# Pluk Logos

Source of truth for Pluk's brand marks. The shipped app assets
(`swift/AppIcon.icns`, `swift/Sources/Resources/MenuBarIcon.png`) are generated
from the SVGs here — edit the SVG, then regenerate.

## The mark

A toggle switch, knob pushed **on** (right), gating a left‑to‑right channel —
"plug a service in, switch it on."

## Files

| File | Role |
| --- | --- |
| `logo.svg` | **Canonical app icon.** Ink tile (rounded square) + toggle + accent knob. Master for the icns and all raster sizes. |
| `symbol.svg` | Transparent logomark — the toggle glyph with no tile. For docs, web, and embeds on their own background. Not used by the app. |
| `menubar.svg` | **Menu bar template.** Monochrome silhouette, knob as a cut‑out, sized to the bar. Rendered as a macOS template image (the system recolors it). |
| `export/AppIcon.iconset/` | The 10 PNG sizes `iconutil` packs into `AppIcon.icns`. |
| `export/logo-1024.png` | High‑res master raster for the web / READMEs. |

## Brand colors

| Token | Hex | Use |
| --- | --- | --- |
| Ink | `#161616` | Tile background, glyph in the symbol mark. |
| Paper | `#FAFAF7` | Channel + pill on the app icon. |
| Accent | `#E0A23B` | The "on" knob. The one pop of color. |

The menu bar icon is intentionally colorless — it ships as a template so macOS
tints it for light/dark menu bars.

## Regenerating the app assets

After editing `logo.svg` or `menubar.svg` (needs `rsvg-convert` and `iconutil`):

```bash
# App icon — render every iconset size from logo.svg, then pack the .icns
cd logos
for s in 16 32 128 256 512; do
  rsvg-convert -w $s   -h $s   logo.svg -o export/AppIcon.iconset/icon_${s}x${s}.png
  rsvg-convert -w $((s*2)) -h $((s*2)) logo.svg -o export/AppIcon.iconset/icon_${s}x${s}@2x.png
done
rsvg-convert -w 1024 -h 1024 logo.svg -o export/logo-1024.png
iconutil -c icns export/AppIcon.iconset -o ../swift/AppIcon.icns

# Menu bar icon — 60x36 template PNG (2x of the 30x18 bar slot)
rsvg-convert -w 60 -h 36 menubar.svg -o ../swift/Sources/Resources/MenuBarIcon.png
```

`make bundle` then copies both into `Pluk.app`.
