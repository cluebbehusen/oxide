//! Capture provenance in the native executable.
#[path = "../build-support/identity.rs"]
mod identity;

fn main() {
    identity::configure("shell");
}
