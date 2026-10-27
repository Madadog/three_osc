//! Keeps the editor's windows from outliving the host's on X11.
//!
//! There each window the editor opens runs on a thread of its own (baseview's), which the host
//! can only ask to close: the thread goes on drawing until it notices, while the host may
//! already have destroyed the window it was in, or opened the editor again. Drawing into a
//! destroyed window fails in the X server (and baseview panics on it, which can leave later
//! windows unable to draw at all), and the threads of two windows at once trip over each
//! other's X error handlers, which can leave the new window black or take the host down. So closing the editor waits until its window's thread has finished, and opening it
//! waits for any window before (each for a frame or two, at most a second). And as some hosts
//! destroy their window before closing the editor, each window's thread watches the host's
//! window, and ends itself (before drawing another frame) once it's gone, see
//! [`end_if_orphaned()`]. On Windows and macOS the windows run on the host's own GUI thread,
//! and close when they're told to.

use std::any::Any;
use std::cell::RefCell;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use nih_plug::prelude::*;

/// Whether the editor's windows run on threads of their own, as baseview's do on X11.
const SEPARATE_THREADS: bool = cfg!(all(unix, not(target_os = "macos")));

/// The longest a window's thread is waited for: far longer than it takes to finish, but short
/// enough not to hang the host if the thread is stuck (say, on a call to the host).
const TIMEOUT: Duration = Duration::from_secs(1);

/// An editor whose windows have closed by the time the host is told they have (see the
/// [module documentation][self]). Its windows let it know they've opened with
/// [`WindowThreads::opened()`].
pub struct WindowedEditor {
    inner: Box<dyn Editor>,
    threads: Arc<WindowThreads>,
}

impl WindowedEditor {
    pub fn new(inner: Box<dyn Editor>, threads: Arc<WindowThreads>) -> Self {
        Self { inner, threads }
    }
}

impl Editor for WindowedEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        self.threads.wait();
        if let ParentWindowHandle::X11Window(window) = parent {
            *self.threads.parent.lock().unwrap_or_else(PoisonError::into_inner) = Some(window);
        }
        Box::new(WindowHandle {
            inner: Some(self.inner.spawn(parent, context)),
            threads: self.threads.clone(),
        })
    }

    fn size(&self) -> (u32, u32) {
        self.inner.size()
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        self.inner.set_scale_factor(factor)
    }

    fn param_value_changed(&self, id: &str, normalized_value: f32) {
        self.inner.param_value_changed(id, normalized_value)
    }

    fn param_modulation_changed(&self, id: &str, modulation_offset: f32) {
        self.inner.param_modulation_changed(id, modulation_offset)
    }

    fn param_values_changed(&self) {
        self.inner.param_values_changed()
    }
}

/// An open window, which closes when it's dropped.
struct WindowHandle {
    inner: Option<Box<dyn Any + Send>>,
    threads: Arc<WindowThreads>,
}

impl Drop for WindowHandle {
    fn drop(&mut self) {
        // Asks the window to close, and waits until it has
        drop(self.inner.take());
        self.threads.wait();
    }
}

/// The thread of the window opened last, which tells when it has finished, and the host's
/// window the next one opens in.
#[derive(Default)]
pub struct WindowThreads {
    last: Mutex<Option<Arc<Finished>>>,
    parent: Mutex<Option<u32>>,
}

#[derive(Default)]
struct Finished {
    finished: Mutex<bool>,
    condvar: Condvar,
}

thread_local! {
    /// Tells that a window's thread has finished, as the thread's locals are dropped: after the
    /// window and its connection to the X server are gone.
    static ON_EXIT: RefCell<Option<OnExit>> = const { RefCell::new(None) };
    /// What the window's thread knows of the host's window.
    #[cfg(all(unix, not(target_os = "macos")))]
    static PARENT: RefCell<Option<x11::ParentWatch>> = const { RefCell::new(None) };
}

struct OnExit(Arc<Finished>);

impl Drop for OnExit {
    fn drop(&mut self) {
        *self.0.finished.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.condvar.notify_all();
    }
}

