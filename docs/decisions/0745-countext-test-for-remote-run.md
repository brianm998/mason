# 0745: test `countText` to exercise a remote `compose-clients` run

Decision: add a pure-function unit test (`CountTextTest`) rather than change product code. It is a real gap (`countText` was untested) and needs no Compose UI runtime, so the run stays fast on `ubuntu-latest`. No gates or trust files change.
