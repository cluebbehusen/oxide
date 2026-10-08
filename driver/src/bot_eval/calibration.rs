//! Attack calibration: what `oxide-opponent` believed when it launched each
//! attack, against how the attack went. Each launch is followed through the
//! seat's mission statuses until its mission ends, and the units it sent are
//! charged with the value they dealt and lost meanwhile, from the impact
//! ledger.

use crate::ledger::ImpactLedger;
use oxide_opponent::{Launch, MissionStatus, Phase};
use oxide_sim::UnitId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Ticks after an attack's mission ends during which its units' losses and
/// kills still count: a withdrawal finishes and stragglers die.
const TAIL_TICKS: u64 = 300;

/// Attacks grouped by what was sent against the known defense.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackCalibration {
    /// Launched with no defense known at the target.
    pub unknown: AttackBucket,
    /// Sent under twice the known defense.
    pub under_2x: AttackBucket,
    /// Sent at two to four times the known defense.
    pub from_2x_to_4x: AttackBucket,
    /// Sent at four times the known defense or more.
    pub over_4x: AttackBucket,
}

impl AttackCalibration {
    /// The buckets with their names, in order.
    pub fn buckets(&self) -> [(&'static str, &AttackBucket); 4] {
        [
            ("no known defense", &self.unknown),
            ("under 2x", &self.under_2x),
            ("2x to 4x", &self.from_2x_to_4x),
            ("4x and over", &self.over_4x),
        ]
    }

    /// Adds `other` to these totals.
    pub fn add(&mut self, other: &AttackCalibration) {
        self.unknown.add(&other.unknown);
        self.under_2x.add(&other.under_2x);
        self.from_2x_to_4x.add(&other.from_2x_to_4x);
        self.over_4x.add(&other.over_4x);
    }

    /// One line per group with attacks: how many, the shares that withdrew,
    /// fought on and never met the enemy, and value dealt over value lost.
    pub fn render(&self, out: &mut String, indent: &str) {
        let _ = writeln!(
            out,
            "{indent}{:<17} {:>7} {:>8} {:>6} {:>10} {:>8}",
            "sent vs known", "attacks", "withdrew", "fought", "no contact", "exchange"
        );
        for (name, bucket) in self.buckets() {
            if bucket.attacks == 0 {
                continue;
            }
            let exchange = if bucket.lost == 0 {
                "-".to_owned()
            } else {
                format!("{:.2}", bucket.dealt as f64 / bucket.lost as f64)
            };
            let _ = writeln!(
                out,
                "{indent}{:<17} {:>7} {:>8} {:>6} {:>10} {:>8}",
                name,
                bucket.attacks,
                crate::ledger::share(bucket.withdrew, bucket.attacks),
                crate::ledger::share(bucket.fought, bucket.attacks),
                crate::ledger::share(bucket.no_contact, bucket.attacks),
                exchange
            );
        }
    }

    fn bucket(&mut self, defense: u64, sent: u64) -> &mut AttackBucket {
        if defense == 0 {
            &mut self.unknown
        } else if sent < 2 * defense {
            &mut self.under_2x
        } else if sent < 4 * defense {
            &mut self.from_2x_to_4x
        } else {
            &mut self.over_4x
        }
    }
}

/// How the attacks in one group went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackBucket {
    /// Attacks launched.
    pub attacks: u64,
    /// Attacks that withdrew from a fight.
    pub withdrew: u64,
    /// Attacks that fought and never withdrew.
    pub fought: u64,
    /// Attacks that ended without fighting.
    pub no_contact: u64,
    /// Strength sent, summed.
    pub sent: u64,
    /// Scrap value the sent units lost.
    pub lost: u64,
    /// Scrap value of the damage the sent units dealt.
    pub dealt: u64,
}

impl AttackBucket {
    fn add(&mut self, other: &AttackBucket) {
        self.attacks += other.attacks;
        self.withdrew += other.withdrew;
        self.fought += other.fought;
        self.no_contact += other.no_contact;
        self.sent += other.sent;
        self.lost += other.lost;
        self.dealt += other.dealt;
    }
}

/// An attack being followed.
struct Open {
    defense: u64,
    sent: u64,
    units: Vec<UnitId>,
    dealt: u64,
    taken: u64,
    engaged: bool,
    withdrew: bool,
    ended: Option<u64>,
}

/// One seat's attacks.
#[derive(Default)]
struct SeatAttacks {
    seen: BTreeSet<u64>,
    open: BTreeMap<u64, Open>,
    done: AttackCalibration,
}

