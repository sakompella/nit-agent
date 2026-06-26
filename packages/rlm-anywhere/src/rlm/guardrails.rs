#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BudgetError {
    #[error("{kind} budget exhausted: requested {requested}, remaining {remaining}")]
    Exhausted {
        kind: &'static str,
        requested: u64,
        remaining: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    kind: &'static str,
    remaining: u64,
}

impl Budget {
    #[must_use]
    pub const fn new(kind: &'static str, limit: u64) -> Self {
        Self {
            kind,
            remaining: limit,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    #[must_use]
    pub const fn remaining(&self) -> u64 {
        self.remaining
    }

    /// # Errors
    /// Returns an error if the budget would go below zero.
    pub const fn decrement(&mut self, amount: u64) -> Result<(), BudgetError> {
        let Some(remaining) = self.remaining.checked_sub(amount) else {
            return Err(BudgetError::Exhausted {
                kind: self.kind,
                requested: amount,
                remaining: self.remaining,
            });
        };

        self.remaining = remaining;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guardrails {
    steps: Budget,
    subcalls: Budget,
}

impl Guardrails {
    #[must_use]
    pub const fn new(max_steps: u64, max_subcalls: u64) -> Self {
        Self {
            steps: Budget::new("steps", max_steps),
            subcalls: Budget::new("subcalls", max_subcalls),
        }
    }

    #[must_use]
    pub const fn steps(&self) -> Budget {
        self.steps
    }

    #[must_use]
    pub const fn subcalls(&self) -> Budget {
        self.subcalls
    }

    /// # Errors
    /// Returns an error if the step budget is exhausted.
    pub fn use_step(&mut self) -> Result<(), BudgetError> {
        self.steps.decrement(1)
    }

    /// # Errors
    /// Returns an error if the subcall budget is exhausted.
    pub fn use_subcall(&mut self) -> Result<(), BudgetError> {
        self.subcalls.decrement(1)
    }
}
