# 0034: Provider accounts, the choices the ticket left open

Task #25 (the first of four usage tickets). Each is the reading taken; change one and the others stand.

- **A separate `providers.toml`** in the config directory, with `[provider.<name>]` tables in file order. `resources.toml` stays machine-wide facts; an account is something with a kind, not a number.
- **`adapter` is `claude` or `codex`** (the existing `config::Provider`). Names follow the project id rule (lowercase letters, digits, `-`, `_`). Unknown keys are refused, and the message says credentials never go in the file.
- **Billing kinds: both adapters accept `subscription` and `api`.** The per-adapter list is `providers::billing_kinds`; an unknown kind is refused at load naming the adapter and the kinds it knows.
- **Sessions.** `sessions` in an account wins; with none, the account takes `--max-concurrent`. `[llm] <name> = n` still sets that name. Naming the same account in both is refused at load (two answers to one question). An `[llm]` name that is not an account still becomes a capacity, as before.
- **Owner settings** are parsed and validated now and do nothing yet: `warn_at` in (0, 1], default 0.8; `monthly_budget_usd` > 0, `api` accounts only (refused on a subscription); `overage` `allow` or `hold`, default `hold`.
- **No file:** one account `claude` (claude, subscription, sessions = `--max-concurrent`). An empty file is refused (delete it for the default).
- **Project to account.** `account = "<name>"` in the overlay. Without it, `provider` picks the first account in file order whose adapter matches; none is an error naming the fix (add an account with that adapter). A missing named account is refused at load naming the project and the account, so the daemon does not start. `ProjectCfg.account` is set provisionally by `parse_project` (the adapter's name) and for real by `load_projects`, which knows the accounts.
- **Admission is per account.** `acquire` takes `llm:<project's account>`; `llm_free(w, account)` counts only that account's unclaimed tasks; `next_admission` takes the first ranked queued task whose account has a free session, so a full account holds back only its own projects.
- **Home view sessions** sum used and limit over all accounts.
- **Protocol (additive, PROTOCOL stays 1):** `ProviderStatus` 5 to 9 (adapter, billing, warn_at, overage, monthly_budget_usd; 0 means none set).
- **Doctor and login (open for the owner).** Doctor reports per account the adapter's CLI (a `claude_bin`/`codex_bin` set on a project using it, else PATH), billing and projects. It does not probe login: the adapters learn "Not logged in" only when a step runs, and a probe would start the CLI. The line says so rather than guess. A later ticket can read the last such crash from the store.
