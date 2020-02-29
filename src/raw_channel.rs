use std::fs;
use std::io;
use std::mem;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::slice;

use nix::sys::socket::{
    c_uint, recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags, CMSG_SPACE,
};
use nix::sys::uio::IoVec;
use nix::unistd;

/// A raw receiver.
#[derive(Debug)]
pub struct RawReceiver {
    fd: RawFd,
}

/// A raw sender.
#[derive(Debug)]
pub struct RawSender {
    fd: RawFd,
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
#[derive(Default)]
struct MsgHeader {
    payload_len: u32,
    fd_count: u32,
}

macro_rules! fd_impl {
    ($ty:ty) => {
        impl FromRawFd for $ty {
            unsafe fn from_raw_fd(fd: RawFd) -> Self {
                Self { fd }
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
    };
}

fd_impl!(RawReceiver);
fd_impl!(RawSender);

fn nix_as_io_error(err: nix::Error) -> io::Error {
    if let Some(errno) = err.as_errno() {
        io::Error::from(errno)
    } else {
        io::Error::new(io::ErrorKind::Other, err.to_string())
    }
}

impl RawReceiver {
    /// Connects a receiver to a named unix socket.
    pub fn connect<P: AsRef<Path>>(p: P) -> io::Result<RawReceiver> {
        let sock = UnixStream::connect(p)?;
        unsafe { Ok(RawReceiver::from_raw_fd(sock.into_raw_fd())) }
    }

    /// Receives raw bytes from the socket.
    pub fn recv(&self) -> io::Result<(Vec<u8>, Option<Vec<RawFd>>)> {
        let mut header = MsgHeader::default();
        self.recv_impl(
            unsafe {
                slice::from_raw_parts_mut(
                    (&mut header as *mut _) as *mut u8,
                    mem::size_of_val(&header),
                )
            },
            0,
        )?;

        let mut buf = vec![0u8; header.payload_len as usize];
        let (_, fds) = self.recv_impl(&mut buf, header.fd_count as usize)?;
        Ok((buf, fds))
    }

    fn recv_impl(
        &self,
        buf: &mut [u8],
        fd_count: usize,
    ) -> io::Result<(usize, Option<Vec<RawFd>>)> {
        let mut pos = 0;
        let mut fds = None;

        loop {
            let iov = [IoVec::from_mut_slice(&mut buf[pos..])];
            let mut new_fds = None;
            let msgspace_size =
                unsafe { CMSG_SPACE(mem::size_of::<RawFd>() as c_uint) * fd_count as u32 };
            let mut cmsgspace = vec![0u8; msgspace_size as usize];

            let msg = recvmsg(self.fd, &iov, Some(&mut cmsgspace), MsgFlags::empty())
                .map_err(nix_as_io_error)?;

            for cmsg in msg.cmsgs() {
                if let ControlMessageOwned::ScmRights(fds) = cmsg {
                    if !fds.is_empty() {
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

            pos += msg.bytes;
            if pos >= buf.len() {
                return Ok((pos, fds));
            }
        }
    }
}

impl RawSender {
    /// Opens a new unix socket and block until someone connects.
    pub fn bind_and_accept<P: AsRef<Path>>(p: P) -> io::Result<RawSender> {
        fs::remove_file(&p).ok();
        let listener = UnixListener::bind(&p)?;
        let (sock, _) = listener.accept()?;
        let sender = unsafe { RawSender::from_raw_fd(sock.into_raw_fd()) };
        Ok(sender)
    }

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
            if !fds.is_empty() {
                pos += sendmsg(
                    self.fd,
                    &iov,
                    &[ControlMessage::ScmRights(fds)],
                    MsgFlags::empty(),
                    None,
                )
                .map_err(nix_as_io_error)?;
                fds = &[][..];
            } else {
                pos += sendmsg(self.fd, &iov, &[], MsgFlags::empty(), None)
                    .map_err(nix_as_io_error)?;
            }
            if pos >= data.len() {
                return Ok(pos);
            }
        }
    }
}

impl Drop for RawReceiver {
    fn drop(&mut self) {
        unistd::close(self.fd).ok();
    }
}

#[test]
fn test_basic() {
    let path = "/tmp/unix-ipc-test-socket-raw.sock";

    let server = std::thread::spawn(move || {
        let sender = RawSender::bind_and_accept(path).unwrap();
        sender.send(b"Hello World!", &[][..]).unwrap();
    });

    std::thread::sleep(std::time::Duration::from_millis(10));

    let client = std::thread::spawn(move || {
        let c = RawReceiver::connect(path).unwrap();
        let (bytes, fds) = c.recv().unwrap();
        assert_eq!(bytes, b"Hello World!");
        assert_eq!(fds, None);
    });

    server.join().unwrap();
    client.join().unwrap();
}

#[test]
fn test_large_buffer() {
    use std::fmt::Write;

    let path = "/tmp/unix-ipc-test-socket-large-buf.sock";
    let mut buf = String::new();
    for x in 0..10000 {
        write!(&mut buf, "{}", x).ok();
    }

    let server_buf = buf.clone();
    let server = std::thread::spawn(move || {
        let sender = RawSender::bind_and_accept(path).unwrap();
        sender.send(server_buf.as_bytes(), &[][..]).unwrap();
    });

    std::thread::sleep(std::time::Duration::from_millis(10));

    let client = std::thread::spawn(move || {
        let c = RawReceiver::connect(path).unwrap();
        let (bytes, fds) = c.recv().unwrap();
        assert_eq!(bytes, buf.as_bytes());
        assert_eq!(fds, None);
    });

    server.join().unwrap();
    client.join().unwrap();
}
