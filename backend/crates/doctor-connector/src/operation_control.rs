//! Per-Audit-batch cancellation. Socket clones never perform protocol IO.
use crate::{ConnectorError, ConnectorErrorCode};
use std::{
    net::{Shutdown, TcpStream},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
/// Wake protocol IO blocked on another handle of the same socket.
///
/// `shutdown` makes every later recv/send fail at once. On Linux it also wakes
/// a recv already blocked in the kernel; on Windows it does not (SD_RECEIVE only
/// rejects new calls, SD_SEND only sends FIN), so a blocked recv would wait for
/// the peer to close or for the read timeout. `CancelIoEx` cancels the pending
/// request on the shared socket object from any thread and any handle.
fn interrupt(socket: &TcpStream) {
    let _ = socket.shutdown(Shutdown::Both);
    #[cfg(windows)]
    windows_cancel::cancel_pending_io(socket);
}
#[cfg(windows)]
mod windows_cancel {
    use std::{ffi::c_void, net::TcpStream, os::windows::io::AsRawSocket, ptr};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CancelIoEx(file: *mut c_void, overlapped: *mut c_void) -> i32;
    }
    pub(super) fn cancel_pending_io(socket: &TcpStream) {
        // SAFETY: the borrowed clone keeps its SOCKET handle open for the call;
        // a null OVERLAPPED cancels every pending request on the socket. Failure
        // (nothing pending) is fine: shutdown already rejects any new IO.
        let _ = unsafe { CancelIoEx(socket.as_raw_socket() as *mut c_void, ptr::null_mut()) };
    }
}
pub struct OperationControl {
    deadline: Instant,
    expired: Arc<dyn Fn() -> bool + Send + Sync>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    stopped: bool,
    finished: bool,
    acquired: bool,
    socket: Option<TcpStream>,
}
impl OperationControl {
    pub fn new(remaining: Duration) -> Arc<Self> {
        Self::with_expiry(remaining, Arc::new(|| false))
    }
    pub fn with_expiry(
        remaining: Duration,
        expired: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            deadline: Instant::now() + remaining,
            expired,
            state: Mutex::new(State::default()),
        })
    }
    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.finished {
            state.stopped = true;
            if let Some(socket) = &state.socket {
                interrupt(socket);
            }
        }
    }
    pub fn acquired_session(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .acquired
    }
    pub(crate) fn acquire_session(&self) -> Result<(), ConnectorError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if Instant::now() >= self.deadline || (self.expired)() {
            state.stopped = true;
        }
        if state.stopped {
            return Err(Self::error());
        }
        state.acquired = true;
        Ok(())
    }
    pub fn is_stopped(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stopped
    }
    pub fn check(&self) -> Result<(), ConnectorError> {
        if Instant::now() >= self.deadline || (self.expired)() {
            self.stop();
        }
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped {
            Err(Self::error())
        } else {
            Ok(())
        }
    }
    pub fn error() -> ConnectorError {
        ConnectorError::new(ConnectorErrorCode::RemoteTimeout, "audit_operation")
    }
    pub(crate) fn bind(&self, socket: TcpStream) -> Result<(), ConnectorError> {
        self.check()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped {
            interrupt(&socket);
            return Err(Self::error());
        }
        state.socket = Some(socket);
        Ok(())
    }
    /// Pure in-memory publication only: serialize the CAS against stop, never IO.
    pub(crate) fn publish<T>(
        &self,
        publish: impl FnOnce() -> Result<T, ConnectorError>,
    ) -> Result<T, ConnectorError> {
        self.check()?;
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped || (Instant::now() >= self.deadline || (self.expired)()) {
            return Err(Self::error());
        }
        let result = publish();
        drop(state);
        result
    }
    pub(crate) fn unbind(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).socket = None;
    }
    /// Linearize completion against a late timer/drop notification.
    pub fn finish(&self) -> Result<(), ConnectorError> {
        self.check()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if Instant::now() >= self.deadline || (self.expired)() {
            state.stopped = true;
            if let Some(socket) = &state.socket {
                interrupt(socket);
            }
        }
        if state.stopped {
            return Err(Self::error());
        }
        state.finished = true;
        state.socket = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, net::TcpListener, sync::Barrier, thread};
    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        (client, server)
    }
    #[test]
    fn stop_bind_race_always_prevents_io_or_shuts_down() {
        for _ in 0..32 {
            let control = OperationControl::new(Duration::from_secs(5));
            let (client, mut server) = pair();
            let barrier = Arc::new(Barrier::new(2));
            let peer = Arc::clone(&barrier);
            let stopping = Arc::clone(&control);
            let thread = thread::spawn(move || {
                peer.wait();
                stopping.stop();
            });
            barrier.wait();
            let _ = control.bind(client);
            thread.join().unwrap();
            assert!(control.check().is_err());
            // A rejected binding owns/drops its only client socket, or stop shuts it down.
            assert_eq!(server.read(&mut [0]).unwrap(), 0);
        }
    }
    #[test]
    fn stop_wakes_recv_blocked_on_another_handle_while_peer_stays_silent() {
        for _ in 0..8 {
            let control = OperationControl::new(Duration::from_secs(30));
            let (mut client, _silent_server) = pair();
            // Far above the wait below: only `stop()` may end the blocked recv.
            client
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            control.bind(client.try_clone().unwrap()).unwrap();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let worker = thread::spawn(move || {
                let _ = client.read(&mut [0; 16]);
                done_tx.send(()).unwrap();
            });
            // Let the worker enter the kernel recv; a stop before it is also covered.
            thread::sleep(Duration::from_millis(20));
            control.stop();
            done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            worker.join().unwrap();
        }
    }
    #[test]
    fn stopped_control_never_publishes_and_late_stop_does_not_touch_reused_socket() {
        let control = OperationControl::new(Duration::from_secs(5));
        control.stop();
        assert!(
            control
                .publish(|| -> Result<(), ConnectorError> { panic!("published after stop") })
                .is_err()
        );
        let completed = OperationControl::new(Duration::from_secs(5));
        let (client, mut server) = pair();
        completed.bind(client.try_clone().unwrap()).unwrap();
        completed.unbind();
        completed.finish().unwrap();
        completed.stop();
        use std::io::Write;
        (&client).write_all(b"x").unwrap();
        let mut byte = [0];
        server.read_exact(&mut byte).unwrap();
        assert_eq!(byte, *b"x");
    }
    #[test]
    fn finish_rechecks_expiry_under_its_completion_lock() {
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe = Arc::clone(&checks);
        let control = OperationControl::with_expiry(
            Duration::from_secs(5),
            Arc::new(move || probe.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0),
        );
        assert!(control.finish().is_err());
        assert!(control.is_stopped());
    }
}
