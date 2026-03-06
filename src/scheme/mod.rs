pub mod layers;
pub mod passes;
pub use layers::SourceText;

use std::marker::PhantomData;
use std::thread::{self, JoinHandle};

use crossbeam::channel;

/// An Intermediate Representation (IR) layer modeled as an incremental database.
///
/// Each IR layer stores structural data and exposes it through CRUD commands and
/// point queries. Multiple commands batched into a [`Transaction`] represent an
/// atomic time-slice of updates (the "water flowing down one step" in the
/// terraced compiler model).
///
/// # Transaction Ordering Invariant
///
/// Within a transaction, every `Create` command that mints a local id `ℕ`
/// **must appear before** any `Insert` or `Replace` command that references
/// that id. Upstream IR layers and [`Pass`] implementations are responsible for
/// upholding this invariant when producing transactions.
pub trait IR {
    type Ix;
    type Value;
    type Error;

    /// Query the IR at a given index. Read-only; may be called at any time by
    /// downstream [`Pass`]es or external observers (the "sunlight hitting the
    /// terraced slope" in the document's metaphor).
    fn query(&self, index: Self::Ix) -> Result<Self::Value, Self::Error>;

    /// Apply a full transaction to this IR.
    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Error>
    where
        Self: Sized;
}

/// A Pass transforms a [`Transaction`] from upstream IR `U` into a
/// [`Transaction`] for downstream IR `D`.
///
/// After the upstream IR has applied the incoming transaction, the Pass
/// receives both the *updated* upstream IR (for querying historical context)
/// and the original transaction commands (to know what changed). It then
/// produces only the incremental delta needed by the downstream layer —
/// querying the upstream IR for anything else it requires.
///
/// `&mut self` allows the Pass to maintain stateful caches between calls
/// (e.g. alignment tables or LRU caches for incremental diffing).
pub trait Pass<U: IR, D: IR> {
    type Error;

    fn transform(
        &mut self,
        upstream: &U,
        txn: Transaction<U>,
    ) -> Result<Transaction<D>, Self::Error>;
}

/// Internal message type for IR queries crossing the thread boundary.
/// Bundles the index with a one-shot reply channel.
struct QueryMsg<Repr: IR> {
    index: Repr::Ix,
    reply: channel::Sender<Result<Repr::Value, Repr::Error>>,
}

/// A running IR layer backed by a dedicated worker thread.
///
/// Receives [`Transaction`]s and query requests through separate channels and
/// processes them in arrival order on a single worker thread. This guarantees
/// that every query sees a fully consistent state: it is always processed
/// between two complete transactions, never in the middle of one.
///
/// The transaction channel acts as the backpressure buffer between concurrent
/// pipeline stages (bounded channels provide natural backpressure).
pub struct IRInstance<Repr: IR + Send + 'static>
where
    Repr::Ix: Send,
    Repr::Value: Send,
    Repr::Error: Send,
{
    handle: JoinHandle<()>,
    sender: channel::Sender<Transaction<Repr>>,
    query_sender: channel::Sender<QueryMsg<Repr>>,
}

