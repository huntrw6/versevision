//! Runs a future to its end on the calling thread, INSIDE a Tokio runtime.
//!
//! The Bible providers talk over `reqwest`, whose futures only work while a Tokio runtime is entered; a plain executor such as
//! `pollster::block_on` polls them outside one and they panic with "there is no reactor running". Every command that can reach a
//! provider (`run`, `import`, `parse`, `providers test`) goes through here. A small current-thread runtime is enough: the commands
//! do one thing at a time, and the web page (`serve`) has a runtime of its own.

use std::future::Future;

/// `future` to completion, with a Tokio runtime entered while it runs.
pub fn block_on<F: Future>(future: F) -> F::Output {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(future),
        // A runtime that cannot start (no threads or sockets at all) is not recoverable here; say so instead of polling blind.
        Err(error) => panic!("VerseVision could not start its network runtime: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::block_on;

    #[test]
    fn a_future_that_needs_a_tokio_runtime_runs() {
        // `tokio::time::sleep` and `reqwest` panic ("there is no reactor running") when polled by an executor with no runtime.
        let answer = block_on(async {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            7
        });
        assert_eq!(answer, 7);
    }

    #[test]
    fn a_socket_call_fails_cleanly_and_does_not_panic() {
        // A socket needs the runtime's reactor, as reqwest's does. Nothing listens on port 1: the call comes back as an error value.
        let result = block_on(async {
            tokio::net::TcpStream::connect(("127.0.0.1", 1))
                .await
                .map(|_| ())
        });
        assert!(result.is_err());
    }
}
