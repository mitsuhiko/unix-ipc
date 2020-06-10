use std::cell::RefCell;
use std::io;
use std::mem;
use std::os::unix::io::{FromRawFd, IntoRawFd, RawFd};
use std::sync::Mutex;

use serde_::{de, ser};
use serde_::{de::DeserializeOwned, Deserialize, Serialize};

thread_local! {
    static IPC_FDS: RefCell<Vec<Vec<RawFd>>> = RefCell::new(Vec::new());
}

pub trait Format {
    fn serialize<S: Serialize>(s: S) -> io::Result<(Vec<u8>, Vec<RawFd>)>;
    fn deserialize<D: DeserializeOwned>(bytes: &[u8], fds: &[RawFd]) -> io::Result<D>;
}

pub struct Bincode;

pub struct Cbor;

/// Can transfer a unix file handle across processes.
///
/// The basic requirement is that you have an object that can be converted
/// into a raw file handle and back.  This for instance is the case for
/// regular file objects, sockets and many more things.
///
/// Once the handle has been serialized the handle no longer lets you
/// extract the value contained in it.
///
/// For customizing the serialization of libraries the
/// [`HandleRef`](struct.HandleRef.html) object should be used instead.
pub struct Handle<F>(Mutex<Option<F>>);

/// A raw reference to a handle.
///
/// This serializes the same way as a `Handle` but only uses a raw
/// fd to represent it.  Useful to implement custom serializers.
pub struct HandleRef(pub RawFd);

impl<F: FromRawFd + IntoRawFd> Handle<F> {
    /// Wraps the value in a handle.
    pub fn new(f: F) -> Self {
        f.into()
    }

    fn extract_raw_fd(&self) -> RawFd {
        self.0
            .lock()
            .unwrap()
            .take()
            .map(|x| x.into_raw_fd())
            .expect("cannot serialize handle twice")
    }

    /// Extracts the internal value.
    pub fn into_inner(self) -> F {
        self.0.lock().unwrap().take().expect("handle was moved")
    }
}

impl<F: FromRawFd + IntoRawFd> From<F> for Handle<F> {
    fn from(f: F) -> Self {
        Handle(Mutex::new(Some(f)))
    }
}

impl Serialize for HandleRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        if serde_in_ipc_mode() {
            let fd = self.0;
            let idx = register_fd(fd);
            idx.serialize(serializer)
        } else {
            Err(ser::Error::custom("can only serialize in ipc mode"))
        }
    }
}

impl<F: FromRawFd + IntoRawFd> Serialize for Handle<F> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        HandleRef(self.extract_raw_fd()).serialize(serializer)
    }
}

impl<'de, F: FromRawFd + IntoRawFd> Deserialize<'de> for Handle<F> {
    fn deserialize<D>(deserializer: D) -> Result<Handle<F>, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        if serde_in_ipc_mode() {
            let idx = u32::deserialize(deserializer)?;
            let fd = lookup_fd(idx).ok_or_else(|| de::Error::custom("fd not found in mapping"))?;
            unsafe { Ok(Handle(Mutex::new(Some(FromRawFd::from_raw_fd(fd))))) }
        } else {
            Err(de::Error::custom("can only deserialize in ipc mode"))
        }
    }
}

struct ResetIpcSerde;

impl Drop for ResetIpcSerde {
    fn drop(&mut self) {
        IPC_FDS.with(|x| x.borrow_mut().pop());
    }
}

fn enter_ipc_mode<F: FnOnce() -> R, R>(f: F, fds: &mut Vec<RawFd>) -> R {
    IPC_FDS.with(|x| x.borrow_mut().push(fds.clone()));
    let reset = ResetIpcSerde;
    let rv = f();
    *fds = IPC_FDS.with(|x| x.borrow_mut().pop()).unwrap_or_default();
    mem::forget(reset);
    rv
}

fn register_fd(fd: RawFd) -> u32 {
    IPC_FDS.with(|x| {
        let mut x = x.borrow_mut();
        let fds = x.last_mut().unwrap();
        let rv = fds.len() as u32;
        fds.push(fd);
        rv
    })
}

fn lookup_fd(idx: u32) -> Option<RawFd> {
    IPC_FDS.with(|x| x.borrow().last().and_then(|l| l.get(idx as usize).copied()))
}

/// Checks if serde is in IPC mode.
///
/// This can be used to customize the behavior of serialization/deserialization
/// implementations for the use with unix-ipc.
pub fn serde_in_ipc_mode() -> bool {
    IPC_FDS.with(|x| !x.borrow().is_empty())
}

#[cfg(feature = "bincode")]
impl Format for Bincode {
    /// Serializes something for IPC communication.
    ///
    /// This uses bincode for serialization.  Because UNIX sockets require that
    /// file descriptors are transmitted separately they are accumulated in a
    /// separate buffer.
    fn serialize<S: Serialize>(s: S) -> io::Result<(Vec<u8>, Vec<RawFd>)> {
        let mut fds = Vec::new();
        let mut out = Vec::new();
        enter_ipc_mode(|| bincode::serialize_into(&mut out, &s), &mut fds)
            .map_err(bincode_to_io_error)?;
        Ok((out, fds))
    }

