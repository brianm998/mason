# 0005: Readings made while building the scheduler (Stage 8)

Where the plan or design left room, Stage 8 chose the following. Any can be overruled.

- **Two new crates.** `mason-scheduler` is pure (no clock, no IO, no dependencies;
  `s02` now checks it with the same purity checker as `core` and `engine`). It holds
  the rank tuple, the all-at-once grant with the passing rule, the wait explanations
  and a `Ledger` that owns who holds and who waits. `mason-resources` is the IO half:
  kernel file locks. The daemon's `sched.rs` joins them.
- **"Steps remaining" counts the step itself.** It is the shortest path through
  `next` to a step that ends the task, ignoring `on_red` loops, counted in steps
  still to run. A task about to run `land` has 1; in the built-in graph `explore` has
  4. A fan-out counts its shortest branch. Only comparisons matter, so the origin is
  arbitrary; counting the current step keeps "2 steps from landing" meaning "this
  and one more".
- **A queued task is ranked from where it would start, without the clarity check.**
  The check reads git and ranking runs on every admission, so a queued task is ranked
  as exploring unless it carries the `explored` flag (and the project has an
  `explored_entry`). The task's real route is still chosen at admission, as before.
- **The LLM session limit replaces `--max-concurrent` as the admission gate.**
  `llm:claude` defaults to `--max-concurrent`, so existing setups behave the same
  until a limit is set. The difference: a task in `testing` or `landing` holds no
  session, so another task can be claimed while it runs. A claimed task that has not
  yet asked for its step's resources counts as holding one, so a full pool never
  produces a worktree with nothing happening in it. An approval holds nothing.
- **What a step needs.** Its `uses` plus, for an agent step, one session on the
  provider account (only `claude` exists so far). The name `exclusive` in `uses` is
  the whole machine. Resources are declared by `[resource.NAME] count = N` in a
  definition. Names are machine-wide, so two projects naming `compile` share it,
  which is the point; the first declaration wins, and a limit the owner set (in
  `resources.toml` or live) is never overridden by a definition.
- **Kinds.** `llm:<account>` is an LLM pool; names listed under `[pools]` in
  `<config dir>/resources.toml` are leased pools; everything else is a machine
  class held by kernel locks. `resources.toml` also sets `[llm]`, `[machine]` and
  `[memory] floor_mb`. A limit changed live (`mason resources limit`) is kept in
  `<data dir>/limits.kv` and survives a restart.
- **Who holds the kernel locks.** For a pipeline step, the daemon, for the life of
  the step's job. For `mason run --needs`, the `mason` process itself: the daemon
  only says when it is that caller's turn, and the caller then takes the locks, so
  they vanish with it if it is killed. The daemon notices a dead interactive holder
  because its marker lock (`lease-<id>.lock`) is no longer held, and starts the next
  waiter in the same pass (207 ms in `s08`). A new grant has 5 seconds to take its
  locks before it is treated as dead; an interactive request that is still waiting
  but has not been polled for 10 seconds is dropped.
- **A class is a set of slot files plus one `machine.lock`.** Holders of a class
  take `machine.lock` shared; `exclusive` takes it exclusively, so the kernel keeps
  them apart.
- **Pools are accounting only for now.** A pool lease is held by a job thread or an
  interactive caller and freed when it ends or dies. Nothing uses a pool until
  Stage 25 (devices), which is where step-host heartbeats for pool leases will
  matter; the interface is in place.
- **The wait is an event once, and live on request.** The first time a step has to
  wait it records `WaitStarted` with the sentence as it stood, and `WaitEnded` when
  granted, so every monitor shows it as the task's activity. `mason task show` and
  `mason resources` ask the broker, which recomputes the sentence (positions, who is
  ahead, how long the holder has held) every time.
- **The agent hook asks, it does not allow.** `mason hook bash` rewrites a build or
  test command to run under `mason run --needs compile`, and answers
  `permissionDecision: "ask"` so the owner still sees what will run. It is optional
  and not installed anywhere by default.
- **Free memory** is `MemAvailable` on Linux and free + inactive + speculative pages
  on macOS. It is read at most once a second, and only when a floor is configured.
  The floor applies only when something else is already running, so it can never
  stop an idle machine from starting work.