/// Follows every `oxide-opponent` seat's attacks over a leg.
pub(crate) struct AttackFollower {
    seats: Vec<Option<SeatAttacks>>,
}

fn totals(ledger: &ImpactLedger, units: &[UnitId]) -> (u64, u64) {
    units.iter().fold((0, 0), |(dealt, taken), unit| {
        let (d, t) = ledger.totals(*unit);
        (dealt + d, taken + t)
    })
}

impl AttackFollower {
    /// A follower for the seats `opponents` marks.
    pub(crate) fn new(opponents: impl IntoIterator<Item = bool>) -> Self {
        Self {
            seats: opponents
                .into_iter()
                .map(|opponent| opponent.then(SeatAttacks::default))
                .collect(),
        }
    }

    /// Opens the attacks `launches` reports for the first time, before
    /// `ledger` accounts for the tick their orders run in: an attack can land
    /// on its command tick. An ended attack still in its tail closes once any
    /// of its units launch again, so the two never share later fighting.
    pub(crate) fn open(&mut self, seat: u8, launches: &[Launch], ledger: &ImpactLedger) {
        let Some(Some(attacks)) = self.seats.get_mut(usize::from(seat)) else {
            return;
        };
        for launch in launches {
            if !attacks.seen.insert(launch.mission) {
                continue;
            }
            let relaunched: Vec<u64> = attacks
                .open
                .iter()
                .filter(|(_, open)| {
                    open.ended.is_some()
                        && open.units.iter().any(|unit| launch.units.contains(unit))
                })
                .map(|(id, _)| *id)
                .collect();
            for id in relaunched {
                let open = attacks.open.remove(&id).expect("listed as open");
                close(&mut attacks.done, &open, ledger);
            }
            let (dealt, taken) = totals(ledger, &launch.units);
            attacks.open.insert(
                launch.mission,
                Open {
                    defense: launch.defense,
                    sent: launch.sent,
                    units: launch.units.clone(),
                    dealt,
                    taken,
                    engaged: false,
                    withdrew: false,
                    ended: None,
                },
            );
        }
    }

    /// Advances `seat`'s open attacks by `missions` as its controller reports
    /// them at `tick`, closing those whose tail has passed.
    pub(crate) fn follow(
        &mut self,
        seat: u8,
        tick: u64,
        missions: &[MissionStatus],
        ledger: &ImpactLedger,
    ) {
        let Some(Some(attacks)) = self.seats.get_mut(usize::from(seat)) else {
            return;
        };
        for (id, open) in &mut attacks.open {
            if open.ended.is_some() {
                continue;
            }
            match missions.iter().find(|mission| mission.id == *id) {
                Some(mission) => match mission.phase {
                    Phase::Engage => open.engaged = true,
                    Phase::Withdraw => open.withdrew = true,
                    _ => {}
                },
                None => open.ended = Some(tick),
            }
        }
        let closing: Vec<u64> = attacks
            .open
            .iter()
            .filter(|(_, open)| open.ended.is_some_and(|ended| tick >= ended + TAIL_TICKS))
            .map(|(id, _)| *id)
            .collect();
        for id in closing {
            let open = attacks.open.remove(&id).expect("listed as open");
            close(&mut attacks.done, &open, ledger);
        }
    }

    /// Closes every attack still open and returns each seat's calibration;
    /// `None` for seats that are not `oxide-opponent`.
    pub(crate) fn finish(self, ledger: &ImpactLedger) -> Vec<Option<AttackCalibration>> {
        self.seats
            .into_iter()
            .map(|seat| {
                seat.map(|mut attacks| {
                    for open in std::mem::take(&mut attacks.open).into_values() {
                        close(&mut attacks.done, &open, ledger);
                    }
                    attacks.done
                })
            })
            .collect()
    }
}

fn close(done: &mut AttackCalibration, open: &Open, ledger: &ImpactLedger) {
    let (dealt, taken) = totals(ledger, &open.units);
    let bucket = done.bucket(open.defense, open.sent);
    bucket.attacks += 1;
    if open.withdrew {
        bucket.withdrew += 1;
    } else if open.engaged {
        bucket.fought += 1;
    } else {
        bucket.no_contact += 1;
    }
    bucket.sent += open.sent;
    bucket.dealt += crate::ledger::scrap(dealt.saturating_sub(open.dealt));
    bucket.lost += crate::ledger::scrap(taken.saturating_sub(open.taken));
}

#[cfg(test)]
mod tests;
