//! Pure logic of Budd: sessions, presence, crew, replies, history.
//!
//! Nothing here touches the file system, processes or the network.
//! Every module is a port of the Swift `AvatarCore` module or Python hook it names, with the same tests.

pub mod account;
pub mod crew;
pub mod delivery;
pub mod fingerprint;
pub mod history;
pub mod hook;
pub mod keepalive;
pub mod location;
pub mod presence;
pub mod reply;
pub mod state;
pub mod text;
pub mod tools;
pub mod transcript;
