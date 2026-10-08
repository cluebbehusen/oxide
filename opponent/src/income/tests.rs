use super::*;

#[test]
fn income_counts_spending_and_skips_samples_after_a_rejection() {
    let mut income = Income::default();
    assert_eq!(income.observe(0, 150, false), 0, "no previous decision");
    income.record(0, 150, 100);
    assert_eq!(income.observe(12, 80, false), 30);
    assert_eq!(income.per_minute(), 750);
    income.record(12, 80, 0);
    assert_eq!(income.observe(24, 200, true), 0);
    assert_eq!(income.per_minute(), 750, "unchanged");
    assert_eq!(income.validate(24), Ok(()));
    assert!(income.validate(11).is_err());
}
