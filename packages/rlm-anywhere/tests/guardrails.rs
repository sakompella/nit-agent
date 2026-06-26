use hegel::TestCase;
use hegel::generators;

use rlm_anywhere::rlm::{Budget, BudgetError, Guardrails};

/// Stateful model: a single `Budget` must behave like a saturating `u64`
/// counter that refuses to go below zero.
struct BudgetModel {
    subject: Budget,
    model: u64,
}

#[hegel::state_machine]
impl BudgetModel {
    #[rule]
    // The `#[rule]` macro requires `fn(&mut self, TestCase)`; `TestCase` is
    // used by reference (`tc.draw`) but the by-value signature is mandated.
    #[allow(clippy::needless_pass_by_value)]
    fn decrement(&mut self, tc: TestCase) {
        let amount = tc.draw(generators::integers::<u64>());
        let result = self.subject.decrement(amount);

        match self.model.checked_sub(amount) {
            Some(remaining) => {
                assert_eq!(result, Ok(()), "decrement within budget must succeed");
                self.model = remaining;
            }
            None => {
                assert!(
                    matches!(result, Err(BudgetError::Exhausted { .. })),
                    "decrement past zero must report exhaustion"
                );
                // Subject must leave `remaining` untouched on failure; the
                // invariant below confirms it still equals the model.
            }
        }
    }

    #[invariant]
    fn remaining_matches_model(&mut self, _: TestCase) {
        assert_eq!(self.subject.remaining(), self.model);
    }
}

#[hegel::test]
fn budget_matches_saturating_model(tc: TestCase) {
    let limit = tc.draw(generators::integers::<u64>());
    let machine = BudgetModel {
        subject: Budget::new("steps", limit),
        model: limit,
    };
    hegel::stateful::run(machine, tc);
}

/// Stateful model: `Guardrails` tracks step and subcall budgets
/// independently, each as its own saturating counter.
struct GuardrailsModel {
    subject: Guardrails,
    model_steps: u64,
    model_subcalls: u64,
}

#[hegel::state_machine]
impl GuardrailsModel {
    #[rule]
    fn use_step(&mut self, _: TestCase) {
        let result = self.subject.use_step();
        match self.model_steps.checked_sub(1) {
            Some(remaining) => {
                assert_eq!(result, Ok(()));
                self.model_steps = remaining;
            }
            None => assert!(matches!(
                result,
                Err(BudgetError::Exhausted { kind: "steps", .. })
            )),
        }
    }

    #[rule]
    fn use_subcall(&mut self, _: TestCase) {
        let result = self.subject.use_subcall();
        match self.model_subcalls.checked_sub(1) {
            Some(remaining) => {
                assert_eq!(result, Ok(()));
                self.model_subcalls = remaining;
            }
            None => assert!(matches!(
                result,
                Err(BudgetError::Exhausted {
                    kind: "subcalls",
                    ..
                })
            )),
        }
    }

    #[invariant]
    fn budgets_match_model(&mut self, _: TestCase) {
        assert_eq!(self.subject.steps().remaining(), self.model_steps);
        assert_eq!(self.subject.subcalls().remaining(), self.model_subcalls);
    }
}

#[hegel::test]
fn guardrails_track_budgets_independently(tc: TestCase) {
    // Small limits so exhaustion is actually reachable within a run.
    let max_steps = tc.draw(generators::integers::<u64>().max_value(8));
    let max_subcalls = tc.draw(generators::integers::<u64>().max_value(8));
    let machine = GuardrailsModel {
        subject: Guardrails::new(max_steps, max_subcalls),
        model_steps: max_steps,
        model_subcalls: max_subcalls,
    };
    hegel::stateful::run(machine, tc);
}

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
