// SPDX-License-Identifier: GPL-3.0-or-later

//! LLM provider accounts (s56): each account names its adapter and how it is
//! billed, which decides what its limits are. Parsing and resolving are pure;
//! only `load` reads the file. Credentials are never here: the adapters keep
//! using the CLIs' own logins.
//!
//! ```toml
//! # <config dir>/providers.toml
//! [provider.work]
//! adapter = "claude"          # claude | codex
//! billing = "subscription"    # subscription | api
//! sessions = 2                # optional; else --max-concurrent
//! warn_at = 0.8               # optional, in (0, 1]
//! overage = "hold"            # optional, allow | hold
//!
//! [provider.metered]
//! adapter = "codex"
//! billing = "api"
//! monthly_budget_usd = 200    # api accounts only
//! ```

use crate::config::Provider;
use std::path::Path;

pub const DEFAULT_WARN_AT: f64 = 0.8;

/// How an account is charged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Billing {
    /// A plan with its own limit windows (and maybe extra usage beyond them).
    Subscription,
    /// An API key, billed per token, with no windows.
    Api,
}

impl Billing {
    pub fn name(self) -> &'static str {
        match self {
            Billing::Subscription => "subscription",
            Billing::Api => "api",
        }
    }
}

/// What to do when the plan's own limit is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overage {
    Allow,
    Hold,
}

impl Overage {
    pub fn name(self) -> &'static str {
        match self {
            Overage::Allow => "allow",
            Overage::Hold => "hold",
        }
    }
}

pub fn adapter_name(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

/// The billing kinds an adapter understands.
pub fn billing_kinds(p: Provider) -> &'static [Billing] {
    match p {
        Provider::Claude | Provider::Codex => &[Billing::Subscription, Billing::Api],
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub name: String,
    pub adapter: Provider,
    pub billing: Billing,
    /// None: the daemon's `--max-concurrent`.
    pub sessions: Option<u32>,
    pub warn_at: f64,
    pub monthly_budget_usd: Option<f64>,
    pub overage: Overage,
}

/// The one account of an installation with no `providers.toml`.
pub fn default_accounts() -> Vec<Account> {
    vec![Account {
        name: crate::sched::DEFAULT_ACCOUNT.into(),
        adapter: Provider::Claude,
        billing: Billing::Subscription,
        sessions: None,
        warn_at: DEFAULT_WARN_AT,
        monthly_budget_usd: None,
        overage: Overage::Hold,
    }]
}

fn err(name: &str, what: impl std::fmt::Display) -> String {
    format!("providers.toml: provider.{name}: {what}")
}

pub fn parse_providers(text: &str) -> Result<Vec<Account>, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("providers.toml: {e}"))?;
    let mut out: Vec<Account> = vec![];
    for (key, item) in doc.iter() {
        if key != "provider" {
            return Err(format!(
                "providers.toml: unknown top-level `{key}`; accounts are `[provider.<name>]`"
            ));
        }
        let t = item
            .as_table_like()
            .ok_or("providers.toml: `provider` must hold `[provider.<name>]` tables")?;
        for (name, v) in t.iter() {
            if !crate::config::valid_id(name) {
                return Err(err(
                    name,
                    "the account name must be lowercase letters, digits, - and _",
                ));
            }
            let a = v
                .as_table_like()
                .ok_or_else(|| err(name, "must be a table"))?;
            for (k, _) in a.iter() {
                if !matches!(
                    k,
                    "adapter"
                        | "billing"
                        | "sessions"
                        | "warn_at"
                        | "monthly_budget_usd"
                        | "overage"
                ) {
                    return Err(err(
                        name,
                        format!(
                        "unknown key `{k}` (keys: adapter, billing, sessions, warn_at, monthly_budget_usd, overage; credentials never go in this file)"
                    ),
                    ));
                }
            }
            let text_of = |k: &str| -> Result<Option<String>, String> {
                match a.get(k) {
                    None => Ok(None),
                    Some(v) => v
                        .as_str()
                        .map(|s| Some(s.to_string()))
                        .ok_or_else(|| err(name, format!("`{k}` must be a string"))),
                }
            };
            let num_of = |k: &str| -> Result<Option<f64>, String> {
                match a.get(k) {
                    None => Ok(None),
                    Some(v) => v
                        .as_float()
                        .or_else(|| v.as_integer().map(|i| i as f64))
                        .map(Some)
                        .ok_or_else(|| err(name, format!("`{k}` must be a number"))),
                }
            };
            let adapter = match text_of("adapter")?.as_deref() {
                Some("claude") => Provider::Claude,
                Some("codex") => Provider::Codex,
                Some(o) => {
                    return Err(err(
                        name,
                        format!("`adapter` is `claude` or `codex`, not {o:?}"),
                    ))
                }
                None => return Err(err(name, "`adapter` is required (`claude` or `codex`)")),
            };
            let kinds = billing_kinds(adapter);
            let choices = kinds
                .iter()
                .map(|b| format!("`{}`", b.name()))
                .collect::<Vec<_>>()
                .join(" or ");
            let billing = match text_of("billing")? {
                None => {
                    return Err(err(
                        name,
                        format!("`billing` is required: {choices} for adapter {}", adapter_name(adapter)),
                    ))
                }
                Some(b) => *kinds.iter().find(|k| k.name() == b).ok_or_else(|| {
                    err(
                        name,
                        format!(
                            "`billing` {b:?} is not understood by the {} adapter; it knows {choices}",
                            adapter_name(adapter)
                        ),
                    )
                })?,
            };
            let sessions = match a.get("sessions") {
                None => None,
                Some(v) => Some(
                    v.as_integer()
                        .filter(|n| (1..=10_000).contains(n))
                        .ok_or_else(|| err(name, "`sessions` must be a whole number, 1 or more"))?
                        as u32,
                ),
            };
            let warn_at = num_of("warn_at")?.unwrap_or(DEFAULT_WARN_AT);
            if !(warn_at > 0.0 && warn_at <= 1.0) {
                return Err(err(
                    name,
                    format!("`warn_at` is a fraction above 0 and up to 1, not {warn_at}"),
                ));
            }
            let monthly_budget_usd = num_of("monthly_budget_usd")?;
            if let Some(b) = monthly_budget_usd {
                if billing != Billing::Api {
                    return Err(err(
                        name,
                        "`monthly_budget_usd` applies to `api` accounts only; a subscription has plan limits instead",
                    ));
                }
                if !(b > 0.0 && b.is_finite()) {
                    return Err(err(name, "`monthly_budget_usd` must be more than 0"));
                }
            }
            let overage = match text_of("overage")?.as_deref() {
                None | Some("hold") => Overage::Hold,
                Some("allow") => Overage::Allow,
                Some(o) => {
                    return Err(err(
                        name,
                        format!("`overage` is `allow` or `hold`, not {o:?}"),
                    ))
                }
            };
            out.push(Account {
                name: name.to_string(),
                adapter,
                billing,
                sessions,
                warn_at,
                monthly_budget_usd,
                overage,
            });
        }
    }
    if out.is_empty() {
        return Err("providers.toml: no accounts; add `[provider.<name>]` or delete the file for the default `claude` account".into());
    }
    Ok(out)
}

