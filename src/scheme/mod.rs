pub mod doc;
pub mod layers;
pub mod passes;
pub use doc::*;
pub use layers::{SourceAtom, SourceText};

use std::thread::{self, JoinHandle};
use std::{
    fmt,
    marker::PhantomData,
    sync::{Arc, OnceLock},
};

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

/// Three-way result for a lazy IR query.
///
/// - `Present(V)`: the value is available.
/// - `Absent`:     the index is not yet populated; demand will resolve it.
/// - `Fault(F)`:   a permanent domain error; demand cannot resolve it.
pub enum LazyResult<V, F> {
    Present(V),
    Absent,
    Fault(F),
}

impl<V, F> LazyResult<V, F> {
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    pub fn ok(self) -> Option<V> {
        match self {
            Self::Present(v) => Some(v),
            _ => None,
        }
    }
}

pub trait IR {
    type Ix;
    type Value;
    /// Only permanent domain errors. Absence is expressed via `LazyResult::Absent`.
    type Fault;

    fn query(&self, index: Self::Ix) -> LazyResult<Self::Value, Self::Fault>;

    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Fault>
    where
        Self: Sized;
}

#[derive(Debug)]
pub enum ObserveError<F> {
    /// Channel not yet wired; may resolve soon.
    NotReady,
    /// Pipeline is dead; transient at the channel level.
    Disconnected,
    /// The queried index is not yet populated; demand can resolve it.
    Absent,
    /// Permanent domain fault; demand cannot resolve this.
    Fault(F),
    /// Demand was attempted but the pass gave up permanently.
    Exhausted,
}

impl<F> ObserveError<F> {
    /// Returns `true` for all conditions that may resolve with more upstream
    /// data or a future transaction — i.e. the three transient states.
    pub fn is_resolvable(&self) -> bool {
        matches!(self, Self::NotReady | Self::Disconnected | Self::Absent)
    }
}

pub trait Pass<U: IR, D: IR> {
    /// Called when upstream emits a transaction. Return the corresponding
    /// downstream commands (may be empty).
    fn push(
        &mut self,
        upstream: &LayerObserver<U>,
        downstream: &D,
        txn: &[Command<U>],
    ) -> Vec<Command<D>>;

    /// Called when `index` is queried but not yet present in downstream.
    /// Return `PullOutcome::Ready(cmds)` to populate it,
    /// `PullOutcome::Pending` if upstream data isn't available yet (will
    /// retry after the next push), or `PullOutcome::Dead` if the index
    /// can never be resolved.
    fn pull(&mut self, upstream: &LayerObserver<U>, downstream: &D, index: D::Ix)
    -> PullOutcome<D>;
}

pub enum PullOutcome<Repr: IR> {
    /// Commands that will populate the requested index.
    Ready(Vec<Command<Repr>>),
    /// Upstream data isn't available yet; retry after the next push.
    Pending,
    /// This index can never be resolved by this pass.
    Dead,
}

pub(crate) struct QueryMsg<Repr: IR> {
    pub(crate) index: Repr::Ix,
    pub(crate) strict: bool,
    pub(crate) reply: channel::Sender<Result<Repr::Value, ObserveError<Repr::Fault>>>,
}

pub struct QueryHandle<Repr: IR> {
    query_sender: channel::Sender<QueryMsg<Repr>>,
}

impl<Repr: IR> Clone for QueryHandle<Repr> {
    fn clone(&self) -> Self {
        Self {
            query_sender: self.query_sender.clone(),
        }
    }
}

impl<Repr: IR> QueryHandle<Repr> {
    pub(crate) fn from_sender(query_sender: channel::Sender<QueryMsg<Repr>>) -> Self {
        Self { query_sender }
    }

    fn query_with_mode(
        &self,
        index: Repr::Ix,
        strict: bool,
    ) -> Result<Repr::Value, ObserveError<Repr::Fault>> {
        let (reply_tx, reply_rx) = channel::bounded(1);
        self.query_sender
            .send(QueryMsg {
                index,
                strict,
                reply: reply_tx,
            })
            .map_err(|_| ObserveError::Disconnected)?;
        reply_rx.recv().map_err(|_| ObserveError::Disconnected)?
    }

