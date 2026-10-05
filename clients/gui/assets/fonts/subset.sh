#!/bin/sh
# Rebuilds DejaVuSans.ttf, the face the GUI host draws its text with, from an
# upstream DejaVu Sans (2.37): the writing systems an interface is written in
# and the symbols it uses, with no hinting and no layout tables, since the
# host's rasterizer reads neither. The name table is kept whole, so the
# copyright and the license travel inside the file.
#
#   pip install fonttools
#   clients/gui/assets/fonts/subset.sh /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf
#
# A subset is a modified copy, which the license allows under a name that
# holds neither "Bitstream" nor "Vera" (LICENSE, beside this file).
set -eu
src="$1"
here="$(cd "$(dirname "$0")" && pwd)"
# Latin (basic, Latin-1, extended A and B, additional), spacing and combining
# marks, Greek, Cyrillic, punctuation, super- and subscripts, currency,
# letterlike, number forms, arrows, mathematical operators, technical,
# geometric shapes, miscellaneous symbols, dingbats, ligatures, and the
# replacement character.
ranges="U+0020-007E,U+00A0-024F,U+02B0-02FF,U+0300-036F,U+0370-03FF"
ranges="$ranges,U+0400-04FF,U+1E00-1EFF,U+2000-206F,U+2070-209F,U+20A0-20BF"
ranges="$ranges,U+2100-214F,U+2150-218F,U+2190-21FF,U+2200-22FF,U+2300-23FF"
ranges="$ranges,U+25A0-25FF,U+2600-26FF,U+2700-27BF,U+FB00-FB06,U+FFFD"
pyftsubset "$src" --unicodes="$ranges" --output-file="$here/DejaVuSans.ttf" \
    --no-hinting --desubroutinize --layout-features='' \
    --name-IDs='*' --name-languages='*' --notdef-outline
