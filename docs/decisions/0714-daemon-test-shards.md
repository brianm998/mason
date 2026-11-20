# 0714: shard the daemon CI leg instead of raising its timeout or world slots

Context: `CI/test (ubuntu-latest, daemon)` exceeded 45 minutes on develop. World-heavy tests queue for 2 slots on a 4-vCPU runner.

Decision: split the daemon scope into 3 matrix shards (`dshard`), each test assigned by a checksum of its name (`scripts/daemon-shard.sh`), so a new test needs no list edit. `scripts/check-daemon-shards.sh` fails on a test in none or several shards. The 45-minute limit and `MASON_TEST_WORLDS` stay as they are. Module-prefix filters were rejected: they need editing for every new module and are unbalanced.

Consequences: 3 more runner legs per OS. Raising the timeout would weaken the hung-test gate; raising world slots has unknown memory cost. Shard balance is statistical; if one shard is slow, raise the count in ci.yml and the script argument together.