    /// Deserializes something for IPC communication.
    ///
    /// File descriptors need to be provided for deserialization if handles are
    /// involved.
    fn deserialize<D: DeserializeOwned>(bytes: &[u8], fds: &[RawFd]) -> io::Result<D> {
        let mut fds = fds.to_owned();
        let result = enter_ipc_mode(|| bincode::deserialize(bytes), &mut fds)
            .map_err(bincode_to_io_error)?;
        Ok(result)
    }
}

#[cfg(feature = "bincode")]
#[allow(clippy::boxed_local)]
fn bincode_to_io_error(err: bincode::Error) -> io::Error {
    match *err {
        bincode::ErrorKind::Io(err) => err,
        err => io::Error::new(io::ErrorKind::Other, err.to_string()),
    }
}

#[cfg(feature = "cbor")]
impl Format for Cbor {
    /// Serializes something for IPC communication.
    ///
    /// This uses CBOR for serialization.  Because UNIX sockets require that
    /// file descriptors are transmitted separately they are accumulated in a
    /// separate buffer.
    fn serialize<S: Serialize>(s: S) -> io::Result<(Vec<u8>, Vec<RawFd>)> {
        let mut fds = Vec::new();
        let mut out = Vec::new();
        enter_ipc_mode(|| serde_cbor::to_writer(&mut out, &s), &mut fds)
            .map_err(cbor_to_io_error)?;
        Ok((out, fds))
    }

    /// Deserializes something for IPC communication.
    ///
    /// File descriptors need to be provided for deserialization if handles are
    /// involved.
    fn deserialize<D: DeserializeOwned>(bytes: &[u8], fds: &[RawFd]) -> io::Result<D> {
        let mut fds = fds.to_owned();
        let result =
            enter_ipc_mode(|| serde_cbor::from_slice(bytes), &mut fds).map_err(cbor_to_io_error)?;
        Ok(result)
    }
}

#[cfg(feature = "cbor")]
fn cbor_to_io_error(err: serde_cbor::Error) -> io::Error {
    // cbor doesn't provide a way to move the io::Error out of its wrapper.
    io::Error::new(io::ErrorKind::Other, err.to_string())
}

macro_rules! implement_handle_serialization {
    ($ty:ty) => {
        impl $crate::_serde_ref::Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: $crate::_serde_ref::ser::Serializer,
            {
                $crate::_serde_ref::Serialize::serialize(
                    &$crate::HandleRef(self.extract_raw_fd()),
                    serializer,
                )
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<$ty, D::Error>
            where
                D: $crate::_serde_ref::de::Deserializer<'de>,
            {
                let handle: $crate::Handle<$ty> =
                    $crate::_serde_ref::Deserialize::deserialize(deserializer)?;
                Ok(handle.into_inner())
            }
        }
    };
}

implement_handle_serialization!(crate::RawSender);
implement_handle_serialization!(crate::RawReceiver);

macro_rules! implement_typed_handle_serialization {
    ($ty:ty) => {
        impl<F, T: Serialize + DeserializeOwned> $crate::_serde_ref::Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: $crate::_serde_ref::ser::Serializer,
            {
                $crate::_serde_ref::Serialize::serialize(
                    &$crate::HandleRef(self.extract_raw_fd()),
                    serializer,
                )
            }
        }
        impl<'de, F, T: Serialize + DeserializeOwned> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<$ty, D::Error>
            where
                D: $crate::_serde_ref::de::Deserializer<'de>,
            {
                let handle: $crate::Handle<$ty> =
                    $crate::_serde_ref::Deserialize::deserialize(deserializer)?;
                Ok(handle.into_inner())
            }
        }
    };
}

implement_typed_handle_serialization!(crate::Sender<F, T>);
implement_typed_handle_serialization!(crate::Receiver<F, T>);

#[cfg(feature = "bincode")]
#[test]
fn test_basic_bincode() {
    use std::io::Read;
    let f = std::fs::File::open("src/serde.rs").unwrap();
    let handle = Handle::from(f);
    let (bytes, fds) = Bincode::serialize(handle).unwrap();
    let f2: Handle<std::fs::File> = Bincode::deserialize(&bytes, &fds).unwrap();
    let mut out = Vec::new();
    f2.into_inner().read_to_end(&mut out).unwrap();
    assert!(out.len() > 100);
}

#[cfg(feature = "cbor")]
#[test]
fn test_basic_cbor() {
    use std::io::Read;
    let f = std::fs::File::open("src/serde.rs").unwrap();
    let handle = Handle::from(f);
    let (bytes, fds) = Cbor::serialize(handle).unwrap();
    let f2: Handle<std::fs::File> = Cbor::deserialize(&bytes, &fds).unwrap();
    let mut out = Vec::new();
    f2.into_inner().read_to_end(&mut out).unwrap();
    assert!(out.len() > 100);
}
