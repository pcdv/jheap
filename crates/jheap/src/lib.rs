//! Reads JVM heap dumps (.hprof) and says what holds the memory.
//!
//! [`parse`] reads a dump through a window, never whole, and gives every path
//! from a root with the bytes and objects that end there: what heap-landscape
//! draws, and what the models of the command line start from.

mod blocks;
mod hash;
pub mod hprof;

pub use hprof::{Error, Options, Parse, Path, Phase, Pointers, Report, parse};
