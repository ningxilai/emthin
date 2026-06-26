//! DBus router — message routing decision engine + subprocess types.

mod ipc;
mod rule;

pub use ipc::*;
pub use rule::*;
