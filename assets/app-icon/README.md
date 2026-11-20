# GitCraft app icon

**Branch:** a disc in GitCraft's brick (`app-gitcraft` `#c4501d`) with a line of history, a branch
leaving it and three commits cut out as one real hole. Transparent ground, no tile. Part of the
ONEFOLD LABS app family (brand book, "The app family").

The SVGs come from the brand repository (`ONEFOLD-LABS/brand`, `apps/gitcraft/`), computed by
`tools/make_app_icons.py`; nothing is drawn by hand or traced. The motif is a common way to show a
branch, drawn from scratch.

| File | Use |
|---|---|
| `icon.svg` | The icon from 48 px (30 px strokes on the 512 grid) |
| `icon-small.svg` | Simplified for 16–32 px (44 px strokes) |
| `gitcraft-1024.png` | 1024 px render |
| `hicolor/<n>x<n>/apps/gitcraft.png` | Linux/FreeBSD icon theme, 16–512 px (16–32 from `icon-small.svg`), plus `scalable/` |

Windows `.ico` and macOS `.icns` follow with the packages (roadmap 0.9).

Rebuild after the brand changes the icon: copy both SVGs from the brand repository, then

```sh
for s in 16 24 32; do rsvg-convert -w $s -h $s icon-small.svg -o hicolor/${s}x${s}/apps/gitcraft.png; done
for s in 48 64 128 256 512; do rsvg-convert -w $s -h $s icon.svg -o hicolor/${s}x${s}/apps/gitcraft.png; done
rsvg-convert -w 1024 -h 1024 icon.svg -o gitcraft-1024.png
```

Licence: MIT OR Apache-2.0 like the code.
