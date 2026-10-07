//! A Unix stream socket connected within a time, created with the flags Linux has for it.
use std::{
    io,
    os::{
        fd::FromRawFd,
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
        let fd = libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let stream = UnixStream::from_raw_fd(fd);
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
        let length =
            (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;
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
