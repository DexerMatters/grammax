pub mod layers;
pub mod passes;
pub use layers::SourceText;

use std::thread::{self, JoinHandle};
use std::{fmt, marker::PhantomData};

use crossbeam::channel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct LayerName(u64);

impl LayerName {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn root() -> Self {
        // Reserved for the top-most user-submitted layer in a composed compiler.
        Self(0x6f63_a9e2_4a71_11c1)
    }

    pub const fn runtime() -> Self {
        Self(0x08a8_6514_4f6b_72de)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for LayerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "layer:{:016x}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PassId(u64);

impl PassId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn ingress() -> Self {
        // Reserved for submit-at-root events.
        Self(0x8f01_dca1_3ae0_43ef)
    }

    pub const fn runtime_error() -> Self {
        Self(0x58d0_8398_f905_174d)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PassId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pass:{:016x}", self.0)
    }
}

pub trait IR {
    type Ix;
    type Value;
    type Error;

    fn query(&self, index: Self::Ix) -> Result<Self::Value, Self::Error>;

    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Error>
    where
        Self: Sized;
}

pub trait Pass<U: IR, D: IR> {
    type Error;

    fn transform(
        &mut self,
        upstream: &U,
        txn: Transaction<U>,
    ) -> Result<Transaction<D>, Self::Error>;
}

struct QueryMsg<Repr: IR> {
    index: Repr::Ix,
    reply: channel::Sender<Result<Repr::Value, Repr::Error>>,
}

pub struct QueryHandle<Repr: IR> {
    sender: channel::Sender<QueryMsg<Repr>>,
}

impl<Repr: IR> Clone for QueryHandle<Repr> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<Repr: IR> QueryHandle<Repr> {
    pub fn query(&self, index: Repr::Ix) -> Option<Result<Repr::Value, Repr::Error>> {
        let (reply_tx, reply_rx) = channel::bounded(1);
        self.sender
            .send(QueryMsg {
                index,
                reply: reply_tx,
            })
            .ok()?;
        reply_rx.recv().ok()
    }
}

pub(crate) struct IRInstance<Repr: IR + Send + 'static>
where
    Repr::Ix: Send + Sync,
    Repr::Value: Send + Sync,
    Repr::Error: Send,
{
    handle: JoinHandle<()>,
    sender: channel::Sender<Transaction<Repr>>,
    query_sender: channel::Sender<QueryMsg<Repr>>,
}

impl<Repr: IR + Send + 'static> IRInstance<Repr>
where
    Repr::Ix: Send + Sync,
    Repr::Value: Send + Sync,
    Repr::Error: Send,
{
    pub fn new(repr: Repr) -> Self {
        let (sender, txn_rx) = channel::unbounded::<Transaction<Repr>>();
        let (query_sender, query_rx) = channel::unbounded::<QueryMsg<Repr>>();
        let handle = thread::spawn(move || {
            let mut repr = repr;
            loop {
                crossbeam::select! {
                    recv(txn_rx) -> msg => match msg {
                        Ok(txn) => { let _ = repr.apply_transaction(txn); }
                        Err(_) => {
                            for msg in query_rx.try_iter() {
                                let _ = msg.reply.send(repr.query(msg.index));
                            }
                            break;
                        }
                    },
                    recv(query_rx) -> msg => match msg {
                        Ok(QueryMsg { index, reply }) => {
                            let _ = reply.send(repr.query(index));
                        }
                        Err(_) => {}
                    },
                }
            }
        });
        IRInstance {
            handle,
            sender,
            query_sender,
        }
    }
    pub fn query_handle(&self) -> QueryHandle<Repr> {
        QueryHandle {
            sender: self.query_sender.clone(),
        }
    }

    pub fn shutdown(self) {
        drop(self.sender);
        let _ = self.handle.join();
    }
}

/// Concurrent wrapper for one pipeline stage:
/// owns upstream IR + pass in a worker thread and streams downstream transactions.
pub struct Pipeline<U, P, D>
where
    U: IR + 'static,
    U::Ix: Send + Sync,
    U::Value: Send + Sync,
    U::Error: Send,
    D: IR + Send + 'static,
    D::Ix: Send + Sync,
    D::Value: Send + Sync,
    D::Error: Send,
    P: Pass<U, D> + 'static,
    P::Error: Send,
{
    handle: JoinHandle<()>,
    sender: channel::Sender<Transaction<U>>,
    upstream_query_sender: channel::Sender<QueryMsg<U>>,
    pub(crate) downstream: IRInstance<D>,
    _pass: PhantomData<P>,
}

impl<U, P, D> Pipeline<U, P, D>
where
    U: IR + 'static,
    U::Ix: Clone + Send + Sync,
    U::Value: Clone + Send + Sync,
    U::Error: Send,
    D: IR + Send + 'static,
    D::Ix: Clone + Send + Sync,
    D::Value: Clone + Send + Sync,
    D::Error: Send,
    P: Pass<U, D> + 'static,
    P::Error: Send,
{
    pub fn connect(upstream: U, pass: P, downstream: D) -> Self
    where
        U: Send,
        P: Send,
    {
        Self::connect_with(move || upstream, move || pass, downstream)
    }

    pub fn connect_with<UF, PF>(make_upstream: UF, make_pass: PF, downstream: D) -> Self
    where
        UF: FnOnce() -> U + Send + 'static,
        PF: FnOnce() -> P + Send + 'static,
    {
        Self::connect_with_tap(make_upstream, make_pass, downstream, None)
    }

    pub fn connect_with_tap<UF, PF>(
        make_upstream: UF,
        make_pass: PF,
        downstream: D,
        tap_sender: Option<channel::Sender<Transaction<D>>>,
    ) -> Self
    where
        UF: FnOnce() -> U + Send + 'static,
        PF: FnOnce() -> P + Send + 'static,
    {
        let downstream = IRInstance::new(downstream);
        let downstream_sender = downstream.sender.clone();
        let (sender, receiver) = channel::unbounded::<Transaction<U>>();
        let (upstream_query_sender, upstream_query_rx) = channel::unbounded::<QueryMsg<U>>();

        let handle = thread::spawn(move || {
            let mut upstream = make_upstream();
            let mut pass = make_pass();
            loop {
                crossbeam::select! {
                    recv(receiver) -> msg => match msg {
                        Ok(txn) => {
                            let for_pass = std::sync::Arc::clone(&txn);

                            if upstream.apply_transaction(txn).is_err() {
                                continue;
                            }

                            if let Ok(downstream_txn) = pass.transform(&upstream, for_pass) {
                                let cloned = std::sync::Arc::clone(&downstream_txn);
                                let _ = downstream_sender.send(cloned);
                                if let Some(tap) = tap_sender.as_ref() {
                                    let _ = tap.send(downstream_txn);
                                }
                            }
                        }
                        Err(_) => {
                            for msg in upstream_query_rx.try_iter() {
                                let _ = msg.reply.send(upstream.query(msg.index));
                            }
                            break;
                        }
                    },
                    recv(upstream_query_rx) -> msg => match msg {
                        Ok(QueryMsg { index, reply }) => {
                            let _ = reply.send(upstream.query(index));
                        }
                        Err(_) => {}
                    }
                }
            }
        });

        Pipeline {
            handle,
            sender,
            upstream_query_sender,
            downstream,
            _pass: PhantomData,
        }
    }

    pub fn send(&self, txn: Transaction<U>) {
        let _ = self.sender.send(txn);
    }

    pub fn clone_sender(&self) -> channel::Sender<Transaction<U>> {
        self.sender.clone()
    }

    pub fn upstream_query_handle(&self) -> QueryHandle<U> {
        QueryHandle {
            sender: self.upstream_query_sender.clone(),
        }
    }

    pub fn query_upstream(&self, index: U::Ix) -> Option<Result<U::Value, U::Error>> {
        self.upstream_query_handle().query(index)
    }

    pub fn shutdown(self) {
        drop(self.sender);
        let _ = self.handle.join();
        self.downstream.shutdown();
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[serde(bound(
    serialize = "Repr::Ix: serde::Serialize, Repr::Value: serde::Serialize",
    deserialize = "Repr::Ix: serde::Deserialize<'de>, Repr::Value: serde::Deserialize<'de>"
))]
pub enum Command<Repr: IR> {
    Create { id: usize, value: Repr::Value },
    Insert { index: Repr::Ix, id: usize },
    Delete { index: Repr::Ix },
    Replace { index: Repr::Ix, id: usize },
    SetRoot { id: Option<usize> },
}

impl<Repr: IR> Command<Repr> {
    /// Clone this command by cloning only its fields.
    ///
    /// This method only requires `Repr::Ix: Clone` and `Repr::Value: Clone`,
    /// unlike the derive-generated `Clone` impl which requires `Repr: Clone`.
    pub fn clone_fields(&self) -> Self
    where
        Repr::Ix: Clone,
        Repr::Value: Clone,
    {
        match self {
            Command::Create { id, value } => Command::Create {
                id: *id,
                value: value.clone(),
            },
            Command::Insert { index, id } => Command::Insert {
                index: index.clone(),
                id: *id,
            },
            Command::Delete { index } => Command::Delete {
                index: index.clone(),
            },
            Command::Replace { index, id } => Command::Replace {
                index: index.clone(),
                id: *id,
            },
            Command::SetRoot { id } => Command::SetRoot { id: *id },
        }
    }
}

pub type Transaction<Repr> = std::sync::Arc<Vec<Command<Repr>>>;
