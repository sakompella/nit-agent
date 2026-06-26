use rlm_anywhere::rlm::{Budget, BudgetError, Guardrails};

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
