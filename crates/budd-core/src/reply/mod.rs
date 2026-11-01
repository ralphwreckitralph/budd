//! What a session's replies carry for the human: the answers its last line offers, the links it
//! pointed at, who answers its asks (autopilot), the replies not read yet, the tickets it works on,
//! the evidence it leaves, and what its conversation card shows mid-turn.
//!
//! Ported from `AvatarCore/ReplyOptions.swift`, `ReplyLinks.swift`, `Autopilot.swift`, `Unread.swift`,
//! `TicketKeys.swift`, `Evidence.swift` and `Thinking.swift`, each in its own submodule.

mod autopilot;
mod evidence;
mod links;
mod options;
mod thinking;
mod tickets;
mod unread;

pub use autopilot::{Answer, Autopilot, PilotMode};
pub use evidence::{Evidence, Item};
pub use links::{LinkKind, ReplyLink};
pub use options::{Kind, ReplyOptions};
pub use thinking::{AwaitLine, CardTurn, Sent, Shown, ToolKind};
pub use tickets::{Routed, TicketKeys, TicketProject, TicketProjects, TicketTable};
pub use unread::UnreadReplies;
