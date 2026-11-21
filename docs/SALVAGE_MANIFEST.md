# Salvage manifest

Files rescued from the volatile `/tmp` into this repository, so they survive reboots and the
daily `systemd-tmpfiles` cleaner (10-day age limit on this machine). Each entry says where the
files came from, where they went, and why.

## 2026-11-21: original agy artwork

**From:** an earlier Claude Code session's scratch directory,
`/tmp/claude-1000/-home-parobek-Code-OSS-Public-Projects-StarGazer/cd3748d3-1f7c-414a-a275-ad92aecbdb76/scratchpad/agy-art/`
(the v0.2.0 README artwork session, 2026-11-15; about 4 days from automatic deletion).

**To:** `assets/source/agy/` (described in `assets/README.md`).

| File | Bytes before | Bytes after | Check |
|------|--------------|-------------|-------|
| `banner-art.png` | 679,831 | 678,172 | pixel-identical (ImageMagick AE = 0) |
| `banner.png` | 806,308 | 689,793 | pixel-identical |
| `banner-titlezone.png` | 170,551 | 146,960 | pixel-identical |
| `logo.png` | 584,555 | 567,971 | pixel-identical |
| `review.png` | 97,934 | 93,437 | pixel-identical |
| `agy-output.txt` | 363 | 363 | byte-identical |

PNGs were optimized losslessly with `optipng -o2`; the originals were removed from `/tmp` after
the check (a move, not a copy).

**Why:** these are the only copies of the raw `agy` generations. The repository held only edited
derivatives (the titled and faded `assets/banner.png`, the quantized `assets/logo.png`), and
generative output cannot be reproduced.

**Found but deliberately not taken** (left to expire; all preserved elsewhere or regenerable):
v0.1 source copies (byte-identical to commit a874e20), release downloads (on the GitHub
releases), gate and task logs, pre-edit source snapshots (superseded by commits), screenshot and
social-card intermediates, awesome-list research READMEs (re-fetchable), three `agy-review-*.log`
diagnostics (the cause is recorded in ai-memory), and copies of the maintainer's star cache
(regenerable by sync, and personal data that does not belong in a public tree). Nothing from the
26 other projects' scratch trees was taken.
