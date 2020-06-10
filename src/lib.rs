//! This crate implements a minimal abstraction over UNIX domain sockets for
//! the purpose of IPC.  It lets you send both file handles and rust objects
//! between processes.
//!
//! # How it works
//!
//! This uses [serde](https://serde.rs/) to serialize data over unix sockets
//! via [bincode](https://github.com/servo/bincode).  Thanks to the
//! [`Handle`](struct.Handle.html) abstraction you can also send any object
//! across that is convertable into a unix file handle.
//!
//! The way this works under the hood is that during serialization and
//! deserialization encountered file descriptors are tracked.  They are then
//! sent over the unix socket separately.  This lets unassociated processes
//! share file handles.
//!
//! If you only want the unix socket abstraction you can disable all default
//! features and use the raw channels.
//!
//! # Example
//!
//! ```rust
//! # use ::serde_ as serde;
//! use std::env;
//! use std::process;
//! use unix_ipc::{channel, Bootstrapper, BincodeReceiver as Receiver, BincodeSender as Sender, Bincode};
//! use serde::{Deserialize, Serialize};
//!
//! const ENV_VAR: &str = "PROC_CONNECT_TO";
//!
//! #[derive(Serialize, Deserialize, Debug)]
//! # #[serde(crate = "serde_")]
//! pub enum Task {
//!     Sum(Vec<i64>, Sender<i64>),
//!     Shutdown,
//! }
//!
//! if let Ok(path) = env::var(ENV_VAR) {
//!     let receiver = Receiver::<Task>::connect(path).unwrap();
//!     loop {
//!         match receiver.recv().unwrap() {
//!             Task::Sum(values, tx) => {
//!                 tx.send(values.into_iter().sum::<i64>()).unwrap();
//!             }
//!             Task::Shutdown => break,
//!         }
//!     }
//! } else {
//!     let bootstrapper: Bootstrapper<Bincode, _> = Bootstrapper::new().unwrap();
//!     let mut child = process::Command::new(env::current_exe().unwrap())
//!         .env(ENV_VAR, bootstrapper.path())
//!         .spawn()
//!         .unwrap();
//!
//!     let (tx, rx) = channel().unwrap();
//!     bootstrapper.send(Task::Sum(vec![23, 42], tx)).unwrap();
//!     println!("sum: {}", rx.recv().unwrap());
//!     bootstrapper.send(Task::Shutdown).unwrap();
//! }
//! ```
//!
//! # Feature Flags
//!
//! All features are enabled by default but a lot can be turned off to
//! cut down on dependencies.  With all default features enabled only
//! the raw types are available.
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
