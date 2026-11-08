#!/bin/sh
# Rebuilds the two faces the GUI host draws its text with, with no hinting
# and the layout tables kept, since the host shapes text before it
# rasterizes it:
#
# - DejaVuSans.ttf, from an upstream DejaVu Sans (2.37): the writing systems
#   an interface is written in -- Latin, Greek, Cyrillic, Arabic and Hebrew --
#   and the symbols it uses;
# - NotoSansDevanagari.ttf, from Noto Sans Devanagari (2.001): Devanagari,
#   which DejaVu Sans does not draw, as the face the host falls back to.
#
# The name tables are kept whole, so each copyright and license travels
# inside its file.
#
#   pip install fonttools
#   clients/gui/assets/fonts/subset.sh \
#       /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf \
#       /usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf
#
# A subset is a modified copy, which the DejaVu license allows under a name
# that holds neither "Bitstream" nor "Vera" (LICENSE), and the SIL Open Font
# License allows for a face with no reserved font name (OFL.txt).
set -eu
dejavu="$1"
devanagari="$2"
here="$(cd "$(dirname "$0")" && pwd)"
keep="--no-hinting --desubroutinize --layout-features=* --name-IDs=* --name-languages=* --notdef-outline"
# Latin (basic, Latin-1, extended A and B, additional), spacing and combining
# marks, Greek, Cyrillic, Hebrew and its presentation forms, Arabic, its
# supplement and its presentation forms, punctuation, super- and subscripts,
# currency, letterlike, number forms, arrows, mathematical operators,
# technical, geometric shapes, miscellaneous symbols, dingbats, ligatures,
# and the replacement character.
ranges="U+0020-007E,U+00A0-024F,U+02B0-02FF,U+0300-036F,U+0370-03FF"
ranges="$ranges,U+0400-04FF,U+0590-05FF,U+0600-06FF,U+0750-077F"
ranges="$ranges,U+1E00-1EFF,U+2000-206F,U+2070-209F,U+20A0-20BF"
ranges="$ranges,U+2100-214F,U+2150-218F,U+2190-21FF,U+2200-22FF,U+2300-23FF"
ranges="$ranges,U+25A0-25FF,U+2600-26FF,U+2700-27BF,U+FB00-FB06,U+FB1D-FB4F"
ranges="$ranges,U+FB50-FDFF,U+FE70-FEFF,U+FFFD"
# shellcheck disable=SC2086
pyftsubset "$dejavu" --unicodes="$ranges" --output-file="$here/DejaVuSans.ttf" $keep
# Devanagari, its extensions and Vedic marks, the joiners and the dotted
# circle a mark with no base is drawn on.
ranges="U+0020,U+0900-097F,U+1CD0-1CFF,U+200C-200D,U+25CC,U+A8E0-A8FF"
# shellcheck disable=SC2086
pyftsubset "$devanagari" --unicodes="$ranges" --output-file="$here/NotoSansDevanagari.ttf" $keep
