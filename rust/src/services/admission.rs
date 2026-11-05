use crate::domain::Error;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::timeout,
};

#[derive(Clone)]
pub struct Admission {
    slots: Arc<Semaphore>,
    seconds: u32,
}

impl Admission {
    pub fn new(slots: u32, seconds: u32) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(slots as usize)),
            seconds,
        }
    }

    pub fn acquire(&self) -> Result<OwnedSemaphorePermit, Error> {
        self.slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::CapacityExceeded)
    }

    pub async fn wait<T: Send + 'static>(
        &self,
        task: impl Future<Output = Result<T, Error>> + Send + 'static,
    ) -> Result<T, Error> {
        // Dropping a JoinHandle detaches work; its owner still drains SQL and holds the slot.
        timeout(Duration::from_secs(self.seconds.into()), tokio::spawn(task))
            .await
            .map_err(|_| Error::QueryTimeout)?
            .map_err(|_| Error::InternalError)?
    }
}
