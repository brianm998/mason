// SPDX-License-Identifier: GPL-3.0-or-later

//! Who may do what over the network (Part 16), as decisions over values. A device that
//! paired holds a token with one of two roles; a pairing code turns into a token once. Nothing
//! here reads a clock or makes a random number: time and the code itself come in as arguments.

/// A pairing code lives this long.
pub const CODE_TTL_MS: u64 = 120_000;
/// Wrong tries allowed before every outstanding code is voided.
pub const MAX_WRONG: u32 = 5;
/// A code is this many digits.
pub const CODE_DIGITS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Acts as the owner: answers, cancels, lands.
    Owner,
    /// Reads and subscribes; changes nothing.
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Viewer => "viewer",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "owner" => Some(Role::Owner),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Allow,
    Refuse(u16, String),
}

/// May a device with this role make this request? Anything that is not a plain read is a
/// change. Managing devices and pairing codes is the owner's alone.
pub fn decide(role: Role, method: &str, path: &str) -> Access {
    let read = matches!(method, "GET" | "HEAD");
    let admin = path == "/devices"
        || path.starts_with("/devices/")
        || path == "/pair/codes"
        || path.starts_with("/lan");
    match role {
        Role::Owner => Access::Allow,
        Role::Viewer if admin => Access::Refuse(403, "a viewer cannot manage devices".into()),
        Role::Viewer if read => Access::Allow,
        Role::Viewer => Access::Refuse(403, "a viewer can look but not change anything".into()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairError {
    /// Not eight digits.
    Malformed,
    /// No such code (or one already used).
    Wrong,
    Expired,
    /// Too many wrong tries: issue a new code.
    Locked,
}

impl PairError {
    pub fn message(self) -> &'static str {
        match self {
            PairError::Malformed => "a pairing code is eight digits",
            PairError::Wrong => "that code is not right",
            PairError::Expired => "that code has expired; ask for a new one",
            PairError::Locked => "too many wrong codes; ask for a new one",
        }
    }
}

#[derive(Clone, Debug)]
struct Pending {
    code: String,
    role: Role,
    issued_ms: u64,
}

/// The codes waiting to be used. One failure count covers all of them, since a guesser does not
/// know which code it is guessing.
#[derive(Clone, Debug, Default)]
pub struct Pairing {
    pending: Vec<Pending>,
    wrong: u32,
}

pub fn well_formed(code: &str) -> bool {
    code.len() == CODE_DIGITS && code.bytes().all(|b| b.is_ascii_digit())
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |x, (p, q)| x | (p ^ q)) == 0
}

impl Pairing {
    /// Record a new code (made by the caller). Issuing starts the failure count again.
    pub fn issue(&mut self, code: String, role: Role, now_ms: u64) {
        self.pending
            .retain(|p| now_ms.saturating_sub(p.issued_ms) < CODE_TTL_MS);
        self.pending.push(Pending {
            code,
            role,
            issued_ms: now_ms,
        });
        self.wrong = 0;
    }

    /// Try a code. A right one is used up.
    pub fn attempt(&mut self, code: &str, now_ms: u64) -> Result<Role, PairError> {
        if self.wrong >= MAX_WRONG {
            return Err(PairError::Locked);
        }
        if !well_formed(code) {
            return self.miss(PairError::Malformed);
        }
        let at = self.pending.iter().position(|p| same(&p.code, code));
        let Some(i) = at else {
            return self.miss(PairError::Wrong);
        };
        let p = self.pending.remove(i);
        if now_ms.saturating_sub(p.issued_ms) >= CODE_TTL_MS {
            return self.miss(PairError::Expired);
        }
        Ok(p.role)
    }

    fn miss(&mut self, e: PairError) -> Result<Role, PairError> {
        self.wrong += 1;
        if self.wrong >= MAX_WRONG {
            self.pending.clear();
        }
        Err(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_reads_but_changes_nothing() {
        assert_eq!(decide(Role::Viewer, "GET", "/views/home"), Access::Allow);
        assert!(matches!(
            decide(Role::Viewer, "POST", "/tasks/3/cancel"),
            Access::Refuse(403, _)
        ));
        assert!(matches!(
            decide(Role::Viewer, "GET", "/devices"),
            Access::Refuse(403, _)
        ));
        assert_eq!(
            decide(Role::Owner, "POST", "/tasks/3/cancel"),
            Access::Allow
        );
    }

    #[test]
    fn a_code_works_once() {
        let mut p = Pairing::default();
        p.issue("12345678".into(), Role::Viewer, 0);
        assert_eq!(p.attempt("12345678", 10), Ok(Role::Viewer));
        assert_eq!(p.attempt("12345678", 20), Err(PairError::Wrong));
    }

    #[test]
    fn a_code_expires() {
        let mut p = Pairing::default();
        p.issue("12345678".into(), Role::Owner, 0);
        assert_eq!(p.attempt("12345678", CODE_TTL_MS), Err(PairError::Expired));
        p.issue("22345678".into(), Role::Owner, 1000);
        assert_eq!(
            p.attempt("22345678", 1000 + CODE_TTL_MS - 1),
            Ok(Role::Owner)
        );
    }

    #[test]
    fn five_wrong_tries_lock_even_the_right_code() {
        let mut p = Pairing::default();
        p.issue("12345678".into(), Role::Owner, 0);
        for _ in 0..MAX_WRONG {
            assert!(p.attempt("00000000", 1).is_err());
        }
        assert_eq!(p.attempt("12345678", 2), Err(PairError::Locked));
        p.issue("87654321".into(), Role::Owner, 3);
        assert_eq!(p.attempt("87654321", 4), Ok(Role::Owner));
    }

    #[test]
    fn malformed_codes_count_as_wrong() {
        let mut p = Pairing::default();
        assert_eq!(p.attempt("12", 0), Err(PairError::Malformed));
        assert_eq!(p.attempt("1234567a", 0), Err(PairError::Malformed));
        assert!(!well_formed("123456789"));
    }
}
