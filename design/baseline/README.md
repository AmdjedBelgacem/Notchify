# Visual baseline

Reference screenshots for the visual-regression check
(`packaging/visual_diff.py`). CI compares every `*.png` here against the PNG of
the same name produced by the UI self-test. While this folder has no PNGs the
check is skipped.

## Refreshing the baseline

Use screenshots from a self-test run on the same kind of machine CI uses (the
`macos-15` runner), since fonts and display scale change the pixels:

1. Open a green CI run, download the `ui-self-test` artifact and unzip it.
   (Or run locally: `packaging/bundle.sh && dist/Notchisland.app/Contents/MacOS/notchforge --self-test "$PWD/selftest"`.)
2. Copy the PNGs you want to guard into this folder, keeping their names:
   `cp selftest/*.png design/baseline/`
   Leave out screenshots with live data (clock, weather, now playing) — they
   change every run.
3. Check locally: `packaging/visual_diff.py selftest` (options: `--threshold`
   percent of differing pixels allowed per image, default 1.0; `--tolerance`
   per-channel difference ignored, default 8; `--diff-dir` writes red masks).
4. Commit the new PNGs together with the UI change that caused them.
