//! What a profile says about its heap: who owns the memory, who retains it,
//! what probably leaks, and what grew between two dumps. Ported from
//! heap-landscape's `web/js/model`, whose results they give.

pub mod diff;
pub mod leaks;
pub mod ownership;
pub mod retainers;
