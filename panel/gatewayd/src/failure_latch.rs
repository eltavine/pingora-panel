use tokio::sync::watch;

/// Generic single-assignment channel used to preserve the initiating value.
///
/// The concrete value type is selected at the owning composition boundary, so
/// this synchronization primitive does not depend on background-task semantics.
pub(crate) struct FirstValueLatch<T> {
    sender: watch::Sender<Option<T>>,
}

// Deriving Clone would unnecessarily require T: Clone even though cloning a
// watch sender does not clone the stored value.
impl<T> Clone for FirstValueLatch<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<T> FirstValueLatch<T> {
    pub(crate) fn new() -> Self {
        let (sender, _) = watch::channel(None);
        Self { sender }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Option<T>> {
        self.sender.subscribe()
    }

    pub(crate) fn try_set(&self, value: T) -> bool {
        self.sender.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(value);
            true
        })
    }
}

impl<T: Clone> FirstValueLatch<T> {
    pub(crate) fn latest(&self) -> Option<T> {
        self.sender.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn preserves_the_first_value() {
        let latch = FirstValueLatch::new();
        let receiver = latch.subscribe();
        let first = "first failure".to_owned();
        let later = "later failure".to_owned();

        assert!(latch.try_set(first.clone()));
        assert!(!latch.try_set(later));
        assert_eq!(receiver.borrow().as_ref(), Some(&first));
    }

    #[test]
    fn cloning_the_latch_does_not_require_a_cloneable_value() {
        struct NonClone(&'static str);

        let latch = FirstValueLatch::new();
        let writer = latch.clone();
        let receiver = latch.subscribe();

        assert!(writer.try_set(NonClone("first value")));
        assert_eq!(
            receiver.borrow().as_ref().map(|value| value.0),
            Some("first value")
        );
    }

    #[test]
    fn is_single_assignment_under_concurrent_reports() {
        const ITERATIONS: usize = 16;
        const REPORTERS: usize = 8;

        for _ in 0..ITERATIONS {
            let latch = Arc::new(FirstValueLatch::new());
            let barrier = Arc::new(Barrier::new(REPORTERS));
            let handles = (0..REPORTERS)
                .map(|index| {
                    let latch = Arc::clone(&latch);
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        let failure = format!("failure-{index}");
                        barrier.wait();
                        let accepted = latch.try_set(failure.clone());
                        (failure, accepted)
                    })
                })
                .collect::<Vec<_>>();

            let mut accepted_failure = None;
            for handle in handles {
                let (failure, accepted) = handle.join().expect("failure reporter panicked");
                if accepted {
                    assert!(
                        accepted_failure.replace(failure).is_none(),
                        "more than one concurrent failure was accepted"
                    );
                }
            }

            let accepted_failure = accepted_failure.expect("no concurrent failure was accepted");
            assert_eq!(latch.latest().as_ref(), Some(&accepted_failure));
        }
    }
}
