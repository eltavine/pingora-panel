//! The small linearization point shared by mutation admission and shutdown.
//!
//! Keeping this state separate from Tokio's semaphore and task tracker makes
//! the close/admit invariant easy to model with Loom without coupling the
//! production executor to a test runtime.

#[derive(Default, Debug, Eq, PartialEq)]
pub(crate) struct MutationLifecycle {
    closed: bool,
}

impl MutationLifecycle {
    pub(crate) fn is_closed(&self) -> bool {
        self.closed
    }

    pub(crate) fn close(&mut self) -> bool {
        if self.closed {
            return false;
        }
        self.closed = true;
        true
    }
}

#[cfg(all(test, loom))]
mod loom_tests {
    use super::MutationLifecycle;
    use loom::{sync::Arc, sync::Mutex, thread};

    #[test]
    fn admission_and_close_are_linearizable() {
        loom::model(|| {
            let lifecycle = Arc::new(Mutex::new(MutationLifecycle::default()));
            let admitted = Arc::new(Mutex::new(0usize));
            let rejected = Arc::new(Mutex::new(0usize));

            let admit_lifecycle = Arc::clone(&lifecycle);
            let admit_count = Arc::clone(&admitted);
            let reject_count = Arc::clone(&rejected);
            let admit = thread::spawn(move || {
                let lifecycle = admit_lifecycle.lock().unwrap();
                if lifecycle.is_closed() {
                    *reject_count.lock().unwrap() += 1;
                } else {
                    *admit_count.lock().unwrap() += 1;
                }
            });

            let close_lifecycle = Arc::clone(&lifecycle);
            let close = thread::spawn(move || {
                assert!(close_lifecycle.lock().unwrap().close());
            });

            admit.join().unwrap();
            close.join().unwrap();

            assert_eq!(*admitted.lock().unwrap() + *rejected.lock().unwrap(), 1);
            assert!(lifecycle.lock().unwrap().is_closed());
        });
    }
}
