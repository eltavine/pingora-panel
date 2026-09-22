use std::sync::Arc;

pub struct ApiState<U> {
    pub(crate) use_cases: Arc<U>,
}

impl<U> Clone for ApiState<U> {
    fn clone(&self) -> Self {
        Self {
            use_cases: Arc::clone(&self.use_cases),
        }
    }
}

impl<U> ApiState<U> {
    pub fn new(use_cases: Arc<U>) -> Self {
        Self { use_cases }
    }
}
