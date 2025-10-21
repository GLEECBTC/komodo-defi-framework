use common::executor::{BoxFutureSpawner, SpawnFuture};
use futures::Future;
use libp2p::swarm::Executor;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Clone)]
pub struct SwarmRuntime {
    inner: Arc<dyn BoxFutureSpawner + Send + Sync>,
}

impl SwarmRuntime {
    pub fn new<S>(spawner: S) -> SwarmRuntime
    where
        S: BoxFutureSpawner + Send + Sync + 'static,
    {
        SwarmRuntime {
            inner: Arc::new(spawner),
        }
    }
}

impl SpawnFuture for SwarmRuntime {
    fn spawn<F>(&self, f: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.inner.spawn_boxed(Box::new(Box::pin(f)))
    }
}

impl Executor for SwarmRuntime {
    fn exec(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) {
        self.inner.spawn_boxed(Box::new(future))
    }
}
