//! Graceful shutdown on SIGINT/SIGTERM.
//!
//! glib 0.22 no longer ships unix signal sources, so we do the classic
//! "block in every thread, wait in one" dance: the mask is set in the main
//! thread before any other thread exists (they inherit it) and a dedicated
//! thread blocks in `sigwait`. The first signal asks the app to quit, which
//! shuts the engine down cleanly; a second one exits immediately.

use std::mem::MaybeUninit;

fn termination_set() -> libc::sigset_t {
    // SAFETY: plain libc calls on a properly initialised, stack-owned set.
    unsafe {
        let mut set = MaybeUninit::<libc::sigset_t>::uninit();
        libc::sigemptyset(set.as_mut_ptr());
        libc::sigaddset(set.as_mut_ptr(), libc::SIGINT);
        libc::sigaddset(set.as_mut_ptr(), libc::SIGTERM);
        set.assume_init()
    }
}

/// Must be called before any thread is spawned.
pub fn block_termination_signals() {
    let set = termination_set();
    // SAFETY: `set` is valid; a null old-set pointer is allowed.
    let rc = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
    if rc != 0 {
        log::warn!("cannot block termination signals (errno {rc})");
    }
}

/// Spawn the thread that waits for SIGINT/SIGTERM. `on_signal` is called
/// once, from that thread, on the first signal.
pub fn spawn_watcher<F: FnOnce() + Send + 'static>(on_signal: F) {
    let spawned = std::thread::Builder::new()
        .name("pipedeck-signals".into())
        .spawn(move || {
            let set = termination_set();
            let mut on_signal = Some(on_signal);
            loop {
                let mut signal: libc::c_int = 0;
                // SAFETY: `set` and `signal` are valid for the duration of the call.
                if unsafe { libc::sigwait(&set, &mut signal) } != 0 {
                    return;
                }
                match on_signal.take() {
                    Some(f) => {
                        log::info!("signal {signal} received, shutting down");
                        f();
                    }
                    None => {
                        log::warn!("second signal {signal}, exiting immediately");
                        std::process::exit(128 + signal);
                    }
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("cannot spawn the signal watcher: {e}");
    }
}
