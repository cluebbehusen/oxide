//! Deterministic best-first refinement over mutually exclusive domain choices.

use super::*;
use crate::planning::{Progress, WorkBudget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Continuation {
    groups: Vec<Vec<Option<usize>>>,
    frontier: BTreeSet<(PortfolioRank, Reverse<Vec<usize>>)>,
    seen: BTreeSet<Vec<usize>>,
}

impl Continuation {
    pub fn new<Payload>(
        proposals: &[InvestmentProposal<Payload>],
        personality: AllocationPersonality,
        required: Option<usize>,
    ) -> Self {
        let mut domains: Vec<(ProposalDomain, Vec<usize>)> = Vec::new();
        for (index, proposal) in proposals.iter().enumerate() {
            if let Some((_, choices)) = domains
                .iter_mut()
                .find(|(domain, _)| *domain == proposal.key().domain())
            {
                choices.push(index);
            } else {
                domains.push((proposal.key().domain(), vec![index]));
            }
        }
        let groups = domains
            .into_iter()
            .map(|(_, mut choices)| {
                if let Some(required) = required.filter(|required| choices.contains(required)) {
                    return vec![Some(required)];
                }
                choices.sort_by_cached_key(|&index| {
                    Reverse(portfolio_rank(&[index], proposals, personality))
                });
                choices.into_iter().map(Some).chain([None]).collect()
            })
            .collect::<Vec<_>>();
        let positions = vec![0; groups.len()];
        let mut result = Self {
            groups,
            frontier: BTreeSet::new(),
            seen: BTreeSet::new(),
        };
        result.push(positions, proposals, personality);
        result
    }

    fn selected(&self, positions: &[usize]) -> Vec<usize> {
        let mut selected = positions
            .iter()
            .enumerate()
            .filter_map(|(group, &position)| self.groups[group][position])
            .collect::<Vec<_>>();
        selected.sort_unstable();
        selected
    }

    fn push<Payload>(
        &mut self,
        positions: Vec<usize>,
        proposals: &[InvestmentProposal<Payload>],
        personality: AllocationPersonality,
    ) {
        if self.seen.insert(positions.clone()) {
            let rank = portfolio_rank(&self.selected(&positions), proposals, personality);
            self.frontier.insert((rank, Reverse(positions)));
        }
    }

    pub fn advance<Payload>(
        &mut self,
        proposals: &[InvestmentProposal<Payload>],
        personality: AllocationPersonality,
        budget: &mut WorkBudget,
    ) -> Progress<Vec<usize>> {
        if self.frontier.is_empty() {
            return Progress::ProvenInfeasible;
        }
        if !budget.charge(1) {
            return Progress::Deferred;
        }
        let (_, Reverse(positions)) = self.frontier.pop_last().unwrap();
        let selected = self.selected(&positions);
        for group in 0..self.groups.len() {
            if positions[group] + 1 < self.groups[group].len() {
                let mut successor = positions.clone();
                successor[group] += 1;
                self.push(successor, proposals, personality);
            }
        }
        Progress::Ready(selected)
    }
}