impl WindowThreads {
    /// Called on a window's thread as the window opens (by the editor's build function).
    pub fn opened(&self) {
        if !SEPARATE_THREADS {
            return;
        }
        let finished = Arc::new(Finished::default());
        ON_EXIT.with(|on_exit| *on_exit.borrow_mut() = Some(OnExit(finished.clone())));
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(finished);
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(parent) = self.parent.lock().unwrap_or_else(PoisonError::into_inner).take() {
            PARENT.with(|watch| *watch.borrow_mut() = x11::ParentWatch::new(parent));
        }
    }

    /// Waits until the thread of the window opened last has finished, if it hasn't.
    fn wait(&self) {
        let Some(last) = self.last.lock().unwrap_or_else(PoisonError::into_inner).clone() else {
            return;
        };
        let finished = last.finished.lock().unwrap_or_else(PoisonError::into_inner);
        let (finished, wait) = last
            .condvar
            .wait_timeout_while(finished, TIMEOUT, |finished| !*finished)
            .unwrap_or_else(PoisonError::into_inner);
        drop(finished);
        if wait.timed_out() {
            nih_log!("The editor's last window hadn't closed after {} s", TIMEOUT.as_secs_f32());
        }
    }
}

/// Ends the window's thread if the host has destroyed the window it was in: called at the start
/// of each frame, before anything is drawn. The thread unwinds (quietly, without a panic
/// message) out of baseview's event loop, which drops the window as it would on closing, but
/// without drawing into the destroyed window first.
pub fn end_if_orphaned() {
    #[cfg(all(unix, not(target_os = "macos")))]
    if PARENT.with(|watch| watch.borrow().as_ref().is_some_and(x11::ParentWatch::destroyed)) {
        nih_log!("The host destroyed the editor's window before closing the editor");
        std::panic::resume_unwind(Box::new(ParentDestroyed));
    }
}

/// What the window's thread unwinds with when the host's window is gone.
struct ParentDestroyed;

/// Watching the host's window over a connection to the X server of our own.
#[cfg(all(unix, not(target_os = "macos")))]
mod x11 {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask};
    use x11rb::protocol::{ErrorKind, Event};
    use x11rb::rust_connection::RustConnection;

    pub struct ParentWatch {
        connection: RustConnection,
        parent: u32,
    }

    impl ParentWatch {
        /// Starts watching `parent`, if the X server can be reached.
        pub fn new(parent: u32) -> Option<Self> {
            let (connection, _) = x11rb::connect(None).ok()?;
            let events = ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY);
            connection.change_window_attributes(parent, &events).ok()?;
            connection.flush().ok()?;
            Some(Self { connection, parent })
        }

        /// Whether the window has been destroyed, as far as the X server has told so far (or,
        /// for a window already gone when it started being watched, refused to watch it).
        pub fn destroyed(&self) -> bool {
            let mut destroyed = false;
            while let Ok(Some(event)) = self.connection.poll_for_event() {
                destroyed |= match event {
                    Event::DestroyNotify(notify) => notify.window == self.parent,
                    Event::Error(error) => error.error_kind == ErrorKind::Window && error.bad_value == self.parent,
                    _ => false,
                };
            }
            destroyed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn waits_for_the_window_thread_to_finish() {
        let threads = Arc::new(WindowThreads::default());
        // Nothing to wait for before a window has opened
        let start = Instant::now();
        threads.wait();
        assert!(start.elapsed() < Duration::from_millis(100));

        let (opened, wait_for_open) = std::sync::mpsc::channel();
        let window = {
            let threads = threads.clone();
            std::thread::spawn(move || {
                threads.opened();
                opened.send(()).unwrap();
                // A window that takes a while to close
                std::thread::sleep(Duration::from_millis(200));
            })
        };
        wait_for_open.recv().unwrap();
        let start = Instant::now();
        threads.wait();
        let waited = start.elapsed();
        if SEPARATE_THREADS {
            assert!(waited >= Duration::from_millis(150) && waited < TIMEOUT, "waited {waited:?}");
        } else {
            assert!(waited < Duration::from_millis(100), "waited {waited:?}");
        }
        window.join().unwrap();
    }
}
