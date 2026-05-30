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

    pub fn decrement(&mut self, amount: u64) -> Result<(), BudgetError> {
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

    pub fn use_step(&mut self) -> Result<(), BudgetError> {
        self.steps.decrement(1)
    }

    pub fn use_subcall(&mut self) -> Result<(), BudgetError> {
        self.subcalls.decrement(1)
    }
}

#[cfg(test)]
mod tests {
    use super::{Budget, BudgetError, Guardrails};

    #[test]
    fn budget_decrements_until_exhausted() {
        let mut budget = Budget::new("steps", 2);

        assert_eq!(budget.decrement(1), Ok(()));
        assert_eq!(budget.remaining(), 1);
        assert_eq!(
            budget.decrement(2),
            Err(BudgetError::Exhausted {
                kind: "steps",
                requested: 2,
                remaining: 1,
            })
        );
        assert_eq!(budget.remaining(), 1);
    }

    #[test]
    fn guardrails_track_step_and_subcall_budgets() {
        let mut guardrails = Guardrails::new(1, 1);

        assert_eq!(guardrails.use_step(), Ok(()));
        assert_eq!(guardrails.use_subcall(), Ok(()));
        assert!(matches!(
            guardrails.use_step(),
            Err(BudgetError::Exhausted {
                kind: "steps",
                requested: 1,
                remaining: 0,
            })
        ));
        assert_eq!(guardrails.steps().remaining(), 0);
        assert_eq!(guardrails.subcalls().remaining(), 0);
    }
}
