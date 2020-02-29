//! This crate implements a minimal abstraction over UNIX domain sockets for
//! the purpose of IPC.  It lets you send both file handles and rust objects
//! between processes.
//!
//! # Feature Flags
//!
//! * `serde`: enables serialization and deserialization.
//! * `bootstrap`: adds the `Bootstrapper` type.
//! * `bootstrap-simple`: adds the default `new` constructor to the
//!   bootstrapper.
mod raw_channel;

#[cfg(feature = "bootstrap")]
mod bootstrap;
#[cfg(feature = "serde")]
mod serde;
#[cfg(feature = "serde")]
mod typed_channel;

pub use self::raw_channel::*;

#[cfg(feature = "bootstrap")]
pub use self::bootstrap::*;

#[cfg(feature = "serde")]
pub use self::{serde::*, typed_channel::*};

#[doc(hidden)]
#[cfg(feature = "serde")]
pub use ::serde_ as _serde_ref;
