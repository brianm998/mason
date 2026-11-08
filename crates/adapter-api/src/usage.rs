// SPDX-License-Identifier: GPL-3.0-or-later

//! One provider-neutral shape for what an agent's provider says about its
//! limits: each window with how full it is and when it resets, overage and
//! extra billing, whether new work is refused now, and spend for an API
//! account. What a provider does not report stays empty; a percentage is never
//! estimated (DESIGN Part 6).

/// How a CLI is paid for, which decides which measures exist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BillingKind {
    /// A plan with rolling limit windows.
    #[default]
    Subscription,
    /// Pay per token: spend, and a rate limit that clears in minutes.
    Api,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowState {
    #[default]
    Ok,
    /// The provider says so (`allowed_warning`).
    Warning,
    Blocked,
}

/// One limit window. `utilization` is 0..=1 and only when the provider gave it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub id: String,
    pub label: String,
    pub length_secs: Option<u64>,
    pub utilization: Option<f64>,
    pub resets_at: Option<u64>,
    pub state: WindowState,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum OverageState {
    /// The stream says nothing about extra billing.
    #[default]
    NotOffered,
    Off,
    /// Not available, with the provider's reason.
    Disabled(String),
    /// Running on extra billing.
    InUse,
    /// Extra billing is used up.
    UsedUp,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overage {
    pub state: OverageState,
    pub resets_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockedWhy {
    /// This window (by id) is exhausted.
    Window(String),
    OverageUsedUp,
    AccountDisabled(String),
    /// A rate limit that clears in minutes, not a plan limit.
    RateLimit,
    /// The provider refused and said no more than this.
    Unreported(String),
}

/// The provider refuses new work now.
#[derive(Clone, Debug, PartialEq)]
pub struct Blocked {
    pub why: BlockedWhy,
    pub until: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageSnapshot {
    pub windows: Vec<Window>,
    pub overage: Overage,
    pub blocked: Option<Blocked>,
    /// Cost of this invocation on an `api` account, from the stream's figure.
    pub spend_usd: Option<f64>,
    /// Epoch seconds the runner received it; parsers leave it `None`.
    pub observed_at: Option<u64>,
    pub session_id: Option<String>,
}

impl UsageSnapshot {
    pub fn window(&self, id: &str) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }
}

/// A window an adapter can report.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowSpec {
    pub id: String,
    pub label: String,
    pub length_secs: Option<u64>,
}

impl WindowSpec {
    pub fn new(id: &str, label: &str, length_secs: Option<u64>) -> Self {
        WindowSpec {
            id: id.into(),
            label: label.into(),
            length_secs,
        }
    }
}

/// What an adapter can report for a billing kind. Monitors draw their rows
/// from it, so an account shows its own measures and no empty ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageModel {
    pub windows: Vec<WindowSpec>,
    /// Windows come with a utilization.
    pub utilization: bool,
    pub overage: bool,
    pub spend: bool,
}
