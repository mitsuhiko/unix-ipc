use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;

use serde_::de::DeserializeOwned;
use serde_::Serialize;

use crate::raw_channel::{RawReceiver, RawSender};
use crate::serde::{deserialize, serialize};

/// A typed receiver.
pub struct Receiver<T> {
    raw_receiver: RawReceiver,
    _marker: std::marker::PhantomData<T>,
}

/// A typed sender.
pub struct Sender<T> {
    raw_sender: RawSender,
    _marker: std::marker::PhantomData<T>,
}

macro_rules! fd_impl {
    ($field:ident, $raw_ty:ty, $ty:ty) => {
        impl<T: Serialize + DeserializeOwned> From<$raw_ty> for $ty {
            fn from(value: $raw_ty) -> Self {
                Self {
                    $field: value,
                    _marker: std::marker::PhantomData,
                }
            }
        }

        impl<T: Serialize + DeserializeOwned> FromRawFd for $ty {
            unsafe fn from_raw_fd(fd: RawFd) -> Self {
                Self {
                    $field: FromRawFd::from_raw_fd(fd),
                    _marker: std::marker::PhantomData,
                }
            }
        }

        impl<T: Serialize + DeserializeOwned> IntoRawFd for $ty {
            fn into_raw_fd(self) -> RawFd {
                self.$field.into_raw_fd()
            }
        }

        impl<T: Serialize + DeserializeOwned> AsRawFd for $ty {
            fn as_raw_fd(&self) -> RawFd {
                self.$field.as_raw_fd()
            }
        }
    };
}

fd_impl!(raw_receiver, RawReceiver, Receiver<T>);
fd_impl!(raw_sender, RawSender, Sender<T>);

/// Creates a typed connected channel.
pub fn channel<T: Serialize + DeserializeOwned>() -> io::Result<(Sender<T>, Receiver<T>)> {
    let (sender, receiver) = UnixStream::pair()?;
    unsafe {
        Ok((
            Sender::from_raw_fd(sender.into_raw_fd()),
            Receiver::from_raw_fd(receiver.into_raw_fd()),
        ))
    }
}

impl<T: Serialize + DeserializeOwned> Receiver<T> {
    /// Connects a receiver to a named unix socket.
    pub fn connect<P: AsRef<Path>>(p: P) -> io::Result<Receiver<T>> {
        RawReceiver::connect(p).map(Into::into)
    }

    /// Converts the typed receiver into a raw one.
    pub fn into_raw_receiver(self) -> RawReceiver {
        self.raw_receiver
    }

    /// Receives a structured message from the socket.
    pub fn recv(&self) -> io::Result<T> {
        let (buf, fds) = self.raw_receiver.recv()?;
        deserialize(&buf, fds.as_deref().unwrap_or_default())
    }
}

impl<T: Serialize + DeserializeOwned> Sender<T> {
    /// Opens a new unix socket and block until someone connects.
    pub fn bind_and_accept<P: AsRef<Path>>(p: P) -> io::Result<Sender<T>> {
        RawSender::bind_and_accept(p).map(Into::into)
    }

    /// Converts the typed sender into a raw one.
    pub fn into_raw_sender(self) -> RawSender {
        self.raw_sender
    }

    /// Receives a structured message from the socket.
    pub fn send(&self, s: T) -> io::Result<()> {
        let (payload, fds) = serialize(s)?;
        self.raw_sender.send(&payload, &fds)?;
        Ok(())
    }
}

#[test]
fn test_basic() {
    use crate::serde::Handle;
    use std::io::Read;

    let f = Handle::from(std::fs::File::open("src/serde.rs").unwrap());
    let path = "/tmp/unix-ipc-test-socket.sock";

    let server = std::thread::spawn(move || {
        let sender = Sender::bind_and_accept(path).unwrap();
        sender.send(f).unwrap();
    });

    std::thread::sleep(std::time::Duration::from_millis(10));

    let client = std::thread::spawn(move || {
        let c = Receiver::<Handle<std::fs::File>>::connect(path).unwrap();
        let f = c.recv().unwrap();

        let mut out = Vec::new();
        f.into_inner().read_to_end(&mut out).unwrap();
        assert!(out.len() > 100);
    });

    server.join().unwrap();
    client.join().unwrap();
}

#[test]
fn test_send_channel() {
    use crate::serde::Handle;
    use std::fs::File;
    use std::io::Read;

    let path = "/tmp/unix-ipc-test-socket-channel.sock";

    let (sender, receiver) = channel::<Handle<File>>().unwrap();

    let server = std::thread::spawn(move || {
        let c = Sender::bind_and_accept(path).unwrap();
        c.send(sender).unwrap();
        let handle = receiver.recv().unwrap();
        let mut file = handle.into_inner();
        let mut out = Vec::new();
        file.read_to_end(&mut out).unwrap();
        assert!(out.len() > 100);
    });

    std::thread::sleep(std::time::Duration::from_millis(10));

    let client = std::thread::spawn(move || {
        let c = Receiver::<Sender<Handle<File>>>::connect(path).unwrap();
        let sender = c.recv().unwrap();
        sender
            .send(Handle::from(File::open("src/serde.rs").unwrap()))
            .unwrap();
    });

    server.join().unwrap();
    client.join().unwrap();
}
