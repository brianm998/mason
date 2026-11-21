# 0786: exit-test slot holders get a fifo on stdin

Status: accepted (pending owner approval: `scripts/exit/**` is protected)

`mason hold` ends when stdin closes. Background jobs in non-interactive bash get /dev/null, so s748 and s750 held nothing. Both now use a fifo kept open on fd 3, as s155 does, and close it to release. Assertions are unchanged; the alternative of `--pid` was rejected to stay consistent with s155.
