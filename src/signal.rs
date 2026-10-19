//! Signal-safe unlinking of registered paths on termination.
//!
//! A path registered here is `unlink`ed if the process is killed by a
//! terminating signal - the case where ordinary `Drop`-based cleanup never
//! runs, which would otherwise strand the file.
//!
//! The handler must be async-signal-safe, so it can't allocate or take a mutex:
//! it reads a fixed table of raw C-string paths (populated via atomics as paths
//! are registered and unregistered) and calls `unlink` on each. We use a handler
//! rather than a `sigwait` thread so that `execve` - which resets handlers to
//! their default but preserves the signal mask - leaves an exec'd program's
//! signal behavior untouched.

#[cfg(unix)]
use ::std::os::unix::ffi::OsStrExt;
use ::std::path::Path;
#[cfg(unix)]
use ::std::sync::atomic::AtomicPtr;
#[cfg(unix)]
use ::std::sync::atomic::Ordering;

/// Max concurrently-registered paths the signal handler can track.
#[cfg(unix)]
const MAX_SLOTS: usize = 64;

/// Terminating signals we intercept to clean up before dying.
#[cfg(unix)]
const TERMINATING_SIGNALS: [::libc::c_int; 4] = [
    ::libc::SIGINT,
    ::libc::SIGTERM,
    ::libc::SIGHUP,
    ::libc::SIGQUIT,
];

/// Registered paths, as leaked C strings the handler can `unlink` without
/// allocating. A null slot is free.
#[cfg(unix)]
static GLOBAL_SLOTS: [AtomicPtr<::libc::c_char>; MAX_SLOTS] =
    [const { AtomicPtr::new(::std::ptr::null_mut()) }; MAX_SLOTS];

/// Namespace for signal-safe unlinking of registered paths on termination.
pub struct TerminatingSignals;

impl TerminatingSignals {
    /// Async-signal-safe handler: unlink every registered path, then restore
    /// the default disposition and re-raise so the process dies with the right
    /// status.
    #[cfg(unix)]
    extern "C" fn handle(sig: ::libc::c_int) {
        for slot in GLOBAL_SLOTS.iter() {
            let ptr = slot.load(Ordering::Acquire);
            if !ptr.is_null() {
                unsafe { ::libc::unlink(ptr) };
            }
        }
        unsafe {
            ::libc::signal(sig, ::libc::SIG_DFL);
            ::libc::raise(sig);
        }
    }

    /// Install the cleanup handler for every terminating signal, exactly once.
    #[cfg(unix)]
    fn install() {
        static STARTED: ::std::sync::Once = ::std::sync::Once::new();
        STARTED.call_once(|| unsafe {
            let mut action: ::libc::sigaction = ::std::mem::zeroed();
            action.sa_sigaction =
                Self::handle as extern "C" fn(::libc::c_int) as ::libc::sighandler_t;
            ::libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = 0;
            for &sig in &TERMINATING_SIGNALS {
                ::libc::sigaction(sig, &action, ::std::ptr::null_mut());
            }
        });
    }

    /// Register `path` to be `unlink`ed if a terminating signal fires
    /// (installing the handler on first use), returning the slot it took, or
    /// `None` when the path can't be tracked.
    #[cfg(unix)]
    pub(crate) fn register_unlink(path: &Path) -> Option<usize> {
        Self::install();
        // Leak a C string of the path so the handler can `unlink` it without any
        // allocation; the slot is reclaimed on unregister, the string is not freed.
        let path = ::std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let ptr = path.into_raw();
        for (i, slot) in GLOBAL_SLOTS.iter().enumerate() {
            if slot
                .compare_exchange(
                    ::std::ptr::null_mut(),
                    ptr,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return Some(i);
            }
        }
        // Table full: free the string and give up tracking this one.
        drop(unsafe { ::std::ffi::CString::from_raw(ptr) });
        None
    }

    #[cfg(not(unix))]
    pub(crate) fn register_unlink(_path: &Path) -> Option<usize> {
        None
    }

    /// Cancel the pending unlink registered in `slot`. The leaked C string is
    /// intentionally not freed, to avoid a use-after-free against a handler
    /// reading it concurrently; the leak is one small string per registration,
    /// bounded by a single run's work.
    #[cfg(unix)]
    pub(crate) fn unregister_unlink(slot: usize) {
        GLOBAL_SLOTS[slot].store(::std::ptr::null_mut(), Ordering::Release);
    }

    #[cfg(not(unix))]
    pub(crate) fn unregister_unlink(_slot: usize) {}
}
