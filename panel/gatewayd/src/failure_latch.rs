use crate::BackgroundTaskFailure;
use tokio::sync::watch;

/// Single-assignment failure channel used to preserve the initiating root cause.
///
/// Later failures remain visible through task joins and logs, but cannot replace
/// the failure that first requested process shutdown.
#[derive(Clone)]
pub(crate) struct BackgroundTaskFailureLatch {
    sender: watch::Sender<Option<BackgroundTaskFailure>>,
}

impl BackgroundTaskFailureLatch {
    pub(crate) fn new() -> Self {
        let (sender, _) = watch::channel(None);
        Self { sender }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Option<BackgroundTaskFailure>> {
        self.sender.subscribe()
    }

    pub(crate) fn latest(&self) -> Option<BackgroundTaskFailure> {
        self.sender.borrow().clone()
    }

    pub(crate) fn report(&self, failure: BackgroundTaskFailure) -> bool {
        self.sender.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(failure);
            true
        })
    }
}
