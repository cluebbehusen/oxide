//! Capture provenance in the driver executable.
#[path = "../build-support/identity.rs"]
mod identity;

fn main() {
    identity::configure("driver");
}
