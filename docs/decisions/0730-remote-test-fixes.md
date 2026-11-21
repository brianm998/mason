# 0730: The remote test reds are fixed in the tests

Three `remote::tests` failed at ca5406fb because their expectations were wrong (a non-leap date pair, a skip scheduled after the run's time limit, a substring match on a template comment). The code behaves correctly.

**Chosen.** Fix the tests; no production or template change.
