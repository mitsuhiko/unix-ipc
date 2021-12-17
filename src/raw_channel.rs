use std::io;
use std::mem;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};

use nix::sys::socket::{
    c_uint, recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags, CMSG_SPACE,
};
use nix::sys::uio::IoVec;
use nix::unistd;

#[cfg(target_os = "linux")]
const MSG_FLAGS: MsgFlags = MsgFlags::MSG_CMSG_CLOEXEC;

#[cfg(target_os = "macos")]
const MSG_FLAGS: MsgFlags = MsgFlags::empty();

/// A raw receiver.
#[derive(Debug)]
pub struct RawReceiver {
    fd: RawFd,
    dead: AtomicBool,
}

/// A raw sender.
#[derive(Debug)]
pub struct RawSender {
    fd: RawFd,
    dead: AtomicBool,
}

/// Creates a raw connected channel.
pub fn raw_channel() -> io::Result<(RawSender, RawReceiver)> {
    let (sender, receiver) = UnixStream::pair()?;
    unsafe {
        Ok((
            RawSender::from_raw_fd(sender.into_raw_fd()),
            RawReceiver::from_raw_fd(receiver.into_raw_fd()),
        ))
    }
}

#[repr(C)]
#[derive(Default, Debug)]
struct MsgHeader {
    payload_len: u32,
    fd_count: u32,
}

macro_rules! fd_impl {
    ($ty:ty) => {
        #[allow(dead_code)]
        impl $ty {
            pub(crate) fn extract_raw_fd(&self) -> RawFd {
                if self.dead.swap(true, Ordering::SeqCst) {
                    panic!("handle was moved previously");
                } else {
                    self.fd
                }
            }
        }

        impl FromRawFd for $ty {
            unsafe fn from_raw_fd(fd: RawFd) -> Self {
                Self {
                    fd,
                    dead: AtomicBool::new(false),
                }
            }
        }

        impl IntoRawFd for $ty {
            fn into_raw_fd(self) -> RawFd {
                let fd = self.fd;
                mem::forget(self);
                fd
            }
        }

        impl AsRawFd for $ty {
            fn as_raw_fd(&self) -> RawFd {
                self.fd
            }
        }

        impl Drop for $ty {
            fn drop(&mut self) {
                unistd::close(self.fd).ok();
            }
        }
    };
}

fd_impl!(RawReceiver);
fd_impl!(RawSender);

impl RawReceiver {
    /// Connects a receiver to a named unix socket.
    pub fn connect<P: AsRef<Path>>(p: P) -> io::Result<RawReceiver> {
        let sock = UnixStream::connect(p)?;
        unsafe { Ok(RawReceiver::from_raw_fd(sock.into_raw_fd())) }
    }

    pub fn recv(&self) -> io::Result<(Vec<u8>, Option<Vec<RawFd>>)> {
        self.recv_impl(true)
    }

