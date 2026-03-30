use std::{
    collections::HashMap,
    fmt,
    marker::PhantomData,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use crossbeam::channel;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::protocol::{
    RevisionId, RuntimeError, RuntimeEvent, RuntimePath, RuntimeResult, RuntimeWireResult,
};
use crate::{
    grammar,
    interface::Interface,
    runtime::RuntimeService,
    scheme::{self, IR, Pipeline, QueryHandle, Span, layers::SourceText, passes::Identity},
    utils::{self},
};

type SourceTxn = scheme::Transaction<SourceText>;
type QueryFn = Arc<dyn Fn(utils::Payload) -> RuntimeWireResult<utils::Payload> + Send + Sync>;
type SubmitTopFn = Arc<dyn Fn(RevisionId, SourceTxn) -> RuntimeResult<()> + Send + Sync>;
type ShutdownHook = Box<dyn FnOnce() + Send>;
type SharedQueries = Arc<Mutex<HashMap<RuntimePath, QueryFn>>>;
type SharedLayerPaths = Arc<Mutex<Vec<RuntimePath>>>;
type SharedShutdownHooks = Arc<Mutex<Vec<ShutdownHook>>>;
type SharedEventSender = Arc<OnceLock<channel::Sender<RuntimeEvent>>>;
type ListenerSet<U> = Vec<(
    channel::Sender<(RevisionId, scheme::Transaction<U>)>,
    Arc<OnceLock<QueryHandle<U>>>,
)>;
type SharedListeners<U> = Arc<Mutex<ListenerSet<U>>>;

pub struct End<U: IR> {
    seed: U,
    listeners: SharedListeners<U>,
}

pub struct Then<U: IR, P, Left: TypedTree> {
    seed: U,
    pass: P,
    left: Left,
    listeners: SharedListeners<U>,
}

pub struct Fork<U: IR, P1, Left: TypedTree, P2, Right: TypedTree> {
    seed: U,
    left_pass: P1,
    left: Left,
    right_pass: P2,
    right: Right,
    listeners: SharedListeners<U>,
}

pub struct Down<Path>(PhantomData<fn() -> Path>);
pub struct Another<Path>(PhantomData<fn() -> Path>);
pub struct Here;

pub trait TypedTree {
    type Current: IR;
}

impl<U: IR> TypedTree for End<U> {
    type Current = U;
}

impl<U: IR, P, Left> TypedTree for Then<U, P, Left>
where
    Left: TypedTree,
{
    type Current = U;
}

impl<U: IR, P1, Left, P2, Right> TypedTree for Fork<U, P1, Left, P2, Right>
where
    Left: TypedTree,
    Right: TypedTree,
{
    type Current = U;
}

fn clone_listeners<U: IR>(listeners: &SharedListeners<U>) -> ListenerSet<U> {
    listeners
        .lock()
        .map(|items| {
            items
                .iter()
                .map(|(tx, lock)| (tx.clone(), Arc::clone(lock)))
                .collect()
        })
        .unwrap_or_default()
}

fn prepend_path(branch: u32, path: RuntimePath) -> RuntimePath {
    let mut segments = Vec::with_capacity(path.0.len() + 1);
    segments.push(branch);
    segments.extend(path.0);
    RuntimePath(segments)
}

pub trait ContainsPath<Path>: TypedTree {
    type Target: IR;

    fn runtime_path() -> RuntimePath;
}

impl<Tree: TypedTree> ContainsPath<Here> for Tree {
    type Target = Tree::Current;

    fn runtime_path() -> RuntimePath {
        RuntimePath::root()
    }
}

impl<U: IR, P, Left, Path> ContainsPath<Down<Path>> for Then<U, P, Left>
where
    Left: ContainsPath<Path> + TypedTree,
{
    type Target = <Left as ContainsPath<Path>>::Target;

    fn runtime_path() -> RuntimePath {
        prepend_path(0, <Left as ContainsPath<Path>>::runtime_path())
    }
}

impl<U: IR, P1, Left, P2, Right, Path> ContainsPath<Down<Path>> for Fork<U, P1, Left, P2, Right>
where
    Left: ContainsPath<Path> + TypedTree,
    Right: TypedTree,
{
    type Target = <Left as ContainsPath<Path>>::Target;

    fn runtime_path() -> RuntimePath {
        prepend_path(0, <Left as ContainsPath<Path>>::runtime_path())
    }
}

impl<U: IR, P1, Left, P2, Right, Path> ContainsPath<Another<Path>> for Fork<U, P1, Left, P2, Right>
where
    Left: TypedTree,
    Right: ContainsPath<Path> + TypedTree,
{
    type Target = <Right as ContainsPath<Path>>::Target;

    fn runtime_path() -> RuntimePath {
        prepend_path(1, <Right as ContainsPath<Path>>::runtime_path())
    }
}

trait SeededTree: TypedTree {
    fn seed(&self) -> Self::Current
    where
        Self::Current: Clone;
}

impl<U> SeededTree for End<U>
where
    U: IR + Clone,
{
    fn seed(&self) -> Self::Current {
        self.seed.clone()
    }
}

impl<U, P, Left> SeededTree for Then<U, P, Left>
where
    U: IR + Clone,
    Left: TypedTree,
{
    fn seed(&self) -> Self::Current {
        self.seed.clone()
    }
}

impl<U, P1, Left, P2, Right> SeededTree for Fork<U, P1, Left, P2, Right>
where
    U: IR + Clone,
    Left: TypedTree,
    Right: TypedTree,
{
    fn seed(&self) -> Self::Current {
        self.seed.clone()
    }
}

#[derive(Clone)]
pub struct LayerObserver<U: IR> {
    pub updates: channel::Receiver<(RevisionId, scheme::Transaction<U>)>,
    pub query: Arc<OnceLock<QueryHandle<U>>>,
}

impl<U: IR> LayerObserver<U> {
    pub fn recv_update(&self) -> Option<(RevisionId, scheme::Transaction<U>)> {
        self.updates.recv().ok()
    }

    pub fn recv(&self) -> Option<scheme::Transaction<U>> {
        self.recv_update().map(|(_, txn)| txn)
    }

    pub fn try_recv_update(&self) -> Option<(RevisionId, scheme::Transaction<U>)> {
        self.updates.try_recv().ok()
    }

    pub fn try_recv(&self) -> Option<scheme::Transaction<U>> {
        self.try_recv_update().map(|(_, txn)| txn)
    }

    pub fn query(&self, index: U::Ix) -> Result<U::Value, Result<U::Error, RuntimeError>> {
        match self.query.get() {
            None => Err(Err(runtime_invalid("query handle not set".to_string()))),
            Some(handle) => match handle.query(index) {
                None => Err(Err(runtime_invalid("query failed".to_string()))),
                Some(result) => match result {
                    Ok(value) => Ok(value),
                    Err(err) => Err(Ok(err)),
                },
            },
        }
    }
}

fn make_observer<U>(listeners: &SharedListeners<U>) -> LayerObserver<U>
where
    U: IR + Send + 'static,
    U::Ix: Send + Sync + 'static,
    U::Value: Send + Sync + 'static,
{
    let (tx, rx) = channel::unbounded();
    let lock = Arc::new(OnceLock::new());
    if let Ok(mut ls) = listeners.lock() {
        ls.push((tx, Arc::clone(&lock)));
    }
    LayerObserver {
        updates: rx,
        query: lock,
    }
}

// An identity pass used by `fanout` — forwards transactions unchanged (U → U).
// This lets each branch describe its full pipeline from the fork point rather
// than requiring a split pass factory. `Arc` makes the clone O(1).

// ---------------------------------------------------------------------------
// Build<Tree> — CPS builder
//
// `.then(factory, seed, |b, obs| ...)` passes the new Build and its
// LayerObserver into a continuation that returns R.  All previous observers
// are already in the enclosing closure scope — no HList, no tuples.
//
// Usage:
//   Build::new()
//       .then(|| CstPass::new(), ParseTreeIR::new(), |b, cst_obs| {
//           b.then(|| AstPass::new(), AstArena::new(), |b, ast_obs| {
//               b.build_runtime(grammar)   // R = RuntimeService<...>
//           })
//       })
// ---------------------------------------------------------------------------

pub struct Build<Tree: TypedTree>(Tree);

impl Build<End<SourceText>> {
    pub fn new() -> Self {
        Self(End::new(SourceText::default()))
    }
}

impl<Tree: TypedTree> Build<Tree> {
    pub fn finish(self) -> Tree {
        self.0
    }
}

#[allow(private_bounds)]
impl<Tree: TypedTree + InstallTree + TypedTree<Current = SourceText> + Send + 'static> Build<Tree> {
    pub fn build_runtime<I>(self, grammar: &'static grammar::Grammar) -> RuntimeService<Tree, I>
    where
        I: Interface<Tree>,
    {
        RuntimeService::<Tree, I>::new(grammar, move |evt_tx| {
            ComposedCompiler::from_tree_with_events(self.0, evt_tx)
        })
    }
}

impl<U> Build<End<U>>
where
    U: IR + Clone + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + 'static,
    U::Error: Send + Sync + fmt::Debug + 'static,
{
    pub fn then<F, P, D, K, R>(self, factory: F, downstream: D, k: K) -> R
    where
        D: IR + Clone + Send + 'static,
        D::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        D::Value: Clone + Send + Sync + 'static,
        D::Error: Send + Sync + fmt::Debug + 'static,
        P: scheme::Pass<U, D>,
        F: FnOnce() -> P,
        K: FnOnce(Build<Then<U, P, End<D>>>, LayerObserver<D>) -> R,
    {
        let downstream_end = End::new(downstream);
        let obs = make_observer(&downstream_end.listeners);
        let tree = Then {
            seed: self.0.seed,
            pass: factory(),
            left: downstream_end,
            listeners: self.0.listeners,
        };
        k(Build(tree), obs)
    }

    pub fn fork<F1, P1, D1, F2, P2, D2, KL, KR, LTree, RTree>(
        self,
        left_pass_factory: F1,
        left_seed: D1,
        right_pass_factory: F2,
        right_seed: D2,
        left_k: KL,
        right_k: KR,
    ) -> Build<Fork<U, P1, LTree, P2, RTree>>
    where
        D1: IR + Clone + Send + 'static,
        D1::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        D1::Value: Clone + Send + Sync + 'static,
        D1::Error: Send + Sync + fmt::Debug + 'static,
        D2: IR + Clone + Send + 'static,
        D2::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        D2::Value: Clone + Send + Sync + 'static,
        D2::Error: Send + Sync + fmt::Debug + 'static,
        LTree: TypedTree,
        RTree: TypedTree,
        P1: scheme::Pass<U, LTree::Current>,
        P2: scheme::Pass<U, RTree::Current>,
        F1: FnOnce() -> P1,
        F2: FnOnce() -> P2,
        KL: FnOnce(Build<End<D1>>) -> Build<LTree>,
        KR: FnOnce(Build<End<D2>>) -> Build<RTree>,
    {
        let lb = left_k(Build(End::new(left_seed)));
        let rb = right_k(Build(End::new(right_seed)));
        Build(Fork {
            seed: self.0.seed,
            left_pass: left_pass_factory(),
            left: lb.0,
            right_pass: right_pass_factory(),
            right: rb.0,
            listeners: self.0.listeners,
        })
    }

    /// Arrow-style fanout `(f &&& g)`: both branches independently describe
    /// their full pipeline starting from the same `U`.
    ///
    /// This is strictly more monadic than `fork` — there are no split pass
    /// factories; each branch is a self-contained pipeline description.
    /// Requires `U: Clone` (already imposed by `Fork`'s runtime).
    ///
    /// ```
    /// b.fanout(
    ///     |b| b.then(|| pass_AB, seedB, |b, _| b.then(|| pass_BD, seedD, |b, _| b)),
    ///     |b| b.then(|| pass_AC, seedC, |b, _| b.then(|| pass_CE, seedE, |b, _| b)),
    /// )
    /// ```
    #[allow(private_interfaces)]
    pub fn fanout<KL, KR, LTree, RTree>(
        self,
        left_k: KL,
        right_k: KR,
    ) -> Build<Fork<U, Identity, LTree, Identity, RTree>>
    where
        U: Clone,
        LTree: TypedTree<Current = U>,
        RTree: TypedTree<Current = U>,
        KL: FnOnce(Build<End<U>>) -> Build<LTree>,
        KR: FnOnce(Build<End<U>>) -> Build<RTree>,
    {
        let seed = self.0.seed;
        let lb = left_k(Build(End::new(seed.clone())));
        let rb = right_k(Build(End::new(seed.clone())));
        Build(Fork {
            seed,
            left_pass: Identity,
            left: lb.0,
            right_pass: Identity,
            right: rb.0,
            listeners: self.0.listeners,
        })
    }
}

impl<U, P, D> Build<Then<U, P, End<D>>>
where
    U: IR + Clone + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + 'static,
    U::Error: Send + Sync + fmt::Debug + 'static,
    D: IR + Clone + Send + 'static,
    D::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    D::Value: Clone + Send + Sync + 'static,
    D::Error: Send + Sync + fmt::Debug + 'static,
    P: scheme::Pass<U, D>,
{
    pub fn then<F, NewP, NewD, K, R>(self, factory: F, downstream: NewD, k: K) -> R
    where
        NewD: IR + Clone + Send + 'static,
        NewD::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        NewD::Value: Clone + Send + Sync + 'static,
        NewD::Error: Send + Sync + fmt::Debug + 'static,
        NewP: scheme::Pass<D, NewD>,
        F: FnOnce() -> NewP,
        K: FnOnce(Build<Then<U, P, Then<D, NewP, End<NewD>>>>, LayerObserver<NewD>) -> R,
    {
        let downstream_end = End::new(downstream);
        let obs = make_observer(&downstream_end.listeners);
        let new_left = Then {
            seed: self.0.left.seed,
            pass: factory(),
            left: downstream_end,
            listeners: self.0.left.listeners,
        };
        let tree = Then {
            seed: self.0.seed,
            pass: self.0.pass,
            left: new_left,
            listeners: self.0.listeners,
        };
        k(Build(tree), obs)
    }

    /// Fork the downstream end of this chain into two independent branches.
    ///
    /// ```
    /// O → (pass A) → A   ← this Build<Then<O, A, End<D>>>
    ///                 |  |
    ///               B    C   ← left_pass_factory / right_pass_factory
    ///               |    |
    ///             ...   ...  ← continued by left_k / right_k
    /// ```
    pub fn fork<F1, P1, D1, F2, P2, D2, KL, KR, LTree, RTree>(
        self,
        left_pass_factory: F1,
        left_seed: D1,
        right_pass_factory: F2,
        right_seed: D2,
        left_k: KL,
        right_k: KR,
    ) -> Build<Then<U, P, Fork<D, P1, LTree, P2, RTree>>>
    where
        D1: IR + Clone + Send + 'static,
        D1::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        D1::Value: Clone + Send + Sync + 'static,
        D1::Error: Send + Sync + fmt::Debug + 'static,
        D2: IR + Clone + Send + 'static,
        D2::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        D2::Value: Clone + Send + Sync + 'static,
        D2::Error: Send + Sync + fmt::Debug + 'static,
        LTree: TypedTree,
        RTree: TypedTree,
        P1: scheme::Pass<D, LTree::Current>,
        P2: scheme::Pass<D, RTree::Current>,
        F1: FnOnce() -> P1,
        F2: FnOnce() -> P2,
        KL: FnOnce(Build<End<D1>>) -> Build<LTree>,
        KR: FnOnce(Build<End<D2>>) -> Build<RTree>,
    {
        let lb = left_k(Build(End::new(left_seed)));
        let rb = right_k(Build(End::new(right_seed)));
        let fork = Fork {
            seed: self.0.left.seed,
            left_pass: left_pass_factory(),
            left: lb.0,
            right_pass: right_pass_factory(),
            right: rb.0,
            listeners: self.0.left.listeners,
        };
        Build(Then {
            seed: self.0.seed,
            pass: self.0.pass,
            left: fork,
            listeners: self.0.listeners,
        })
    }

    /// Arrow-style fanout `(f &&& g)` on the downstream end of this chain.
    /// Both branches independently describe their full pipeline from `D`.
    /// Requires `D: Clone` (already imposed by `Fork`'s runtime).
    pub fn fanout<KL, KR, LTree, RTree>(
        self,
        left_k: KL,
        right_k: KR,
    ) -> Build<Then<U, P, Fork<D, Identity, LTree, Identity, RTree>>>
    where
        D: Clone,
        LTree: TypedTree<Current = D>,
        RTree: TypedTree<Current = D>,
        KL: FnOnce(Build<End<D>>) -> Build<LTree>,
        KR: FnOnce(Build<End<D>>) -> Build<RTree>,
    {
        let seed = self.0.left.seed;
        let lb = left_k(Build(End::new(seed.clone())));
        let rb = right_k(Build(End::new(seed.clone())));
        let fork = Fork {
            seed,
            left_pass: Identity,
            left: lb.0,
            right_pass: Identity,
            right: rb.0,
            listeners: self.0.left.listeners,
        };
        Build(Then {
            seed: self.0.seed,
            pass: self.0.pass,
            left: fork,
            listeners: self.0.listeners,
        })
    }
}

#[derive(Clone)]
struct BuilderCore {
    submit_top: SubmitTopFn,
    queries: SharedQueries,
    layer_paths: SharedLayerPaths,
    settled: std::cell::RefCell<(RuntimePath, RuntimePath)>,
    shutdown_hooks: SharedShutdownHooks,
    event_sender: SharedEventSender,
}

impl BuilderCore {
    fn new(
        submit_top: SubmitTopFn,
        top_layer_path: RuntimePath,
        top_pass_path: RuntimePath,
        event_sender: SharedEventSender,
    ) -> Self {
        Self {
            submit_top,
            queries: Arc::new(Mutex::new(HashMap::new())),
            layer_paths: Arc::new(Mutex::new(vec![top_layer_path.clone()])),
            settled: std::cell::RefCell::new((top_layer_path, top_pass_path)),
            shutdown_hooks: Arc::new(Mutex::new(Vec::new())),
            event_sender,
        }
    }

    fn insert_query(&self, layer_path: RuntimePath, query: QueryFn) {
        if let Ok(mut queries) = self.queries.lock() {
            queries.insert(layer_path, query);
        }
    }

    fn push_layer(&self, layer_path: RuntimePath) {
        if let Ok(mut layers) = self.layer_paths.lock() {
            if !layers.contains(&layer_path) {
                layers.push(layer_path);
            }
        }
    }

    fn set_settled(&self, layer_path: RuntimePath, pass_path: RuntimePath) {
        *self.settled.borrow_mut() = (layer_path, pass_path);
    }

    fn push_shutdown_hook(&self, hook: ShutdownHook) {
        if let Ok(mut hooks) = self.shutdown_hooks.lock() {
            hooks.push(hook);
        }
    }

    fn settled_snapshot(&self) -> (RuntimePath, RuntimePath) {
        self.settled.borrow().clone()
    }
}

impl<U: IR> End<U> {
    fn new(seed: U) -> Self {
        Self {
            seed,
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

trait InstallTree: TypedTree + Sized {
    fn install(
        self,
        core: &BuilderCore,
        input_rx: channel::Receiver<(RevisionId, scheme::Transaction<Self::Current>)>,
        layer_path: RuntimePath,
        current_query: Option<QueryHandle<Self::Current>>,
    ) where
        Self::Current: Send + 'static,
        SourceText: Send + 'static,
        <Self::Current as IR>::Value: Clone + Send + Sync + 'static,
        <Self::Current as IR>::Error: Send + Sync + fmt::Debug + 'static;
}

impl<U> InstallTree for End<U>
where
    U: IR + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + 'static,
    U::Error: Send + Sync + fmt::Debug + 'static,
{
    fn install(
        self,
        core: &BuilderCore,
        input_rx: channel::Receiver<(RevisionId, scheme::Transaction<U>)>,
        layer_path: RuntimePath,
        current_query: Option<QueryHandle<U>>,
    ) {
        if let Some(qh) = &current_query {
            for (_, lock) in &clone_listeners(&self.listeners) {
                let _ = lock.set(qh.clone());
            }
        }

        let listeners = clone_listeners(&self.listeners);
        if !listeners.is_empty() {
            let (stop_tx, stop_rx) = channel::unbounded::<()>();
            let handle = thread::spawn(move || {
                loop {
                    crossbeam::select! {
                        recv(stop_rx) -> _ => break,
                        recv(input_rx) -> msg => match msg {
                            Ok((revision, txn)) => {
                                for (tx, _) in &listeners {
                                    let _ = tx.send((revision, Arc::clone(&txn)));
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            });

            core.push_shutdown_hook(Box::new(move || {
                let _ = stop_tx.send(());
                let _ = handle.join();
            }));
        }

        core.push_layer(layer_path);
    }
}

impl<U, P, Left> InstallTree for Then<U, P, Left>
where
    U: IR + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + 'static,
    U::Error: Send + Sync + fmt::Debug + 'static,
    Left: InstallTree + SeededTree + TypedTree + 'static,
    Left::Current: IR + Clone + Send + 'static,
    <Left::Current as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    <Left::Current as IR>::Value: Clone + Send + Sync + 'static,
    <Left::Current as IR>::Error: Send + Sync + fmt::Debug + 'static,
    P: scheme::Pass<U, Left::Current> + Send + 'static,
    P::Error: Send + fmt::Debug + 'static,
{
    fn install(
        self,
        core: &BuilderCore,
        input_rx: channel::Receiver<(RevisionId, scheme::Transaction<U>)>,
        layer_path: RuntimePath,
        _current_query: Option<QueryHandle<U>>,
    ) {
        let downstream_layer_path = layer_path.child(0);
        let downstream_seed = self.left.seed();
        let (output_rx, downstream_query) = connect_stage::<U, Left::Current, P>(
            core,
            input_rx,
            self.seed,
            layer_path,
            downstream_layer_path.clone(),
            downstream_layer_path.clone(),
            self.pass,
            downstream_seed,
            self.listeners,
        );
        self.left.install(
            core,
            output_rx,
            downstream_layer_path,
            Some(downstream_query),
        );
    }
}

impl<U, P1, Left, P2, Right> InstallTree for Fork<U, P1, Left, P2, Right>
where
    U: IR + Clone + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + 'static,
    U::Error: Send + Sync + fmt::Debug + 'static,
    Left: InstallTree + SeededTree + TypedTree + 'static,
    Left::Current: IR + Clone + Send + 'static,
    <Left::Current as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    <Left::Current as IR>::Value: Clone + Send + Sync + 'static,
    <Left::Current as IR>::Error: Send + Sync + fmt::Debug + 'static,
    Right: InstallTree + SeededTree + TypedTree + 'static,
    Right::Current: IR + Clone + Send + 'static,
    <Right::Current as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    <Right::Current as IR>::Value: Clone + Send + Sync + 'static,
    <Right::Current as IR>::Error: Send + Sync + fmt::Debug + 'static,
    P1: scheme::Pass<U, Left::Current> + Send + 'static,
    P1::Error: Send + fmt::Debug + 'static,
    P2: scheme::Pass<U, Right::Current> + Send + 'static,
    P2::Error: Send + fmt::Debug + 'static,
{
    fn install(
        self,
        core: &BuilderCore,
        input_rx: channel::Receiver<(RevisionId, scheme::Transaction<U>)>,
        layer_path: RuntimePath,
        current_query: Option<QueryHandle<U>>,
    ) {
        if let Some(qh) = &current_query {
            for (_, lock) in &clone_listeners(&self.listeners) {
                let _ = lock.set(qh.clone());
            }
        }

        let (tx1, rx1) = channel::unbounded::<(RevisionId, scheme::Transaction<U>)>();
        let (tx2, rx2) = channel::unbounded::<(RevisionId, scheme::Transaction<U>)>();
        let listeners = clone_listeners(&self.listeners);
        let (fanout_stop_tx, fanout_stop_rx) = channel::unbounded::<()>();
        let fanout_handle = thread::spawn(move || {
            loop {
                crossbeam::select! {
                    recv(fanout_stop_rx) -> _ => break,
                    recv(input_rx) -> msg => match msg {
                        Ok((revision, txn)) => {
                            for (tx, _) in &listeners {
                                let _ = tx.send((revision, Arc::clone(&txn)));
                            }
                            let _ = tx1.send((revision, Arc::clone(&txn)));
                            let _ = tx2.send((revision, txn));
                        }
                        Err(_) => break,
                    }
                }
            }
        });
        core.push_shutdown_hook(Box::new(move || {
            let _ = fanout_stop_tx.send(());
            let _ = fanout_handle.join();
        }));

        let left_layer_path = layer_path.child(0);
        let right_layer_path = layer_path.child(1);

        let left_seed = self.left.seed();
        let (left_rx, left_query) = connect_stage::<U, Left::Current, P1>(
            core,
            rx1,
            self.seed.clone(),
            layer_path.clone(),
            left_layer_path.clone(),
            left_layer_path.clone(),
            self.left_pass,
            left_seed,
            Arc::new(Mutex::new(Vec::new())),
        );
        self.left
            .install(core, left_rx, left_layer_path, Some(left_query));

        let right_seed = self.right.seed();
        let (right_rx, right_query) = connect_stage::<U, Right::Current, P2>(
            core,
            rx2,
            self.seed,
            layer_path,
            right_layer_path.clone(),
            right_layer_path.clone(),
            self.right_pass,
            right_seed,
            Arc::new(Mutex::new(Vec::new())),
        );
        self.right
            .install(core, right_rx, right_layer_path, Some(right_query));
    }
}

fn connect_stage<U, D, P>(
    core: &BuilderCore,
    input_rx: channel::Receiver<(RevisionId, scheme::Transaction<U>)>,
    upstream_seed: U,
    upstream_layer_path: RuntimePath,
    downstream_layer_path: RuntimePath,
    pass_path: RuntimePath,
    pass: P,
    downstream: D,
    upstream_listeners: SharedListeners<U>,
) -> (
    channel::Receiver<(RevisionId, scheme::Transaction<D>)>,
    QueryHandle<D>,
)
where
    U: IR + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + 'static,
    U::Error: Send + Sync + fmt::Debug + 'static,
    D: IR + Send + 'static,
    D::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    D::Value: Clone + Send + Sync + 'static,
    D::Error: Send + Sync + fmt::Debug + 'static,
    P: scheme::Pass<U, D> + Send + 'static,
    P::Error: Send + fmt::Debug + 'static,
{
    let (tap_tx, tap_rx) = channel::unbounded::<scheme::Transaction<D>>();

    let pipeline = Pipeline::connect_with_tap(
        move || upstream_seed,
        move || pass,
        downstream,
        Some(tap_tx),
    );

    // Populate query handles for all upstream observers now that the IR is live.
    let upstream_query = pipeline.upstream_query_handle();
    for (_, lock) in &clone_listeners(&upstream_listeners) {
        let _ = lock.set(upstream_query.clone());
    }
    core.insert_query(
        upstream_layer_path,
        Arc::new(move |index| query_handle_any::<U>(&upstream_query, index)),
    );

    let downstream_query = pipeline.downstream_query_handle();
    core.insert_query(
        downstream_layer_path.clone(),
        Arc::new({
            let dq = downstream_query.clone();
            move |index| query_handle_any::<D>(&dq, index)
        }),
    );

    let pipeline_sender = pipeline.clone_sender();
    let (next_output_tx, next_output_rx) =
        channel::unbounded::<(RevisionId, scheme::Transaction<D>)>();

    // The relay and bridge threads are tightly coupled in lockstep: relay sends
    // one txn to the pipeline, bridge waits for the corresponding tap output.
    // Merging them into a single coordinator thread eliminates one OS context
    // switch per operation (and the intermediate fwd channel), which meaningfully
    // reduces round-trip latency for incremental edits.
    let event_sender = Arc::clone(&core.event_sender);
    let coord_layer_path = downstream_layer_path.clone();
    let coord_pass_path = pass_path.clone();
    let (coord_stop_tx, coord_stop_rx) = channel::unbounded::<()>();
    let coord_handle = thread::spawn(move || {
        loop {
            // Wait for the next input txn (or a stop signal).
            let (revision, txn) = crossbeam::select! {
                recv(coord_stop_rx) -> _ => break,
                recv(input_rx) -> msg => match msg {
                    Ok(item) => item,
                    Err(_) => break,
                },
            };

            // Hand the txn to the pipeline worker.
            if pipeline_sender.send(Arc::clone(&txn)).is_err() {
                break;
            }

            // Wait for the pipeline to emit its tap output for this txn.
            // Because the pipeline processes txns in FIFO order, tap_rx always
            // yields results in the same order we submitted them.
            let downstream_txn = match tap_rx.recv() {
                Ok(t) => t,
                Err(_) => break,
            };

            // Deliver to upstream-layer observers.
            for (tx, _) in &clone_listeners(&upstream_listeners) {
                let _ = tx.send((revision, Arc::clone(&txn)));
            }

            send_layer_event::<D>(
                &event_sender,
                revision,
                coord_layer_path.clone(),
                coord_pass_path.clone(),
                false,
                &downstream_txn,
            );

            if next_output_tx.send((revision, downstream_txn)).is_err() {
                break;
            }
        }
    });

    core.push_shutdown_hook(Box::new(move || {
        let _ = coord_stop_tx.send(());
        pipeline.shutdown();
        let _ = coord_handle.join();
    }));

    core.push_layer(downstream_layer_path.clone());
    core.set_settled(downstream_layer_path, pass_path);

    (next_output_rx, downstream_query)
}

pub(crate) struct ComposedCompiler<Tree: TypedTree> {
    submit_top: SubmitTopFn,
    queries: SharedQueries,
    settled_layer_path: RuntimePath,
    settled_pass_path: RuntimePath,
    next_revision: AtomicU64,
    source_len: usize,
    shutdown_hooks: SharedShutdownHooks,
    event_sender: SharedEventSender,
    _marker: PhantomData<fn() -> Tree>,
}

impl<Tree: TypedTree> ComposedCompiler<Tree> {
    pub fn submit_source(&mut self, txn: SourceTxn) -> RuntimeResult<RevisionId> {
        let next_len = validate_source_txn_len(self.source_len, &txn)?;
        let revision = self.next_revision.fetch_add(1, Ordering::Relaxed);

        if let Err(err) = (self.submit_top)(revision, txn) {
            send_runtime_error(&self.event_sender, revision, &err);
            return Err(err);
        }

        self.source_len = next_len;
        Ok(revision)
    }

    /// Query a layer and return the result as a type-erased [`Payload`].
    pub(crate) fn query(
        &self,
        layer_path: impl Into<RuntimePath>,
        index: utils::Payload,
    ) -> RuntimeWireResult<utils::Payload> {
        let layer_path = layer_path.into();
        let query = self
            .queries
            .lock()
            .ok()
            .and_then(|queries| queries.get(&layer_path).cloned())
            .ok_or_else(|| runtime_invalid(format!("unknown layer path: {layer_path}")))?;

        query(index)
    }

    pub fn settled_layer_path(&self) -> Option<&RuntimePath> {
        Some(&self.settled_layer_path)
    }

    pub fn settled_pass_path(&self) -> Option<&RuntimePath> {
        Some(&self.settled_pass_path)
    }

    pub fn shutdown(&mut self) {
        if let Ok(mut hooks) = self.shutdown_hooks.lock() {
            while let Some(hook) = hooks.pop() {
                hook();
            }
        }

        // OnceLock does not support clearing once set, so this is a no-op
        // The sender will be dropped naturally when ComposedCompiler is dropped
    }

    fn from_tree_with_events(
        spec: Tree,
        event_sender: Option<channel::Sender<RuntimeEvent>>,
    ) -> Self
    where
        Tree: InstallTree + TypedTree<Current = SourceText> + 'static,
        SourceText: Send + 'static,
        <SourceText as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        <SourceText as IR>::Value: Clone + Send + Sync + 'static,
        <SourceText as IR>::Error: Send + Sync + fmt::Debug + 'static,
    {
        let layer_path = RuntimePath::root();
        let pass_path = RuntimePath::root();
        let submit_layer_path = layer_path.clone();
        let submit_pass_path = pass_path.clone();

        let shared_event_sender: SharedEventSender = Arc::new(OnceLock::new());
        if let Some(sender) = event_sender {
            let _ = shared_event_sender.set(sender);
        }
        let submit_event_sender = Arc::clone(&shared_event_sender);

        let (output_tx, output_rx) = channel::unbounded::<(RevisionId, SourceTxn)>();
        let submit_top: SubmitTopFn = Arc::new(move |revision, txn| {
            send_layer_event::<SourceText>(
                &submit_event_sender,
                revision,
                submit_layer_path.clone(),
                submit_pass_path.clone(),
                false,
                &txn,
            );

            output_tx
                .send((revision, txn))
                .map_err(|_| RuntimeError::ChannelClosed)
        });

        let core = BuilderCore::new(
            submit_top,
            layer_path.clone(),
            pass_path,
            shared_event_sender,
        );
        spec.install(&core, output_rx, layer_path, None);
        let (settled_layer_path, settled_pass_path) = core.settled_snapshot();

        ComposedCompiler {
            submit_top: core.submit_top,
            queries: core.queries,
            settled_layer_path,
            settled_pass_path,
            next_revision: AtomicU64::new(1),
            source_len: 0,
            shutdown_hooks: core.shutdown_hooks,
            event_sender: core.event_sender,
            _marker: PhantomData,
        }
    }
}

impl<Tree: TypedTree> Drop for ComposedCompiler<Tree> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn query_handle_any<R>(
    handle: &QueryHandle<R>,
    index: utils::Payload,
) -> RuntimeWireResult<utils::Payload>
where
    R: IR,
    R::Ix: DeserializeOwned + Clone + 'static,
    R::Value: Send + Sync + 'static,
    R::Error: Send + Sync + 'static,
{
    let typed_index: R::Ix = if let Some(ix) = index.downcast_ref::<R::Ix>() {
        ix.clone()
    } else if let Some(json) = index.downcast_ref::<Value>() {
        serde_json::from_value(json.clone()).map_err(|err| {
            runtime_invalid::<utils::Payload>(format!("query index decode failed: {err}"))
        })?
    } else {
        return Err(runtime_invalid::<utils::Payload>(
            "query index type mismatch (expected typed index or serde_json::Value)",
        ));
    };

    let result = handle
        .query(typed_index)
        .ok_or(RuntimeError::ChannelClosed)?;

    let value = result.map_err(|err| RuntimeError::InvalidRequestFromTarget {
        err: utils::Payload::new(err),
    })?;

    Ok(utils::Payload::new(value))
}

fn clone_event_sender(shared: &SharedEventSender) -> Option<channel::Sender<RuntimeEvent>> {
    shared.get().cloned()
}

fn send_layer_event<R>(
    sender: &SharedEventSender,
    revision: RevisionId,
    layer_path: RuntimePath,
    pass_path: RuntimePath,
    is_error: bool,
    txn: &scheme::Transaction<R>,
) where
    R: IR + 'static,
    R::Ix: Serialize + Clone + Send + Sync + 'static,
    R::Value: Clone + Send + Sync + 'static,
{
    let Some(sender) = clone_event_sender(sender) else {
        return;
    };

    // Clone the commands field-by-field; this only needs R::Ix: Clone and
    // R::Value: Clone (not R: Clone).  Wrap in Arc so the payload type matches
    // `scheme::Transaction<R>` and `SubscriptionHandle::rev_as` can downcast it.
    let commands: scheme::Transaction<R> =
        std::sync::Arc::new(txn.as_ref().iter().map(|cmd| cmd.clone_fields()).collect());
    let payload = utils::Payload::new(commands);
    let _ = sender.send(RuntimeEvent {
        revision,
        layer_path,
        pass_path,
        is_error,
        payload,
    });
}

fn send_runtime_error(sender: &SharedEventSender, revision: RevisionId, err: &RuntimeError) {
    send_runtime_error_text(
        &clone_event_sender(sender),
        revision,
        format!("runtime error: {err}"),
    );
}

fn send_runtime_error_text(
    sender: &Option<channel::Sender<RuntimeEvent>>,
    revision: RevisionId,
    message: impl Into<String>,
) {
    let Some(sender) = sender.as_ref() else {
        return;
    };

    let msg = message.into();
    let _ = sender.send(RuntimeEvent {
        revision,
        layer_path: RuntimePath::root(),
        pass_path: RuntimePath::root(),
        is_error: true,
        payload: utils::Payload::new_serializable(serde_json::json!({ "message": msg })),
    });
}

fn validate_source_txn_len(
    current_len: usize,
    txn: &[scheme::Command<SourceText>],
) -> RuntimeResult<usize> {
    let mut len = current_len;
    let mut staged: Vec<Option<usize>> = Vec::new();

    for command in txn {
        match command {
            scheme::Command::Create { id, value } => {
                if *id >= staged.len() {
                    staged.resize(*id + 1, None);
                }
                staged[*id] = Some(value.len());
            }
            scheme::Command::Insert { index, id } => {
                if index.span.start != index.span.end {
                    return Err(runtime_invalid(format!(
                        "invalid insert span: start {} != end {}",
                        index.span.start, index.span.end
                    )));
                }
                let frag_len = staged
                    .get(*id)
                    .and_then(|v| *v)
                    .ok_or_else(|| runtime_invalid(format!("unknown staging id: {id}")))?;
                len = len.saturating_add(frag_len);
            }
            scheme::Command::Delete { index } => {
                let span = clamp_span(index.span, len);
                len = len.saturating_sub(span.end - span.start);
            }
            scheme::Command::Replace { index, id } => {
                let span = clamp_span(index.span, len);
                let frag_len = staged
                    .get(*id)
                    .and_then(|v| *v)
                    .ok_or_else(|| runtime_invalid(format!("unknown staging id: {id}")))?;
                len = len - (span.end - span.start) + frag_len;
            }
        }
    }

    Ok(len)
}

fn clamp_span(span: Span, len: usize) -> Span {
    let start = span.start.min(len);
    let end = span.end.min(len);
    Span::new(start.min(end), end)
}

fn runtime_invalid<Err>(message: impl Into<String>) -> RuntimeError<Err> {
    RuntimeError::InvalidRequest {
        message: message.into(),
    }
}
