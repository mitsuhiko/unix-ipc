use std::fmt;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::path::Path;

use serde_::de::DeserializeOwned;
use serde_::Serialize;

use crate::raw_channel::{raw_channel, RawReceiver, RawSender};
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

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Receiver")
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Sender")
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}

macro_rules! fd_impl {
    ($field:ident, $raw_ty:ty, $ty:ty) => {
        #[allow(dead_code)]
        impl<T> $ty {
            pub(crate) fn extract_raw_fd(&self) -> RawFd {
                self.$field.extract_raw_fd()
            }
        }

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

        impl<T> IntoRawFd for $ty {
            fn into_raw_fd(self) -> RawFd {
                self.$field.into_raw_fd()
            }
        }

        impl<T> AsRawFd for $ty {
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
    let (sender, receiver) = raw_channel()?;
    Ok((sender.into(), receiver.into()))
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
        deserialize::<(T, bool)>(&buf, fds.as_deref().unwrap_or_default()).map(|x| x.0)
    }
}

impl<T: Serialize + DeserializeOwned> Sender<T> {
    /// Converts the typed sender into a raw one.
    pub fn into_raw_sender(self) -> RawSender {
        self.raw_sender
    }

    /// Receives a structured message from the socket.
    pub fn send(&self, s: T) -> io::Result<()> {
        // we always serialize a dummy bool at the end so that the message
        // will not be empty because of zero sized types.
        let (payload, fds) = serialize((&s, true))?;
        self.raw_sender.send(&payload, &fds)?;
        Ok(())
    }
}

#[test]
fn test_basic() {
    use crate::serde::Handle;
    use std::io::Read;

    let f = Handle::from(std::fs::File::open("src/serde.rs").unwrap());

    let (tx, rx) = channel().unwrap();

    let server = std::thread::spawn(move || {
        tx.send(f).unwrap();
    });

    std::thread::sleep(std::time::Duration::from_millis(10));

    let client = std::thread::spawn(move || {
        let f = rx.recv().unwrap();

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

    let (tx, rx) = channel().unwrap();
    let (sender, receiver) = channel::<Handle<File>>().unwrap();

    let server = std::thread::spawn(move || {
        tx.send(sender).unwrap();
        let handle = receiver.recv().unwrap();
        let mut file = handle.into_inner();
        let mut out = Vec::new();
        file.read_to_end(&mut out).unwrap();
        assert!(out.len() > 100);
    });

    std::thread::sleep(std::time::Duration::from_millis(10));

    let client = std::thread::spawn(move || {
        let sender = rx.recv().unwrap();
        sender
            .send(Handle::from(File::open("src/serde.rs").unwrap()))
            .unwrap();
    });

    server.join().unwrap();
    client.join().unwrap();
}

#[test]
fn test_multiple_fds() {
    let (tx1, rx1) = channel().unwrap();
    let (tx2, rx2) = channel::<()>().unwrap();
    let (tx3, rx3) = channel::<()>().unwrap();

    let a = std::thread::spawn(move || {
        tx1.send((tx2, rx2, tx3, rx3)).unwrap();
    });

    let b = std::thread::spawn(move || {
        let _channels = rx1.recv().unwrap();
    });

    a.join().unwrap();
    b.join().unwrap();
}

#[test]
fn test_conversion() {
    let (tx, rx) = channel::<i32>().unwrap();
    let raw_tx = tx.into_raw_sender();
    let raw_rx = rx.into_raw_receiver();
    let tx = Sender::<bool>::from(raw_tx);
    let rx = Receiver::<bool>::from(raw_rx);

    let a = std::thread::spawn(move || {
        tx.send(true).unwrap();
    });

    let b = std::thread::spawn(move || {
        assert_eq!(rx.recv().unwrap(), true);
    });

    a.join().unwrap();
    b.join().unwrap();
}

#[test]
fn test_zero_sized_type() {
    let (tx, rx) = channel::<()>().unwrap();

    let a = std::thread::spawn(move || {
        tx.send(()).unwrap();
    });

    let b = std::thread::spawn(move || {
        rx.recv().unwrap();
    });

    a.join().unwrap();
    b.join().unwrap();
}

#[test]
fn test_many_nested() {
    for _ in 0..2000 {
        let (tx, rx) = channel().unwrap();
        let (tx2, rx2) = channel().unwrap();

        tx.send(tx2).unwrap();

        let recv = rx.recv().unwrap();

        recv.send(1).unwrap();

        rx2.recv().unwrap();
    }
}
