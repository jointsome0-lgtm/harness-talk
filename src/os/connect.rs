//! A Unix stream socket connected within a time.
use std::{
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
    time::Duration,
};

/// Connect a Unix stream socket within `timeout`, like Python's `settimeout` then `connect`.
pub fn connect_unix(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: plain socket syscalls on a descriptor owned by the returned UnixStream.
    unsafe {
        let stream = socket()?;
        let fd = stream.as_raw_fd();
        let mut address: libc::sockaddr_un = std::mem::zeroed();
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        if bytes.len() >= address.sun_path.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AF_UNIX path too long",
            ));
        }
        for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as libc::c_char;
        }
        let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1)
            as libc::socklen_t;
        if libc::connect(fd, (&raw const address).cast(), length) != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
            let mut poll = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            let milliseconds = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
            match libc::poll(&mut poll, 1, milliseconds) {
                0 => return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT)),
                n if n < 0 => return Err(io::Error::last_os_error()),
                _ => (),
            }
            let mut status: libc::c_int = 0;
            let mut size = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            if libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut status).cast(),
                &mut size,
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status));
            }
        }
        stream.set_nonblocking(false)?;
        Ok(stream)
    }
}

/// A stream socket that does not block and that a child does not inherit. Linux makes it so
/// in one call.
#[cfg(target_os = "linux")]
fn socket() -> io::Result<UnixStream> {
    let kind = libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK;
    let fd = unsafe { libc::socket(libc::AF_UNIX, kind, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { UnixStream::from_raw_fd(fd) })
}
/// macOS has no such call, and sets both afterwards as its standard library does. A child that
/// another thread starts in between inherits the socket.
#[cfg(not(target_os = "linux"))]
fn socket() -> io::Result<UnixStream> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    stream.set_nonblocking(true)?;
    Ok(stream)
}
