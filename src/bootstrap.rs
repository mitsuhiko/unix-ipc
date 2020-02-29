use std::cell::RefCell;
use std::fs;
use std::io;
use std::os::unix::io::{FromRawFd, IntoRawFd};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::{thread_rng, RngCore};
use serde_::de::DeserializeOwned;
use serde_::Serialize;

use crate::typed_channel::Sender;

/// A bootstrap helper.
#[derive(Debug)]
pub struct Bootstrapper<T> {
    listener: UnixListener,
    sender: RefCell<Option<Sender<T>>>,
    path: PathBuf,
}

impl<T: Serialize + DeserializeOwned> Bootstrapper<T> {
    /// Creates a bootstrapper at a random socket in `/tmp`.
    pub fn new() -> io::Result<Bootstrapper<T>> {
        let mut dir = std::env::temp_dir();
        let mut rng = thread_rng();
        let now = SystemTime::now();
        dir.push(&format!(
            ".rust-unix-ipc.{}-{}.sock",
            now.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            rng.next_u64(),
        ));
        Bootstrapper::bind(&dir)
    }

    /// Creates a bootstrapper at a specific socket path.
    pub fn bind<P: AsRef<Path>>(p: P) -> io::Result<Bootstrapper<T>> {
        fs::remove_file(&p).ok();
        let listener = UnixListener::bind(&p)?;
        Ok(Bootstrapper {
            listener,
            sender: RefCell::new(None),
            path: p.as_ref().to_path_buf(),
        })
    }

    /// Returns the path of the socket.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consumes the boostrapper and sends a single value in.
    ///
    /// This can be called multiple times to send more than one value
    /// into the inner socket.
    pub fn send(&self, val: T) -> io::Result<()> {
        if self.sender.borrow().is_none() {
            let (sock, _) = self.listener.accept()?;
            let sender = unsafe { Sender::from_raw_fd(sock.into_raw_fd()) };
            *self.sender.borrow_mut() = Some(sender);
        }
        self.sender.borrow().as_ref().unwrap().send(val)
    }
}

impl<T> Drop for Bootstrapper<T> {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

#[test]
fn test_bootstrap() {
    use crate::Receiver;

    let bootstrapper = Bootstrapper::new().unwrap();
    let path = bootstrapper.path().to_owned();

    let handle = std::thread::spawn(move || {
        let receiver = Receiver::<u32>::connect(path).unwrap();
        let a = receiver.recv().unwrap();
        let b = receiver.recv().unwrap();
        assert_eq!(a + b, 65);
    });

    bootstrapper.send(42u32).unwrap();
    bootstrapper.send(23u32).unwrap();

    handle.join().unwrap();
}

#[test]
fn test_bootstrap_reverse() {
    use crate::{Receiver, channel};

    let bootstrapper = Bootstrapper::new().unwrap();
    let path = bootstrapper.path().to_owned();
    let (tx, rx) = channel::<u32>().unwrap();

    std::thread::spawn(move || {
        let receiver = Receiver::<Sender<u32>>::connect(path).unwrap();
        let result_sender = receiver.recv().unwrap();
        result_sender.send(42 + 23).unwrap();
    });

    bootstrapper.send(tx).unwrap();
    assert_eq!(rx.recv().unwrap(), 65);
}