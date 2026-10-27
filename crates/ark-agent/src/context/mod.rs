//! Context utilities: the single token estimator and history hygiene.

pub mod hygiene;
pub mod token;

pub use hygiene::{HistoryHygiene, HygieneReport};
pub use token::TokenEstimator;