    pub fn query(&self, index: Repr::Ix) -> Result<Repr::Value, ObserveError<Repr::Fault>> {
        self.query_with_mode(index, false)
    }

    pub fn query_strict(&self, index: Repr::Ix) -> Result<Repr::Value, ObserveError<Repr::Fault>> {
        self.query_with_mode(index, true)
    }
}

#[derive(Clone)]
pub struct LayerObserver<Repr: IR> {
    pub updates: channel::Receiver<(u64, Transaction<Repr>)>,
    handle: Arc<OnceLock<QueryHandle<Repr>>>,
}

impl<Repr: IR> LayerObserver<Repr> {
    pub fn recv_update(&self) -> Option<(u64, Transaction<Repr>)> {
        self.updates.recv().ok()
    }

    pub fn recv(&self) -> Option<Transaction<Repr>> {
        self.recv_update().map(|(_, txn)| txn)
    }

    pub fn try_recv_update(&self) -> Option<(u64, Transaction<Repr>)> {
        self.updates.try_recv().ok()
    }

    pub fn try_recv(&self) -> Option<Transaction<Repr>> {
        self.try_recv_update().map(|(_, txn)| txn)
    }

    pub fn query(&self, index: Repr::Ix) -> Result<Repr::Value, ObserveError<Repr::Fault>> {
        self.handle
            .get()
            .map_or(Err(ObserveError::NotReady), |h| h.query(index))
    }

    pub fn query_strict(&self, index: Repr::Ix) -> Result<Repr::Value, ObserveError<Repr::Fault>> {
        self.handle
            .get()
            .map_or(Err(ObserveError::NotReady), |h| h.query_strict(index))
    }

    pub(crate) fn new(
        updates: channel::Receiver<(u64, Transaction<Repr>)>,
        handle: Arc<OnceLock<QueryHandle<Repr>>>,
    ) -> Self {
        Self { updates, handle }
    }

    pub(crate) fn from_handle(handle: QueryHandle<Repr>) -> Self {
        let (updates_tx, updates) = channel::unbounded();
        drop(updates_tx);
        let lock = Arc::new(OnceLock::new());
        let _ = lock.set(handle);
        Self {
            updates,
            handle: lock,
        }
    }
}

fn resolve_query<U, P, D>(
    pass: &mut P,
    upstream: &LayerObserver<U>,
    downstream: &mut D,
    demanded: &mut Vec<D::Ix>,
    index: D::Ix,
    strict: bool,
) -> Result<D::Value, ObserveError<D::Fault>>
where
    U: IR,
    D: IR,
    P: Pass<U, D>,
    D::Ix: Clone + PartialEq,
{
    let remember = |demanded: &mut Vec<D::Ix>, index: &D::Ix| {
        if !demanded.iter().any(|existing| existing == index) {
            demanded.push(index.clone());
        }
    };

    match downstream.query(index.clone()) {
        LazyResult::Present(value) => {
            if !strict {
                remember(demanded, &index);
            }
            Ok(value)
        }
        LazyResult::Fault(f) => Err(ObserveError::Fault(f)),
        LazyResult::Absent if strict => Err(ObserveError::Absent),
        LazyResult::Absent => {
            let needs_upstream = match pass.pull(upstream, downstream, index.clone()) {
                PullOutcome::Dead => return Err(ObserveError::Exhausted),
                PullOutcome::Ready(txn) => {
                    if !txn.is_empty() {
                        downstream
                            .apply_transaction(std::sync::Arc::new(txn))
                            .map_err(ObserveError::Fault)?;
                    }
                    false
                }
                PullOutcome::Pending => true,
            };

            match downstream.query(index.clone()) {
                LazyResult::Present(value) => {
                    remember(demanded, &index);
                    Ok(value)
                }
                LazyResult::Absent => {
                    if needs_upstream {
                        remember(demanded, &index);
                    }
                    Err(ObserveError::Absent)
                }
                LazyResult::Fault(f) => Err(ObserveError::Fault(f)),
            }
        }
    }
}

