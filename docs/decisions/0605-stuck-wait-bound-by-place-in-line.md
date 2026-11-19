# 0605: stuck:wait is bounded by the work ahead of the waiter

Status: accepted (s612). Amends 0547.

Context: 0547 bounded a wait with holders by queue depth x longest holder run, for every waiter. A task 2nd in line (#516, 9-deep queue) was judged against the whole queue and flagged only after 259 min, though one holder run was ahead of it.

Decision: the `stuck:wait` bound is max(`wait_m`, position x longest holder run), where position is the waiter's place in its class (1 is next). The whole-queue bound (queue depth x run) is kept only as evidence on the `stuck:aged-wait` note. Quiet holders (no grant within `wait_m`) still raise `stuck:wait` at once.

Consequence: for #516 the verdict is unchanged, since the `wait_m` floor (2 h) is the larger bound and 265 min is flagged; only the evidence number changes (259 to 120 min). Tradeoff: the front of a shared queue reaches the 2 h floor before a deep-queue bound, so some of the warns 0547 removed return for positions 1 to about 6 at a 20-min run.

Trust: the daemon publishes position beside depth and holder grant times in `crates/daemon/src/sched.rs`, which is protected and needs owner approval.
