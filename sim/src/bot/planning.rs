//! Deterministic work allowances shared by nested planning services.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Progress<T> {
    Ready(T),
    ProvenInfeasible,
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WorkBudget {
    remaining: usize,
    spent: usize,
}

impl WorkBudget {
    pub(super) const fn new(allowance: usize) -> Self {
        Self {
            remaining: allowance,
            spent: 0,
        }
    }

    pub(super) fn charge(&mut self, amount: usize) -> bool {
        if amount > self.remaining {
            return false;
        }
        self.remaining -= amount;
        self.spent += amount;
        true
    }

    pub(super) const fn spent(&self) -> usize {
        self.spent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_services_cannot_refill_or_overdraw_the_shared_allowance() {
        fn service(budget: &mut WorkBudget) -> bool {
            budget.charge(3)
        }
        let mut budget = WorkBudget::new(5);
        assert!(service(&mut budget));
        assert!(!service(&mut budget));
        assert_eq!(budget.spent(), 3);
        assert!(budget.charge(2));
        assert!(!budget.charge(1));
        assert_eq!(budget.spent(), 5);
    }

    #[test]
    fn zero_and_maximum_allowances_do_not_wrap() {
        let mut empty = WorkBudget::new(0);
        assert!(!empty.charge(1));
        let mut maximum = WorkBudget::new(usize::MAX);
        assert!(maximum.charge(usize::MAX));
        assert!(!maximum.charge(1));
        assert_eq!(maximum.spent(), usize::MAX);
    }
}
