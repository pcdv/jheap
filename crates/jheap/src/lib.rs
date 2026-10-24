//! Reads JVM heap dumps (.hprof) and says what holds the memory.
//!
//! [`parse`] reads a dump through a window, never whole, and gives every path
//! from a root with the bytes and objects that end there: what heap-landscape
//! draws. A [`Profile`] holds those paths as a tree, and the [`model`]s say
//! from it who owns the memory, who retains it, what could leak and what grew.

mod blocks;
mod hash;
pub mod hprof;
pub mod model;
pub mod profile;

pub use hprof::{Error, Options, Parse, Path, Phase, Pointers, Report, parse};
pub use profile::Profile;