/// The accounts of the config directory, in file order; the default one if
/// there is no file.
pub fn load(config_dir: &Path) -> Result<Vec<Account>, String> {
    match std::fs::read_to_string(config_dir.join("providers.toml")) {
        Ok(t) => parse_providers(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default_accounts()),
        Err(e) => Err(format!("providers.toml: {e}")),
    }
}

/// Which account a project uses: the one it names, else the first account
/// whose adapter is the project's provider.
pub fn resolve_account(
    accounts: &[Account],
    provider: Provider,
    explicit: Option<&str>,
) -> Result<String, String> {
    match explicit {
        Some(n) => {
            if accounts.iter().any(|a| a.name == n) {
                Ok(n.to_string())
            } else {
                let have = accounts
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(format!(
                    "`account` {n:?} is not in providers.toml (accounts: {have})"
                ))
            }
        }
        None => accounts
            .iter()
            .find(|a| a.adapter == provider)
            .map(|a| a.name.clone())
            .ok_or_else(|| {
                format!(
                    "provider = {:?} but no account uses the {} adapter: add `[provider.<name>]` with adapter = {:?} to providers.toml, or name an `account`",
                    adapter_name(provider),
                    adapter_name(provider),
                    adapter_name(provider)
                )
            }),
    }
}

/// An account whose sessions are set in providers.toml must not also be in
/// resources.toml `[llm]`: two answers to one question.
pub fn check_llm_conflict(
    accounts: &[Account],
    llm: &std::collections::BTreeMap<String, u32>,
) -> Result<(), String> {
    for a in accounts {
        if a.sessions.is_some() && llm.contains_key(&a.name) {
            return Err(format!(
                "account {0:?} sets sessions in providers.toml and in resources.toml [llm]; keep one",
                a.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(body: &str) -> Result<Account, String> {
        parse_providers(&format!("[provider.a]\n{body}")).map(|v| v[0].clone())
    }

    #[test]
    fn each_billing_kind_of_each_adapter_parses_with_defaults() {
        for ad in ["claude", "codex"] {
            for b in ["subscription", "api"] {
                let a = one(&format!("adapter=\"{ad}\"\nbilling=\"{b}\"\n")).unwrap();
                assert_eq!(adapter_name(a.adapter), ad);
                assert_eq!(a.billing.name(), b);
                assert_eq!(
                    (a.warn_at, a.overage, a.sessions),
                    (0.8, Overage::Hold, None)
                );
            }
        }
    }

    #[test]
    fn unknown_adapter_and_billing_are_refused_with_the_choices() {
        let e = one("adapter=\"gemini\"\nbilling=\"api\"\n").unwrap_err();
        assert!(e.contains("`claude` or `codex`"), "{e}");
        let e = one("adapter=\"claude\"\nbilling=\"free\"\n").unwrap_err();
        assert!(
            e.contains("`subscription` or `api`") && e.contains("provider.a"),
            "{e}"
        );
        assert!(one("adapter=\"claude\"\n").unwrap_err().contains("billing"));
    }

    #[test]
    fn owner_settings_are_validated() {
        let ok = one("adapter=\"codex\"\nbilling=\"api\"\nsessions=3\nwarn_at=0.5\nmonthly_budget_usd=120\noverage=\"allow\"\n").unwrap();
        assert_eq!(
            (ok.sessions, ok.warn_at, ok.monthly_budget_usd, ok.overage),
            (Some(3), 0.5, Some(120.0), Overage::Allow)
        );
        let base = "adapter=\"claude\"\nbilling=\"subscription\"\n";
        for bad in [
            "warn_at=0\n",
            "warn_at=1.5\n",
            "warn_at=-1\n",
            "warn_at=\"x\"\n",
        ] {
            assert!(
                one(&format!("{base}{bad}"))
                    .unwrap_err()
                    .contains("warn_at"),
                "{bad}"
            );
        }
        assert!(one(&format!("{base}warn_at=1\n")).is_ok());
        let e = one(&format!("{base}monthly_budget_usd=5\n")).unwrap_err();
        assert!(e.contains("api"), "{e}");
        assert!(one("adapter=\"claude\"\nbilling=\"api\"\nmonthly_budget_usd=0\n").is_err());
        assert!(one(&format!("{base}overage=\"maybe\"\n"))
            .unwrap_err()
            .contains("`allow` or `hold`"));
        assert!(one(&format!("{base}sessions=0\n")).is_err());
        assert!(one(&format!("{base}api_key=\"sk\"\n"))
            .unwrap_err()
            .contains("credentials"));
    }

    #[test]
    fn accounts_keep_file_order_and_names_are_checked() {
        let v = parse_providers("[provider.zeta]\nadapter=\"codex\"\nbilling=\"api\"\n[provider.alpha]\nadapter=\"claude\"\nbilling=\"subscription\"\n").unwrap();
        assert_eq!(v[0].name, "zeta");
        assert!(parse_providers("[provider.Bad]\nadapter=\"claude\"\nbilling=\"api\"\n").is_err());
        assert!(parse_providers("").is_err());
    }

    #[test]
    fn resolution_follows_the_project() {
        let v = parse_providers("[provider.c1]\nadapter=\"claude\"\nbilling=\"api\"\n[provider.x1]\nadapter=\"codex\"\nbilling=\"subscription\"\n[provider.x2]\nadapter=\"codex\"\nbilling=\"api\"\n").unwrap();
        assert_eq!(resolve_account(&v, Provider::Codex, None).unwrap(), "x1");
        assert_eq!(resolve_account(&v, Provider::Claude, None).unwrap(), "c1");
        assert_eq!(
            resolve_account(&v, Provider::Claude, Some("x2")).unwrap(),
            "x2"
        );
        let e = resolve_account(&v, Provider::Claude, Some("nope")).unwrap_err();
        assert!(e.contains("\"nope\"") && e.contains("c1, x1, x2"), "{e}");
        let e = resolve_account(&default_accounts(), Provider::Codex, None).unwrap_err();
        assert!(
            e.contains("adapter = \"codex\"") && e.contains("providers.toml"),
            "{e}"
        );
    }

    #[test]
    fn default_account_is_claude_subscription() {
        let d = default_accounts();
        assert_eq!(
            (d.len(), d[0].name.as_str(), d[0].billing),
            (1, "claude", Billing::Subscription)
        );
    }

    #[test]
    fn sessions_in_both_files_is_refused() {
        let v = parse_providers("[provider.a]\nadapter=\"claude\"\nbilling=\"api\"\nsessions=2\n[provider.b]\nadapter=\"claude\"\nbilling=\"api\"\n").unwrap();
        let mut llm = std::collections::BTreeMap::new();
        llm.insert("b".to_string(), 3);
        assert!(check_llm_conflict(&v, &llm).is_ok());
        llm.insert("a".to_string(), 1);
        assert!(check_llm_conflict(&v, &llm).unwrap_err().contains("\"a\""));
    }
}
