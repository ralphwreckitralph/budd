//! The running state machine: sessions, crew, asks, autopilot, scheduler, IPC server.
//!
//! [`engine::Engine`] is the machine, driven by inputs and answering with effects and lines; it knows
//! the OS only through [`host::ClaudeHost`]. [`daemon`] runs it: the single instance lock, the server on
//! the endpoint, the pollers and the clock.

pub mod config;
pub mod engine;
pub mod host;
pub mod state;

pub use config::Config;
pub use engine::{ClientId, Completion, Effect, Engine, Outgoing};
pub use host::{ClaudeHost, Outcome, RealHost, SendTarget};
pub use state::Persisted;
