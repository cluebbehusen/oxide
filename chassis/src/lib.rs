#![doc = include_str!("../README.md")]
// Floats and hash-ordered collections would break bit-identical replays.
#![deny(clippy::float_arithmetic, clippy::disallowed_types)]

pub mod compass;
pub mod fsx;
pub mod fx;
pub mod grid;
pub mod hash;
pub mod path;
pub mod replay;
pub mod rng;

/// Simulation time, counted in fixed-timestep ticks since scenario start.
pub type Tick = u64;

/// Declares a fieldless enum together with an `ALL` list of its variants,
/// so the list cannot omit a variant or drift from declaration order.
#[macro_export]
macro_rules! listed_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident,)+
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: [$name; [$($name::$variant),+].len()] = [$($name::$variant),+];
        }
    };
}
