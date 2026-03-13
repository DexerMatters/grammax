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

use super::protocol::{RevisionId, RuntimeError, RuntimeEvent, RuntimePath, RuntimeResult};
use crate::{
    grammar,
    interface::Interface,
    runtime::RuntimeService,
    scheme::{self, IR, Pipeline, QueryHandle, layers::SourceText},
    utils::{self, Span},
};

type SourceTxn = scheme::Transaction<SourceText>;
type QueryFn = Arc<dyn Fn(utils::Payload) -> RuntimeResult<utils::Payload> + Send + Sync>;
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

fn prepend_path(branch: u32, path: RuntimePath) -> RuntimePath {
    let mut segments = Vec::with_capacity(path.0.len() + 1);
    segments.push(branch);
    segments.extend(path.0);
    RuntimePath(segments)
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

pub trait ContainsPath<Path>: TypedTree {
    type Target: IR;

    fn runtime_path() -> RuntimePath;
}

impl<Tree> ContainsPath<Here> for Tree
where
    Tree: TypedTree,
{
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

pub trait ContainsTree<Needle>: TypedTree {}

impl<Hay, U: IR> ContainsTree<End<U>> for Hay where Hay: TypedTree<Current = U> {}

impl<U: IR, P, Left, NeedleLeft> ContainsTree<Then<U, P, NeedleLeft>> for Then<U, P, Left>
where
    Left: ContainsTree<NeedleLeft> + TypedTree,
    NeedleLeft: TypedTree,
{
}

impl<U: IR, P1, Left, P2, Right, NeedleLeft, NeedleRight>
    ContainsTree<Fork<U, P1, NeedleLeft, P2, NeedleRight>> for Fork<U, P1, Left, P2, Right>
where
    Left: ContainsTree<NeedleLeft> + TypedTree,
    Right: ContainsTree<NeedleRight> + TypedTree,
    NeedleLeft: TypedTree,
    NeedleRight: TypedTree,
{
}

pub struct Here;

pub trait SeededTree: TypedTree {
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

trait HasListeners: TypedTree {
    fn listeners(&self) -> SharedListeners<Self::Current>;
}

impl<U: IR> HasListeners for End<U> {
    fn listeners(&self) -> SharedListeners<Self::Current> {
        Arc::clone(&self.listeners)
    }
}

impl<U: IR, P, Left: TypedTree> HasListeners for Then<U, P, Left> {
    fn listeners(&self) -> SharedListeners<Self::Current> {
        Arc::clone(&self.listeners)
    }
}

impl<U: IR, P1, Left: TypedTree, P2, Right: TypedTree> HasListeners
    for Fork<U, P1, Left, P2, Right>
{
    fn listeners(&self) -> SharedListeners<Self::Current> {
        Arc::clone(&self.listeners)
    }
}

pub type ObservedLayer<Tree, Path> = LayerObserver<<Tree as ObservePath<Path>>::Observed>;

pub trait ObservePath<Path>: TypedTree {
    type Observed: IR;

    fn observe_path(&self) -> LayerObserver<Self::Observed>;
}

pub trait Observe: TypedTree {
    fn observe<Path>(&self) -> ObservedLayer<Self, Path>
    where
        Self: ObservePath<Path>;
}

impl<Tree: TypedTree> Observe for Tree {
    fn observe<Path>(&self) -> ObservedLayer<Self, Path>
    where
        Self: ObservePath<Path>,
    {
        <Self as ObservePath<Path>>::observe_path(self)
    }
}

impl<U> ObservePath<Here> for End<U>
where
    U: IR + Send + 'static,
    U::Ix: Send + Sync + 'static,
    U::Value: Send + Sync + 'static,
{
    type Observed = U;

    fn observe_path(&self) -> LayerObserver<U> {
        let (tx, rx) = channel::unbounded();
        let lock = Arc::new(OnceLock::new());
        if let Ok(mut listeners) = self.listeners().lock() {
            listeners.push((tx, Arc::clone(&lock)));
        }
        LayerObserver {
            updates: rx,
            query: lock,
        }
    }
}

impl<U, P, Left> ObservePath<Here> for Then<U, P, Left>
where
    U: IR + Send + 'static,
    U::Ix: Send + Sync + 'static,
    U::Value: Send + Sync + 'static,
    Left: TypedTree,
{
    type Observed = U;

    fn observe_path(&self) -> LayerObserver<U> {
        let (tx, rx) = channel::unbounded();
        let lock = Arc::new(OnceLock::new());
        if let Ok(mut listeners) = self.listeners().lock() {
            listeners.push((tx, Arc::clone(&lock)));
        }
        LayerObserver {
            updates: rx,
            query: lock,
        }
    }
}

impl<U, P1, Left, P2, Right> ObservePath<Here> for Fork<U, P1, Left, P2, Right>
where
    U: IR + Send + 'static,
    U::Ix: Send + Sync + 'static,
    U::Value: Send + Sync + 'static,
    Left: TypedTree,
    Right: TypedTree,
{
    type Observed = U;

    fn observe_path(&self) -> LayerObserver<U> {
        let (tx, rx) = channel::unbounded();
        let lock = Arc::new(OnceLock::new());
        if let Ok(mut listeners) = self.listeners().lock() {
            listeners.push((tx, Arc::clone(&lock)));
        }
        LayerObserver {
            updates: rx,
            query: lock,
        }
    }
}

impl<U, P, Left, Path> ObservePath<Down<Path>> for Then<U, P, Left>
where
    U: IR,
    Left: ObservePath<Path> + TypedTree,
{
    type Observed = <Left as ObservePath<Path>>::Observed;

    fn observe_path(&self) -> LayerObserver<Self::Observed> {
        self.left.observe_path()
    }
}

impl<U, P1, Left, P2, Right, Path> ObservePath<Down<Path>> for Fork<U, P1, Left, P2, Right>
where
    U: IR,
    Left: ObservePath<Path> + TypedTree,
    Right: TypedTree,
{
    type Observed = <Left as ObservePath<Path>>::Observed;

    fn observe_path(&self) -> LayerObserver<Self::Observed> {
        self.left.observe_path()
    }
}

impl<U, P1, Left, P2, Right, Path> ObservePath<Another<Path>> for Fork<U, P1, Left, P2, Right>
where
    U: IR,
    Left: TypedTree,
    Right: ObservePath<Path> + TypedTree,
{
    type Observed = <Right as ObservePath<Path>>::Observed;

    fn observe_path(&self) -> LayerObserver<Self::Observed> {
        self.right.observe_path()
    }
}

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

#[derive(Clone)]
struct BuilderCore {
    submit_top: SubmitTopFn,
    top_layer_path: RuntimePath,
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
            top_layer_path: top_layer_path.clone(),
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

pub struct CompilerBuilder;

impl CompilerBuilder {
    pub fn new() -> End<SourceText> {
        End::new(SourceText::default())
    }
}

impl<U: IR> End<U> {
    pub fn new(seed: U) -> Self {
        Self {
            seed,
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn then<P, D: IR>(self, pass: P, downstream: D) -> Then<U, P, End<D>>
    where
        P: scheme::Pass<U, D>,
    {
        Then {
            seed: self.seed,
            pass,
            left: End::new(downstream),
            listeners: self.listeners,
        }
    }

    pub fn fork<P1, D1: IR, P2, D2: IR>(
        self,
        left_pass: P1,
        left: D1,
        right_pass: P2,
        right: D2,
    ) -> Fork<U, P1, End<D1>, P2, End<D2>>
    where
        P1: scheme::Pass<U, D1>,
        P2: scheme::Pass<U, D2>,
    {
        Fork {
            seed: self.seed,
            left_pass,
            left: End::new(left),
            right_pass,
            right: End::new(right),
            listeners: self.listeners,
        }
    }
}

impl<U: IR, P, Left: TypedTree> Then<U, P, Left> {
    pub fn new(seed: U, pass: P, left: Left) -> Self {
        Self {
            seed,
            pass,
            left,
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn map_left<Next: TypedTree>(self, f: impl FnOnce(Left) -> Next) -> Then<U, P, Next> {
        Then {
            seed: self.seed,
            pass: self.pass,
            left: f(self.left),
            listeners: self.listeners,
        }
    }
}

impl<U: IR, P, D: IR> Then<U, P, End<D>> {
    pub fn then<NewP, NewD: IR>(
        self,
        pass: NewP,
        downstream: NewD,
    ) -> Then<U, P, Then<D, NewP, End<NewD>>>
    where
        NewP: scheme::Pass<D, NewD>,
    {
        self.map_left(|left| left.then(pass, downstream))
    }
}

impl<U: IR, P1, Left: TypedTree, P2, Right: TypedTree> Fork<U, P1, Left, P2, Right> {
    pub fn new(seed: U, left_pass: P1, left: Left, right_pass: P2, right: Right) -> Self {
        Self {
            seed,
            left_pass,
            left,
            right_pass,
            right,
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn map_left<Next: TypedTree>(
        self,
        f: impl FnOnce(Left) -> Next,
    ) -> Fork<U, P1, Next, P2, Right> {
        Fork {
            seed: self.seed,
            left_pass: self.left_pass,
            left: f(self.left),
            right_pass: self.right_pass,
            right: self.right,
            listeners: self.listeners,
        }
    }

    pub fn map_right<Next: TypedTree>(
        self,
        f: impl FnOnce(Right) -> Next,
    ) -> Fork<U, P1, Left, P2, Next> {
        Fork {
            seed: self.seed,
            left_pass: self.left_pass,
            left: self.left,
            right_pass: self.right_pass,
            right: f(self.right),
            listeners: self.listeners,
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
        <Self::Current as IR>::Value: Clone + Send + Sync + Serialize + 'static,
        <Self::Current as IR>::Error: Send + fmt::Debug + 'static;
}

impl<U> InstallTree for End<U>
where
    U: IR + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + Serialize + 'static,
    U::Error: Send + fmt::Debug + 'static,
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
    U::Value: Clone + Send + Sync + Serialize + 'static,
    U::Error: Send + fmt::Debug + 'static,
    Left: InstallTree + SeededTree + TypedTree + 'static,
    Left::Current: IR + Clone + Send + 'static,
    <Left::Current as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    <Left::Current as IR>::Value: Clone + Send + Sync + Serialize + 'static,
    <Left::Current as IR>::Error: Send + fmt::Debug + 'static,
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
    U::Value: Clone + Send + Sync + Serialize + 'static,
    U::Error: Send + fmt::Debug + 'static,
    Left: InstallTree + SeededTree + TypedTree + 'static,
    Left::Current: IR + Clone + Send + 'static,
    <Left::Current as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    <Left::Current as IR>::Value: Clone + Send + Sync + Serialize + 'static,
    <Left::Current as IR>::Error: Send + fmt::Debug + 'static,
    Right: InstallTree + SeededTree + TypedTree + 'static,
    Right::Current: IR + Clone + Send + 'static,
    <Right::Current as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    <Right::Current as IR>::Value: Clone + Send + Sync + Serialize + 'static,
    <Right::Current as IR>::Error: Send + fmt::Debug + 'static,
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

pub trait BuildTree: TypedTree + Sized {
    fn build(self) -> ComposedCompiler<Self>
    where
        Self: TypedTree<Current = SourceText> + 'static,
        SourceText: Send + 'static,
        <SourceText as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        <SourceText as IR>::Value: Clone + Send + Sync + Serialize + 'static,
        <SourceText as IR>::Error: Send + fmt::Debug + 'static;

    fn build_runtime<I>(self, grammar: &'static grammar::Grammar) -> RuntimeService<Self, I>
    where
        I: Interface<Self>,
        Self: TypedTree<Current = SourceText> + Send + 'static,
        SourceText: Send + 'static,
        <SourceText as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        <SourceText as IR>::Value: Clone + Send + Sync + Serialize + 'static,
        <SourceText as IR>::Error: Send + fmt::Debug + 'static;
}

impl<Tree> BuildTree for Tree
where
    Tree: TypedTree + InstallTree,
{
    fn build(self) -> ComposedCompiler<Self>
    where
        Self: TypedTree<Current = SourceText> + 'static,
        SourceText: Send + 'static,
        <SourceText as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        <SourceText as IR>::Value: Clone + Send + Sync + Serialize + 'static,
        <SourceText as IR>::Error: Send + fmt::Debug + 'static,
    {
        ComposedCompiler::from_tree(self)
    }

    fn build_runtime<I>(self, grammar: &'static grammar::Grammar) -> RuntimeService<Self, I>
    where
        I: Interface<Self>,
        Self: TypedTree<Current = SourceText> + Send + 'static,
        SourceText: Send + 'static,
        <SourceText as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        <SourceText as IR>::Value: Clone + Send + Sync + Serialize + 'static,
        <SourceText as IR>::Error: Send + fmt::Debug + 'static,
    {
        RuntimeService::<Self, I>::new(grammar, move |evt_tx| {
            ComposedCompiler::from_tree_with_events(self, evt_tx).into_inner()
        })
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
    U::Value: Clone + Send + Sync + Serialize + 'static,
    U::Error: Send + fmt::Debug + 'static,
    D: IR + Send + 'static,
    D::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    D::Value: Clone + Send + Sync + Serialize + 'static,
    D::Error: Send + fmt::Debug + 'static,
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

    let downstream_query = pipeline.downstream.query_handle();
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

pub struct ComposedCompiler<Tree: TypedTree> {
    submit_top: SubmitTopFn,
    queries: SharedQueries,
    layer_paths: SharedLayerPaths,
    settled_layer_path: RuntimePath,
    settled_pass_path: RuntimePath,
    source_layer_path: RuntimePath,
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
    ) -> RuntimeResult<utils::Payload> {
        let layer_path = layer_path.into();
        let query = self
            .queries
            .lock()
            .ok()
            .and_then(|queries| queries.get(&layer_path).cloned())
            .ok_or_else(|| runtime_invalid(format!("unknown layer path: {layer_path}")))?;

        query(index)
    }

    pub fn source_text(&self) -> Option<String> {
        let span = Span::new(0, self.source_len);
        let index = serde_json::to_value(span).ok()?;
        let payload = self
            .query(self.source_layer_path.clone(), utils::Payload::new(index))
            .ok()?;
        payload.downcast::<String>()
    }

    pub fn layer_paths(&self) -> Vec<RuntimePath> {
        self.layer_paths
            .lock()
            .map(|layers| layers.clone())
            .unwrap_or_default()
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

    fn into_inner(self) -> RawComposedCompiler<Tree> {
        self
    }

    fn from_tree(spec: Tree) -> Self
    where
        Tree: InstallTree + TypedTree<Current = SourceText> + 'static,
        SourceText: Send + 'static,
        <SourceText as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        <SourceText as IR>::Value: Clone + Send + Sync + Serialize + 'static,
        <SourceText as IR>::Error: Send + fmt::Debug + 'static,
    {
        Self::from_tree_with_events(spec, None)
    }

    fn from_tree_with_events(
        spec: Tree,
        event_sender: Option<channel::Sender<RuntimeEvent>>,
    ) -> Self
    where
        Tree: InstallTree + TypedTree<Current = SourceText> + 'static,
        SourceText: Send + 'static,
        <SourceText as IR>::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        <SourceText as IR>::Value: Clone + Send + Sync + Serialize + 'static,
        <SourceText as IR>::Error: Send + fmt::Debug + 'static,
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
            layer_paths: core.layer_paths,
            settled_layer_path,
            settled_pass_path,
            source_layer_path: core.top_layer_path,
            next_revision: AtomicU64::new(1),
            source_len: 0,
            shutdown_hooks: core.shutdown_hooks,
            event_sender: core.event_sender,
            _marker: PhantomData,
        }
    }
}

type RawComposedCompiler<Tree> = ComposedCompiler<Tree>;

impl<Tree: TypedTree> Drop for ComposedCompiler<Tree> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn query_handle_any<R>(
    handle: &QueryHandle<R>,
    index: utils::Payload,
) -> RuntimeResult<utils::Payload>
where
    R: IR,
    R::Ix: DeserializeOwned + Clone + 'static,
    R::Value: Serialize + Send + Sync + 'static,
    R::Error: fmt::Debug,
{
    let typed_index: R::Ix = if let Some(ix) = index.downcast_ref::<R::Ix>() {
        ix.clone()
    } else if let Some(json) = index.downcast_ref::<Value>() {
        serde_json::from_value(json.clone())
            .map_err(|err| runtime_invalid(format!("query index decode failed: {err}")))?
    } else {
        return Err(runtime_invalid(
            "query index type mismatch (expected typed index or serde_json::Value)",
        ));
    };

    let result = handle
        .query(typed_index)
        .ok_or(RuntimeError::ChannelClosed)?;

    let value = result.map_err(|err| runtime_invalid(format!("query failed: {err:?}")))?;

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
    R::Value: Serialize + Clone + Send + Sync + 'static,
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
        payload: utils::Payload::new(serde_json::json!({ "message": msg })),
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
                if index.start != index.end {
                    return Err(runtime_invalid(format!(
                        "invalid insert span: start {} != end {}",
                        index.start, index.end
                    )));
                }
                let frag_len = staged
                    .get(*id)
                    .and_then(|v| *v)
                    .ok_or_else(|| runtime_invalid(format!("unknown staging id: {id}")))?;
                len = len.saturating_add(frag_len);
            }
            scheme::Command::Delete { index } => {
                let span = clamp_span(*index, len);
                len = len.saturating_sub(span.end - span.start);
            }
            scheme::Command::Replace { index, id } => {
                let span = clamp_span(*index, len);
                let frag_len = staged
                    .get(*id)
                    .and_then(|v| *v)
                    .ok_or_else(|| runtime_invalid(format!("unknown staging id: {id}")))?;
                len = len - (span.end - span.start) + frag_len;
            }
            scheme::Command::SetRoot { .. } => {}
        }
    }

    Ok(len)
}

fn clamp_span(span: Span, len: usize) -> Span {
    let start = span.start.min(len);
    let end = span.end.min(len);
    Span::new(start.min(end), end)
}

fn runtime_invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidRequest {
        message: message.into(),
    }
}

pub fn insert_at(offset: usize, text: impl Into<String>) -> SourceTxn {
    let text = text.into();
    let span = Span::new(offset, offset);
    std::sync::Arc::new(vec![
        scheme::Command::Create { id: 0, value: text },
        scheme::Command::Insert { index: span, id: 0 },
    ])
}

pub fn delete_span(span: Span) -> SourceTxn {
    std::sync::Arc::new(vec![scheme::Command::Delete { index: span }])
}

pub fn replace_span(span: Span, text: impl Into<String>) -> SourceTxn {
    let text = text.into();
    std::sync::Arc::new(vec![
        scheme::Command::Create { id: 0, value: text },
        scheme::Command::Replace { index: span, id: 0 },
    ])
}
