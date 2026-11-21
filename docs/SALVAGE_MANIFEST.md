# Salvage manifest

Files rescued from a temporary directory (`/tmp`) into this repository before the system's
temporary-file cleanup deleted them. Each entry says what was rescued, where it went, how it was
checked, and why it was kept.

## 2026-11-21: original agy artwork

**From:** the scratch directory of the 2026-11-15 working session that produced the README
artwork (v0.2.0), a few days before automatic deletion.

**To:** `assets/source/agy/` (described in `assets/README.md`).

| File | Bytes before | Bytes after | Check |
|------|--------------|-------------|-------|
| `banner-art.png` | 679,831 | 678,172 | pixel-identical (ImageMagick AE = 0) |
| `banner.png` | 806,308 | 689,793 | pixel-identical |
| `banner-titlezone.png` | 170,551 | 146,960 | pixel-identical |
| `logo.png` | 584,555 | 567,971 | pixel-identical |
| `review.png` | 97,934 | 93,437 | pixel-identical |

PNGs were optimized losslessly with `optipng -o2`; the temporary originals were removed after
the check (a move, not a copy).

**Why:** these are the only copies of the raw `agy` generations. The repository held only edited
derivatives (the titled and faded `assets/banner.png`, the quantized `assets/logo.png`), and
generative output cannot be reproduced.

**Deliberately not taken** (all preserved elsewhere or regenerable): copies of the v0.1 source
(identical to commit a874e20), release archives (on the GitHub releases), gate and review logs,
pre-edit source snapshots (superseded by commits), screenshot and social-card intermediates,
fetched awesome-list READMEs, and copies of a personal star cache (regenerable by a sync, and
personal data that does not belong in a public repository).
