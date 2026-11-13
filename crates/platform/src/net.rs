// SPDX-License-Identifier: GPL-3.0-or-later

//! Unix-domain sockets. On Unix these are the `std` types themselves. On
//! Windows they are stand-ins with the same methods whose `bind` and `connect`
//! fail with `Unsupported`: the Windows transport is the IPC port task's work.

#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
pub use self::stub::{UnixListener, UnixStream};

#[cfg(windows)]
mod stub {
    use std::io::{self, Read, Write};
    use std::path::Path;
    use std::time::Duration;

    fn no(what: &str) -> io::Error {
        crate::unsupported(what, "IPC transport")
    }

    /// Stand-in for `std::os::unix::net::UnixStream`. It cannot be created.
    #[derive(Debug)]
    pub struct UnixStream(());

    impl UnixStream {
        pub fn connect(_path: impl AsRef<Path>) -> io::Result<UnixStream> {
            Err(no("connecting to a unix socket"))
        }
        pub fn try_clone(&self) -> io::Result<UnixStream> {
            Err(no("cloning a unix socket"))
        }
        pub fn set_nonblocking(&self, _on: bool) -> io::Result<()> {
            Err(no("a unix socket"))
        }
        pub fn set_read_timeout(&self, _t: Option<Duration>) -> io::Result<()> {
            Err(no("a unix socket"))
        }
        pub fn set_write_timeout(&self, _t: Option<Duration>) -> io::Result<()> {
            Err(no("a unix socket"))
        }
    }

    impl Read for UnixStream {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(no("reading a unix socket"))
        }
    }

    impl Write for UnixStream {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(no("writing a unix socket"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(no("writing a unix socket"))
        }
    }

    /// Stand-in for `std::os::unix::net::UnixListener`. It cannot be created.
    #[derive(Debug)]
    pub struct UnixListener(());

    impl UnixListener {
        pub fn bind(_path: impl AsRef<Path>) -> io::Result<UnixListener> {
            Err(no("listening on a unix socket"))
        }
        pub fn set_nonblocking(&self, _on: bool) -> io::Result<()> {
            Err(no("a unix socket"))
        }
        pub fn accept(&self) -> io::Result<(UnixStream, ())> {
            Err(no("accepting on a unix socket"))
        }
    }
}
