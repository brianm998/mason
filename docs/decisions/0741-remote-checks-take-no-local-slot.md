# 0741: a remote check takes no local slot

Status: accepted (#759)

A check that runs on GitHub uses nothing on this Mac, so it must not wait for, or hold, a `compile` or `test-suite` slot, or pass the CPU and heat gates that guard them. The executor is decided before queueing (`hostedcheck::goes_remote`, built on the same decision `remote_check` makes), and a remote check queues for nothing locally. No new resource was added: the `gha` pools already bound GitHub concurrency for test steps. No gate, budget or verdict logic changed; `suites.rs` is untouched.
