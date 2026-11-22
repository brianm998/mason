# 0904: A fallback run the caller already holds slots for does not ask again

Stage s911. Decision 0805 made a run sent back from GitHub take this machine's slots through the
broker. A landing batch already holds those slots for its suite run, so its fallback asked for them
a second time, ranked behind everything, and queued behind an exclusive request that was itself
waiting for the batch's hold to drain. The queue froze (2026-11-22).

Decision: a run whose caller holds this machine's slots for it is marked `covered` and, on fallback,
runs under that hold without a second request. The alternative, the fallback acquiring as the
batch's owner with the batch's rank, still needs the same slots again from a machine whose units the
batch itself occupies, and can queue behind an exclusive request the same way; only not asking makes
the cycle impossible. Covered is set only when the batch's `hold` returned a lease. Runs that hold
nothing before going remote (task checks, test steps, debts, landing checks with empty `holds`) keep
decision 0805 unchanged.

The batch's choice to hold slots while its suite is on GitHub is not changed here.
