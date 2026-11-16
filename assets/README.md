# Assets

Images used by the project README, the GitHub repository page, and as project icons.

| File | What | Origin |
|------|------|--------|
| `banner.png` | README header, 1800x600 | Art generated with the Antigravity CLI (`agy`, image-generator subagent, Gemini 4 Carbon); title and tagline overlaid with ImageMagick; the bottom-left corner of the first terminal outline, including its glow, is faded to the sky color (RGB 7,16,34) so nothing crowds the title's "r": the left edge is fully sky from the corner up to just above the "r" and eases back in by y 185, the bottom edge is fully sky for 30 px and eases back in by three quarters of its length |
| `logo.png` | Raster emblem, 1024x1024 | Generated with `agy` (same session) |
| `logo.svg` | Vector emblem | Hand-drawn redraw of `logo.png`, the master for all icon sizes 64 px and up |
| `logo-small.svg` | Vector emblem for small sizes | Same design with a heavier frame and no cursor, for 16-48 px |
| `icons/icon-{16,32,48}.png` | Small icons | Rendered from `logo-small.svg` |
| `icons/icon-{64,128,180,256,512,1024}.png` | Icons (180 = Apple touch icon size) | Rendered from `logo.svg` |
| `icons/favicon.ico` | Multi-resolution ICO (16/32/48) | Built from the small icons |
| `icons/stargazer.ico` | Windows application icon (16/24/32/48/64/128/256, PNG entries, 12.5 KB), embedded in `stargazer.exe` by `build.rs` | `pack_ico.py` over renders of `logo-small.svg` (16-48) and `logo.svg` (64-256) |
| `pack_ico.py` | Packs PNGs into a PNG-entry `.ico` | Hand-written; ImageMagick's own ICO output is ~30x larger |
| `social-preview.png` | GitHub social preview, 1280x640, centered layout | The text-free right side of `banner.png` mirrored into a symmetric background; a solid navy center panel (opaque within 300 px of center, fading out by 520 px) keeps the text area free of artwork; `logo.svg` without its background; ImageMagick text |
| `screenshot-{main,details,help}.png` | TUI screenshots (v0.2.0) | Konsole, captured with Spectacle and cropped with ImageMagick |
| `screenshot-awesome{,-picker}.png` | Awesome list browser (v0.3.0) and the collapsible picker (v0.3.1) | StarGazer in tmux, Konsole attached to the session so keys can be scripted with `tmux send-keys`; captured with `spectacle -a -b -n` and cropped to the terminal area |

Palette: navy `#0b1020`, cyan `#22d3ee`, gold `#facc15`. Fonts: Noto Sans Bold (title),
Noto Sans Mono Bold (tagline).

## Regenerating icons

```bash
cd assets
for s in 16 32 48; do rsvg-convert -w $s -h $s logo-small.svg -o icons/icon-$s.png; done
for s in 64 128 180 256 512 1024; do rsvg-convert -w $s -h $s logo.svg -o icons/icon-$s.png; done
magick icons/icon-16.png icons/icon-32.png icons/icon-48.png icons/favicon.ico
optipng -quiet -o2 icons/*.png

# Windows application icon (embedded by build.rs)
mkdir -p /tmp/ico
for s in 16 24 32 48; do rsvg-convert -w $s -h $s logo-small.svg -o /tmp/ico/$s.png; done
for s in 64 128 256; do rsvg-convert -w $s -h $s logo.svg -o /tmp/ico/$s.png; done
optipng -quiet -o2 /tmp/ico/*.png
./pack_ico.py icons/stargazer.ico /tmp/ico/{16,24,32,48,64,128,256}.png
```

Screenshots are stored losslessly (`optipng`) so terminal text stays sharp; the artwork is
palette-quantized with `pngquant --quality 85-98`.

## Using the social preview

GitHub has no API for the repository social preview. Upload `social-preview.png` once under
**Settings > General > Social preview**.