impl<Repr: IR + Send + 'static> IRInstance<Repr>
where
    Repr::Ix: Send,
    Repr::Value: Send,
    Repr::Error: Send,
{
    pub fn new(repr: Repr) -> Self {
        let (sender, txn_rx) = channel::unbounded::<Transaction<Repr>>();
        let (query_sender, query_rx) = channel::unbounded::<QueryMsg<Repr>>();
        let handle = thread::spawn(move || {
            let mut repr = repr;
            // Both channels must be drained before the thread exits.  When the
            // transaction sender is dropped we stop accepting new work; when the
            // query sender is also dropped the loop terminates.
            loop {
                crossbeam::select! {
                    recv(txn_rx) -> msg => match msg {
                        Ok(txn) => { let _ = repr.apply_transaction(txn); }
                        // Transaction channel closed: no more updates coming.
                        // Drain any pending queries then exit.
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
                        Err(_) => {} // query channel closed; keep processing transactions
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

    /// Enqueue a transaction to be applied by the worker thread.
    pub fn send(&self, txn: Transaction<Repr>) {
        let _ = self.sender.send(txn);
    }

    /// Clone the transaction sender, allowing multiple producers to feed this IR instance.
    pub fn clone_sender(&self) -> channel::Sender<Transaction<Repr>> {
        self.sender.clone()
    }

    /// Query the IR at `index`. Blocks the calling thread until the worker
    /// processes the request, guaranteeing the result reflects all transactions
    /// applied before this call returned on the worker thread.
    ///
    /// Returns `None` if the worker has already shut down.
    pub fn query(&self, index: Repr::Ix) -> Option<Result<Repr::Value, Repr::Error>> {
        let (reply_tx, reply_rx) = channel::bounded(1);
        self.query_sender
            .send(QueryMsg {
                index,
                reply: reply_tx,
            })
            .ok()?;
        reply_rx.recv().ok()
    }

    /// Drop the transaction sender and wait for the worker thread to finish.
    pub fn shutdown(self) {
        drop(self.sender); // closes the transaction channel; worker drains queries then exits
        let _ = self.handle.join();
    }
}

pub struct Pipeline<U, P, D>
where
    U: IR + 'static,
    U::Ix: Send,
    U::Value: Send,
    U::Error: Send,
    D: IR + Send + 'static,
    D::Ix: Send,
    D::Value: Send,
    D::Error: Send,
    P: Pass<U, D> + 'static,
    P::Error: Send,
{
    /// Worker thread owning the upstream IR and the Pass.
    handle: JoinHandle<()>,
    /// Send transactions into the upstream end.
    sender: channel::Sender<Transaction<U>>,
    /// The downstream IR stage; exposed so it can be chained into further pipelines.
    pub downstream: IRInstance<D>,
    _pass: PhantomData<P>,
}

impl<U, P, D> Pipeline<U, P, D>
where
    U: IR + 'static,
    U::Ix: Clone + Send,
    U::Value: Clone + Send,
    U::Error: Send,
    D: IR + Send + 'static,
    D::Ix: Clone + Send,
    D::Value: Clone + Send,
    D::Error: Send,
    P: Pass<U, D> + 'static,
    P::Error: Send,
{
    /// Wire an upstream IR, a pass, and a downstream IR into a live pipeline.
    ///
    /// Internally this clones each incoming `Transaction<U>` once: the clone
    /// is applied to the upstream IR while the original is forwarded to the
    /// Pass (which needs the commands to know what changed, and queries
    /// `upstream` for the rest).
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
        let handle = thread::spawn(move || {
            let mut upstream = make_upstream();
            let mut pass = make_pass();
            while let Ok(txn) = receiver.recv() {
                let for_pass = clone_transaction::<U>(&txn);

                if upstream.apply_transaction(txn).is_err() {
                    continue;
                }
                if let Ok(downstream_txn) = pass.transform(&upstream, for_pass) {
                    let cloned = clone_transaction::<D>(&downstream_txn);
                    let _ = downstream_sender.send(cloned);
                    if let Some(tap) = tap_sender.as_ref() {
                        let _ = tap.send(downstream_txn);
                    }
                }
            }
        });

        Pipeline {
            handle,
            sender,
            downstream,
            _pass: PhantomData,
        }
    }

    /// Enqueue a transaction into the upstream end of the pipeline.
    pub fn send(&self, txn: Transaction<U>) {
        let _ = self.sender.send(txn);
    }

    pub fn clone_sender(&self) -> channel::Sender<Transaction<U>> {
        self.sender.clone()
    }

    /// Shut down both pipeline stages and wait for all in-flight transactions
    /// to be processed.
    pub fn shutdown(self) {
        drop(self.sender); // upstream thread exits after draining its channel
        let _ = self.handle.join();
        self.downstream.shutdown();
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[serde(bound(serialize = "Repr::Ix: serde::Serialize, Repr::Value: serde::Serialize"))]
pub enum Command<Repr: IR> {
    /// Mint a new value with local transaction id `id`. Must precede any
    /// `Insert` or `Replace` that references this id within the same transaction.
    Create {
        id: usize,
        value: Repr::Value,
    },
    Insert {
        index: Repr::Ix,
        id: usize,
    },
    Delete {
        index: Repr::Ix,
    },
    Replace {
        index: Repr::Ix,
        id: usize,
    },
    /// Set the IR's logical root to the node created with this transaction id.
    /// `None` clears the root.  IRs with no root concept may ignore this.
    SetRoot {
        id: Option<usize>,
    },
}

pub type Transaction<Repr> = Vec<Command<Repr>>;

pub fn clone_transaction<Repr>(txn: &[Command<Repr>]) -> Transaction<Repr>
where
    Repr: IR,
    Repr::Ix: Clone,
    Repr::Value: Clone,
{
    txn.iter()
        .map(|cmd| match cmd {
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
        })
        .collect()
}
