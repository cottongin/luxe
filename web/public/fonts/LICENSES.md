# Fonts

All three families are subset copies, built by `scripts/site-fonts.sh`. A subset is a **Modified
Version** under the SIL Open Font License, so two things matter and both were checked:

- None of them declares a **Reserved Font Name** after its copyright statement, so the subsets may
  keep their names (OFL 1.1 §3).
- The copyright notice and license text must travel with the files (OFL 1.1 §2), hence this file
  and the three `OFL-*.txt` alongside it.

## Instrument Serif — display

Copyright 2022 The Instrument Serif Project Authors
<https://github.com/Instrument/instrument-serif> · SIL Open Font License 1.1 ·
full text in `OFL-InstrumentSerif.txt`.

Subset to latin plus editorial punctuation.

## JetBrains Mono Nerd Font Mono — terminal and UI

Copyright 2020 The JetBrains Mono Project Authors <https://github.com/JetBrains/JetBrainsMono> ·
SIL Open Font License 1.1 · full text in `OFL-JetBrainsMono.txt`.

Patched with [Nerd Fonts](https://github.com/ryanoasis/nerd-fonts) 3.5.1, whose patcher is MIT
licensed and whose added glyph sets carry their own licenses. The private-use codepoints this
subset keeps come from Material Design Icons (Apache-2.0), Font Awesome (OFL-1.1), Powerline
Extra (MIT) and [Codicons](https://github.com/microsoft/vscode-codicons) (© Microsoft Corporation,
CC BY 4.0).

Subset to exactly the characters the generated assets and captures draw, plus printable ASCII:
a couple of hundred codepoints out of several thousand. Regular (400), Bold (700) and Italic.

**Why this font.** It's the monospace face most people already have in their terminal, so a
capture on this site looks like Luxe will look for them.

## Adwaita Mono Nerd Font Mono — the six marks JetBrains Mono lacks

Copyright (c) 2016, The Inter Project Authors · Copyright (c) 2015-2025, Renzhi Li (aka. Belleve
Invis) · Copyright (c) 2025, Jamie Gravendeel · Copyright (c) 2025, Florian Müllner ·
<https://gitlab.gnome.org/GNOME/adwaita-fonts> · SIL Open Font License 1.1 · full text in
`OFL-AdwaitaMono.txt`. Patched with Nerd Fonts, as above.

Subset to only the codepoints the JetBrains Mono subset does not have (`ℹ ↳ ↺ ⏸ ✔ ✖` at the time of
writing), computed at build time. It draws on the same 0.6em cell, so a mark from it lines up with
the columns around it, and the CSS names it second so the browser reaches for it one glyph at a
time.
