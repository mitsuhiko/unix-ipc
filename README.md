# unix-ipc

This crate implements a minimal abstraction over UNIX domain sockets for
the purpose of IPC.  It lets you send both file handles and rust objects
between processes.

## Feature Flags

* `serde`: enables serialization and deserialization.
* `bootstrap`: adds the `Bootstrapper` type.
* `bootstrap-simple`: adds the default `new` constructor to the
  bootstrapper.
