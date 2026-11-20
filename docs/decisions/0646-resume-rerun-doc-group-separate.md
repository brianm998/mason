# 0646: resume rerun expects the doc group separate

Decision 0609 made doc tests run in their own `-p NAME --doc` invocation. The resume tests
still expected the older combined form. Production was right; the test expectations were
updated and no production code changed.