    pub fn try_recv(&self) -> io::Result<Option<(Vec<u8>, Option<Vec<RawFd>>)>> {
        let res = self.recv_impl(false);

        match res {
            Ok(res) => Ok(Some(res)),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Receives raw bytes from the socket.
    fn recv_impl(&self, blocking: bool) -> io::Result<(Vec<u8>, Option<Vec<RawFd>>)> {
        let mut header = MsgHeader::default();
        self.recv_part(
            unsafe {
                slice::from_raw_parts_mut(
                    (&mut header as *mut _) as *mut u8,
                    mem::size_of_val(&header),
                )
            },
            0,
            blocking,
        )?;

        let mut buf = vec![0u8; header.payload_len as usize];
        // Once the header is received, the body must always follow
        let (_, fds) = self.recv_part(&mut buf, header.fd_count as usize, true)?;
        Ok((buf, fds))
    }

    fn recv_part(
        &self,
        buf: &mut [u8],
        fd_count: usize,
        blocking: bool,
    ) -> io::Result<(usize, Option<Vec<RawFd>>)> {
        let mut pos = 0;
        let mut fds = None;

        loop {
            let iov = [IoVec::from_mut_slice(&mut buf[pos..])];
            let mut new_fds = None;
            let msgspace_size =
                unsafe { CMSG_SPACE(mem::size_of::<RawFd>() as c_uint) * fd_count as u32 };
            let mut cmsgspace = vec![0u8; msgspace_size as usize];

            let flags = if blocking {
                MSG_FLAGS
            } else {
                MSG_FLAGS | MsgFlags::MSG_DONTWAIT
            };

            let msg = recvmsg(self.fd, &iov, Some(&mut cmsgspace), flags)?;

            for cmsg in msg.cmsgs() {
                if let ControlMessageOwned::ScmRights(fds) = cmsg {
                    if !fds.is_empty() {
                        #[cfg(target_os = "macos")]
                        unsafe {
                            for &fd in &fds {
                                libc::ioctl(fd, libc::FIOCLEX);
                            }
                        }
                        new_fds = Some(fds);
                    }
                }
            }

            fds = match (fds, new_fds) {
                (None, Some(new)) => Some(new),
                (Some(mut old), Some(new)) => {
                    old.extend(new);
                    Some(old)
                }
                (old, None) => old,
            };

            if msg.bytes == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "could not read",
                ));
            }

            pos += msg.bytes;
            if pos >= buf.len() {
                return Ok((pos, fds));
            }
        }
    }
}

impl RawSender {
    /// Sends raw bytes and fds.
    pub fn send(&self, data: &[u8], fds: &[RawFd]) -> io::Result<usize> {
        let header = MsgHeader {
            payload_len: data.len() as u32,
            fd_count: fds.len() as u32,
        };
        let header_slice = unsafe {
            slice::from_raw_parts(
                (&header as *const _) as *const u8,
                mem::size_of_val(&header),
            )
        };

        self.send_impl(&header_slice, &[][..])?;
        self.send_impl(&data, fds)
    }

    fn send_impl(&self, data: &[u8], mut fds: &[RawFd]) -> io::Result<usize> {
        let mut pos = 0;
        loop {
            let iov = [IoVec::from_slice(&data[pos..])];
            let sent = if !fds.is_empty() {
                sendmsg(
                    self.fd,
                    &iov,
                    &[ControlMessage::ScmRights(fds)],
                    MsgFlags::empty(),
                    None,
                )?
            } else {
                sendmsg(self.fd, &iov, &[], MsgFlags::empty(), None)?
            };
            if sent == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "could not send"));
            }
            pos += sent;
            fds = &[][..];
            if pos >= data.len() {
                return Ok(pos);
            }
        }
    }
}

#[test]
fn test_basic() {
    let (tx, rx) = raw_channel().unwrap();

    let server = std::thread::spawn(move || {
        tx.send(b"Hello World!", &[][..]).unwrap();
    });

    std::thread::sleep(std::time::Duration::from_millis(10));

    let client = std::thread::spawn(move || {
        let (bytes, fds) = rx.recv().unwrap();
        assert_eq!(bytes, b"Hello World!");
        assert_eq!(fds, None);
    });

    server.join().unwrap();
    client.join().unwrap();
}

#[test]
fn test_large_buffer() {
    use std::fmt::Write;

    let mut buf = String::new();
    for x in 0..10000 {
        write!(&mut buf, "{}", x).ok();
    }

    let (tx, rx) = raw_channel().unwrap();

    let server_buf = buf.clone();
    let server = std::thread::spawn(move || {
        tx.send(server_buf.as_bytes(), &[][..]).unwrap();
    });

    std::thread::sleep(std::time::Duration::from_millis(10));

    let client = std::thread::spawn(move || {
        let (bytes, fds) = rx.recv().unwrap();
        assert_eq!(bytes, buf.as_bytes());
        assert_eq!(fds, None);
    });

    server.join().unwrap();
    client.join().unwrap();
}
