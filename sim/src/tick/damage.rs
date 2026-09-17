//! HP loss and owner-only notification shared by combat phases.

use crate::event::Event;
use crate::state::{Building, Unit};

pub(super) fn unit(victim: &mut Unit, amount: u32, events: &mut Vec<Event>) {
    if victim.hp == 0 || amount == 0 {
        return;
    }
    victim.hp = victim.hp.saturating_sub(amount);
    events.push(Event::DamageTaken {
        player: victim.player,
        pos: victim.pos,
    });
}

pub(super) fn building(victim: &mut Building, amount: u32, events: &mut Vec<Event>) {
    if victim.hp == 0 || amount == 0 || victim.provisional {
        return;
    }
    victim.hp = victim.hp.saturating_sub(amount);
    events.push(Event::DamageTaken {
        player: victim.player,
        pos: victim.center(),
    });
}
