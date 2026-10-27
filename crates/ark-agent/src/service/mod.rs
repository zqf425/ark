//! Turn logic: the pure step, the invariant-keeping machine, and the reactor
//! that is the only place I/O happens.

pub mod dispatch;
pub mod machine;
pub mod reactor;
pub mod step;

pub use dispatch::run_calls;
pub use machine::{MachineError, TurnMachine};
pub use reactor::{ModelSeed, Reactor, ReactorConfig};
pub use step::step;
