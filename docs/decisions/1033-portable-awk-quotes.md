# 1033: the exit-script lint uses no gawk-only escapes

`scripts/check-exit-portability.sh` wrote the single quote as `\x27` inside awk regex literals. That is a gawk extension. ubuntu-latest's default awk is mawk, which does not read it as a quote, so the `sed -i ''` rule and the quote-prefixed rules silently stopped matching and s669's "sed -i empty suffix" fixture was not refused.

Decision: the quote is passed in with `-v q="'"` and the affected regexes are built as dynamic strings. The lint must run the same under mawk, BSD awk and gawk. s1040 pins this.
