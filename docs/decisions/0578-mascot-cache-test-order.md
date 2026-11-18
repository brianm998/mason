# 0578: count-based mascot tests reset the cache first

The mascot image cache is static and shared by every test in one process, so a test that counts builds depends on test order. Such tests call `Mascot.emptyCacheForTests()` (internal, tests only) at their start instead of loosening the count.