/// Concurrent wrapper for one pipeline stage.
///
/// The stage owns the downstream IR and reaches upstream through a
/// [`LayerObserver`]. Transactions still flow top-to-bottom, while demand-aware
/// queries resolve missing state by re-entering the same pass contract.
pub struct Pipeline<U, P, D>
where
    U: IR + Send + 'static,
    U::Ix: Send + Sync,
    U::Value: Send + Sync,
    U::Fault: Send,
    D: IR + Send + Clone + 'static,
    D::Ix: Send + Sync + PartialEq,
    D::Value: Send + Sync,
    D::Fault: Send,
    P: Pass<U, D> + 'static,
{
    handle: JoinHandle<()>,
    sender: channel::Sender<Transaction<U>>,
    query_sender: channel::Sender<QueryMsg<D>>,
    _pass: PhantomData<P>,
}

impl<U, P, D> Pipeline<U, P, D>
where
    U: IR + Send + 'static,
    U::Ix: Clone + Send + Sync,
    U::Value: Clone + Send + Sync,
    U::Fault: Send,
    D: IR + Send + Clone + 'static,
    D::Ix: Clone + PartialEq + Send + Sync,
    D::Value: Clone + Send + Sync,
    D::Fault: Send,
    P: Pass<U, D> + 'static,
{
    pub fn connect_with_tap<PF>(
        upstream: LayerObserver<U>,
        make_pass: PF,
        downstream: D,
        tap_sender: Option<channel::Sender<Transaction<D>>>,
    ) -> Self
    where
        PF: FnOnce() -> P + Send + 'static,
        P: Send,
    {
        let (sender, receiver) = channel::unbounded::<Transaction<U>>();
        let (query_sender, query_rx) = channel::unbounded::<QueryMsg<D>>();

        let handle = thread::spawn(move || {
            let mut pass = make_pass();
            let mut downstream = downstream;
            let mut demanded: Vec<D::Ix> = Vec::new();
            loop {
                crossbeam::select! {
                    recv(receiver) -> msg => match msg {
                        Ok(txn) => {
                            let mut combined: Vec<Command<D>> = Vec::new();
                            {
                                let downstream_txn = pass.push(
                                    &upstream,
                                    &downstream,
                                    txn.as_ref(),
                                );
                                if downstream
                                    .apply_transaction(std::sync::Arc::new(downstream_txn.clone()))
                                    .is_ok()
                                {
                                    combined.extend(downstream_txn);
                                }
                            }

                            let mut retained = Vec::with_capacity(demanded.len());
                            for index in demanded.drain(..) {
                                if downstream.query(index.clone()).is_present() {
                                    retained.push(index);
                                    continue;
                                }

                                let should_retain = match pass.pull(
                                    &upstream,
                                    &downstream,
                                    index.clone(),
                                ) {
                                    PullOutcome::Ready(refresh_txn) => {
                                        if downstream
                                            .apply_transaction(std::sync::Arc::new(refresh_txn.clone()))
                                            .is_ok()
                                        {
                                            combined.extend(refresh_txn);
                                        }
                                        true
                                    }
                                    PullOutcome::Pending => true,
                                    PullOutcome::Dead => false,
                                };
                                if should_retain {
                                    retained.push(index);
                                }
                            }
                            demanded = retained;

                            if let Some(tap) = &tap_sender {
                                let _ = tap.send(std::sync::Arc::new(combined));
                            }
                        }
                        Err(_) => break,
                    },
                    recv(query_rx) -> msg => match msg {
                        Ok(QueryMsg { index, strict, reply }) => {
                            let _ = reply.send(resolve_query(&mut pass, &upstream, &mut downstream, &mut demanded, index, strict));
                        }
                        Err(_) => {}
                    },
                }
            }
        });

        Pipeline {
            handle,
            sender,
            query_sender,
            _pass: PhantomData,
        }
    }

    pub fn send(&self, txn: Transaction<U>) {
        let _ = self.sender.send(txn);
    }

    pub fn clone_sender(&self) -> channel::Sender<Transaction<U>> {
        self.sender.clone()
    }

    pub fn downstream_query_handle(&self) -> QueryHandle<D> {
        QueryHandle {
            query_sender: self.query_sender.clone(),
        }
    }

    pub fn shutdown(self) {
        // Closing the sender causes the worker thread to break out of its
        // select loop and drop upstream + downstream + pass.
        drop(self.sender);
        let _ = self.handle.join();
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
        }
    }
}

pub type Transaction<Repr> = std::sync::Arc<Vec<Command<Repr>>>;
