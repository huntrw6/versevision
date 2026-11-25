VerseVision turns sermon notes (PDF, Word, text, Markdown, HTML) into ProPresenter 19 slides, in document order: one slide per point and per verse piece, with review tiers, themes, history and undo.

**1.0.0** is the Rust rewrite of the 0.38 Python app. It is one program with no Python install, it runs on macOS (Intel and Apple Silicon), Windows and Linux, and it is the same program that StagePilot bundles.

- `versevision serve` opens the page on your computer. Import, Replace and Append work on ProPresenter without restarting it, with undo.
- Verse text comes from API.Bible, YouVersion or ESV. On a fresh install the page asks for a Bible key before it accepts a file, with a button that opens Config > Bible keys.
- Inside StagePilot, verse text is served by StagePilot's service and needs no key.

**Download** the file for your computer, unpack it and read START-HERE.txt. These builds are not signed: macOS and Windows will ask you to confirm the first start (START-HERE.txt shows how). Check a download with `SHA256SUMS`.

The old Python app is kept at the git tag `python-final`.
