use std::{
    cell::Cell,
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
use serde_json::{Value, json};

use super::protocol::{RevisionId, RuntimeError, RuntimeEvent, RuntimeResult};
use crate::{
    scheme::{self, IR, LayerName, PassId, Pipeline, QueryHandle, layers::SourceText},
    utils::Span,
};

type SourceTxn = scheme::Transaction<SourceText>;
type QueryFn = Arc<dyn Fn(Value) -> RuntimeResult<Value> + Send + Sync>;
type SubmitTopFn = Arc<dyn Fn(RevisionId, SourceTxn) -> RuntimeResult<()> + Send + Sync>;
type ShutdownHook = Box<dyn FnOnce() + Send>;
type SharedQueries = Arc<Mutex<HashMap<LayerName, QueryFn>>>;
type SharedLayerNames = Arc<Mutex<Vec<LayerName>>>;
type SharedShutdownHooks = Arc<Mutex<Vec<ShutdownHook>>>;
type SharedEventSender = Arc<OnceLock<channel::Sender<RuntimeEvent>>>;

type ListenerSet<U> = Vec<(
    channel::Sender<(RevisionId, scheme::Transaction<U>)>,
    Arc<OnceLock<QueryHandle<U>>>,
)>;

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

    pub fn query_handle(&self) -> Option<&QueryHandle<U>> {
        self.query.get()
    }
}

#[derive(Clone)]
struct IdAllocator {
    seed: u64,
    next: Arc<AtomicU64>,
}

impl IdAllocator {
    fn new() -> Self {
        Self {
            seed: 0x243f_6a88_85a3_08d3,
            next: Arc::new(AtomicU64::new(1)),
        }
    }

    fn next_layer(&self) -> LayerName {
        LayerName::new(mix64(self.seed ^ 0x4c41_5945_525f_4944 ^ self.bump()))
    }

    fn next_pass(&self) -> PassId {
        PassId::new(mix64(self.seed ^ 0x5041_5353_5f49_4400 ^ self.bump()))
    }

    fn bump(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Clone)]
struct BuilderCore {
    submit_top: SubmitTopFn,
    top_layer: LayerName,
    queries: SharedQueries,
    layer_names: SharedLayerNames,
    settled: Cell<(LayerName, PassId)>,
    shutdown_hooks: SharedShutdownHooks,
    event_sender: SharedEventSender,
    ids: IdAllocator,
}

impl BuilderCore {
    fn new(
        submit_top: SubmitTopFn,
        top_layer: LayerName,
        top_milestone: PassId,
        event_sender: SharedEventSender,
        ids: IdAllocator,
    ) -> Self {
        Self {
            submit_top,
            top_layer: top_layer.clone(),
            queries: Arc::new(Mutex::new(HashMap::new())),
            layer_names: Arc::new(Mutex::new(vec![top_layer.clone()])),
            settled: Cell::new((top_layer, top_milestone)),
            shutdown_hooks: Arc::new(Mutex::new(Vec::new())),
            event_sender,
            ids,
        }
    }

    fn insert_query(&self, layer: LayerName, query: QueryFn) {
        if let Ok(mut queries) = self.queries.lock() {
            queries.insert(layer, query);
        }
    }

    fn push_layer(&self, layer: LayerName) {
        if let Ok(mut layers) = self.layer_names.lock() {
            if !layers.contains(&layer) {
                layers.push(layer);
            }
        }
    }

    fn set_settled(&self, layer: LayerName, milestone: PassId) {
        self.settled.set((layer, milestone));
    }

    fn push_shutdown_hook(&self, hook: ShutdownHook) {
        if let Ok(mut hooks) = self.shutdown_hooks.lock() {
            hooks.push(hook);
        }
    }

    fn settled_snapshot(&self) -> (LayerName, PassId) {
        self.settled.get()
    }

    fn next_layer_id(&self) -> LayerName {
        self.ids.next_layer()
    }

    fn next_pass_id(&self) -> PassId {
        self.ids.next_pass()
    }
}

pub struct CompilerBuilder;

impl CompilerBuilder {
    /// Entry point — the pipeline always ingests `SourceText` at the top.
    pub fn new() -> ExpectPass<SourceText> {
        let ids = IdAllocator::new();
        let layer_name = LayerName::root();
        let milestone = PassId::ingress();
        let submit_layer_name = layer_name.clone();
        let submit_milestone = milestone.clone();

        let event_sender: SharedEventSender = Arc::new(OnceLock::new());
        let submit_event_sender = Arc::clone(&event_sender);

        let (output_tx, output_rx) = channel::unbounded::<(RevisionId, SourceTxn)>();
        let submit_top: SubmitTopFn = Arc::new(move |revision, txn| {
            send_layer_event::<SourceText>(
                &submit_event_sender,
                revision,
                submit_layer_name.clone(),
                submit_milestone.clone(),
                &txn,
            );

            output_tx
                .send((revision, txn))
                .map_err(|_| RuntimeError::ChannelClosed)
        });

        let core = BuilderCore::new(submit_top, layer_name.clone(), milestone, event_sender, ids);

        ExpectPass {
            core,
            input_rx: output_rx,
            upstream_seed: SourceText::default(),
            layer_name,
            query_handle: None,
            listeners: Vec::new(),
            _marker: PhantomData,
        }
    }
}

pub struct ExpectPass<U: IR + 'static> {
    core: BuilderCore,
    input_rx: channel::Receiver<(RevisionId, scheme::Transaction<U>)>,
    upstream_seed: U,
    layer_name: LayerName,
    query_handle: Option<QueryHandle<U>>,
    listeners: ListenerSet<U>,
    _marker: PhantomData<U>,
}

impl<U> ExpectPass<U>
where
    U: IR + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + Serialize + 'static,
    U::Error: Send + fmt::Debug + 'static,
{
    pub fn tap(mut self) -> (Self, LayerObserver<U>)
    where
        U::Ix: Send + Sync + 'static,
        U::Value: Send + Sync + 'static,
    {
        let (tx, rx) = channel::unbounded();
        let lock = Arc::new(OnceLock::new());
        if let Some(qh) = &self.query_handle {
            let _ = lock.set(qh.clone());
        }
        self.listeners.push((tx, Arc::clone(&lock)));
        (
            self,
            LayerObserver {
                updates: rx,
                query: lock,
            },
        )
    }

    pub fn then_pass<P>(self, pass: P) -> ExpectLayer<U, P>
    where
        P: Send + 'static,
    {
        let pass_id = self.core.next_pass_id();
        let core = self.core;
        ExpectLayer {
            core,
            input_rx: self.input_rx,
            upstream_seed: self.upstream_seed,
            upstream_layer_name: self.layer_name,
            pass_id,
            pass,
            upstream_listeners: self.listeners,
            _marker: PhantomData,
        }
    }

    pub fn then_fork<P1, P2>(self, pass1: P1, pass2: P2) -> (ExpectLayer<U, P1>, ExpectLayer<U, P2>)
    where
        U: Clone,
        P1: Send + 'static,
        P2: Send + 'static,
    {
        let (tx1, rx1) = channel::unbounded::<(RevisionId, scheme::Transaction<U>)>();
        let (tx2, rx2) = channel::unbounded::<(RevisionId, scheme::Transaction<U>)>();

        let input_rx = self.input_rx;
        let (fanout_stop_tx, fanout_stop_rx) = channel::unbounded::<()>();
        let fanout_listeners = self.listeners;
        let fanout_handle = thread::spawn(move || {
            loop {
                crossbeam::select! {
                    recv(fanout_stop_rx) -> _ => {
                        break;
                    }
                    recv(input_rx) -> msg => {
                        match msg {
                            Ok((revision, txn)) => {
                                // Fan to attached observers (at-submission timing for fork).
                                for (tx, _) in &fanout_listeners {
                                    let _ = tx.send((revision, Arc::clone(&txn)));
                                }
                                let copy = std::sync::Arc::clone(&txn);
                                let _ = tx1.send((revision, copy));
                                let _ = tx2.send((revision, txn));
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        self.core.push_shutdown_hook(Box::new(move || {
            let _ = fanout_stop_tx.send(());
            let _ = fanout_handle.join();
        }));

        let upstream_seed = self.upstream_seed;
        let upstream_layer_name = self.layer_name;
        let core = self.core;
        let pass_id1 = core.next_pass_id();
        let pass_id2 = core.next_pass_id();

        let branch1 = ExpectLayer {
            core: core.clone(),
            input_rx: rx1,
            upstream_seed: upstream_seed.clone(),
            upstream_layer_name: upstream_layer_name.clone(),
            pass_id: pass_id1,
            pass: pass1,
            upstream_listeners: Vec::new(),
            _marker: PhantomData,
        };

        let branch2 = ExpectLayer {
            core,
            input_rx: rx2,
            upstream_seed,
            upstream_layer_name,
            pass_id: pass_id2,
            pass: pass2,
            upstream_listeners: Vec::new(),
            _marker: PhantomData,
        };

        (branch1, branch2)
    }
}

pub struct ExpectLayer<U: IR + 'static, P> {
    core: BuilderCore,
    input_rx: channel::Receiver<(RevisionId, scheme::Transaction<U>)>,
    upstream_seed: U,
    upstream_layer_name: LayerName,
    pass_id: PassId,
    pass: P,
    upstream_listeners: ListenerSet<U>,
    _marker: PhantomData<(U, P)>,
}

impl<U, P> ExpectLayer<U, P>
where
    U: IR + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + Serialize + 'static,
    U::Error: Send + fmt::Debug + 'static,
    P: Send + 'static,
{
    pub fn then_layer<D>(self, downstream: D) -> ExpectPass<D>
    where
        D: IR + Default + Send + 'static,
        D::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        D::Value: Clone + Send + Sync + Serialize + 'static,
        D::Error: Send + fmt::Debug + 'static,
        P: scheme::Pass<U, D> + Send + 'static,
        P::Error: Send + fmt::Debug + 'static,
    {
        let downstream_layer_name = self.core.next_layer_id();
        let (output_rx, downstream_query) = connect_stage::<U, D, P>(
            &self.core,
            self.input_rx,
            self.upstream_seed,
            self.upstream_layer_name,
            downstream_layer_name.clone(),
            self.pass_id,
            self.pass,
            downstream,
            self.upstream_listeners,
        );

        ExpectPass {
            core: self.core,
            input_rx: output_rx,
            upstream_seed: D::default(),
            layer_name: downstream_layer_name,
            query_handle: Some(downstream_query),
            listeners: Vec::new(),
            _marker: PhantomData,
        }
    }
}

fn connect_stage<U, D, P>(
    core: &BuilderCore,
    input_rx: channel::Receiver<(RevisionId, scheme::Transaction<U>)>,
    upstream_seed: U,
    upstream_layer_name: LayerName,
    downstream_layer_name: LayerName,
    pass_id: PassId,
    pass: P,
    downstream: D,
    upstream_listeners: ListenerSet<U>,
) -> (
    channel::Receiver<(RevisionId, scheme::Transaction<D>)>,
    QueryHandle<D>,
)
where
    U: IR + Send + 'static,
    U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
    U::Value: Clone + Send + Sync + Serialize + 'static,
    U::Error: Send + fmt::Debug + 'static,
    D: IR + Default + Send + 'static,
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
    for (_, lock) in &upstream_listeners {
        let _ = lock.set(upstream_query.clone());
    }
    core.insert_query(
        upstream_layer_name,
        Arc::new(move |index| query_handle_json::<U>(&upstream_query, index)),
    );

    let downstream_query = pipeline.downstream.query_handle();
    core.insert_query(
        downstream_layer_name.clone(),
        Arc::new({
            let dq = downstream_query.clone();
            move |index| query_handle_json::<D>(&dq, index)
        }),
    );

    let pipeline_sender = pipeline.clone_sender();
    let (fwd_tx, fwd_rx) = channel::unbounded::<(RevisionId, scheme::Transaction<U>)>();
    let (next_output_tx, next_output_rx) =
        channel::unbounded::<(RevisionId, scheme::Transaction<D>)>();

    let (relay_stop_tx, relay_stop_rx) = channel::unbounded::<()>();
    let relay_handle = thread::spawn(move || {
        loop {
            crossbeam::select! {
                recv(relay_stop_rx) -> _ => {
                    break;
                }
                recv(input_rx) -> msg => {
                    match msg {
                        Ok((revision, txn)) => {
                            if pipeline_sender.send(Arc::clone(&txn)).is_err() {
                                break;
                            }
                            if fwd_tx.send((revision, txn)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });

    let event_sender = Arc::clone(&core.event_sender);
    let bridge_layer_name = downstream_layer_name.clone();
    let bridge_milestone = pass_id.clone();
    let (bridge_stop_tx, bridge_stop_rx) = channel::unbounded::<()>();
    let bridge_handle = thread::spawn(move || {
        loop {
            crossbeam::select! {
                recv(bridge_stop_rx) -> _ => {
                    break;
                }
                recv(tap_rx) -> msg => {
                    match msg {
                        Ok(downstream_txn) => {
                            // tap_rx fires after apply_transaction on the upstream IR,
                            // so the IR is consistent for querying at this point.
                            let Ok((revision, upstream_txn)) = fwd_rx.recv() else {
                                break;
                            };

                            // Deliver to upstream-layer observers.
                            for (tx, _) in &upstream_listeners {
                                let _ = tx.send((revision, Arc::clone(&upstream_txn)));
                            }

                            send_layer_event::<D>(
                                &event_sender,
                                revision,
                                bridge_layer_name.clone(),
                                bridge_milestone.clone(),
                                &downstream_txn,
                            );

                            if next_output_tx.send((revision, downstream_txn)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });

    core.push_shutdown_hook(Box::new(move || {
        let _ = relay_stop_tx.send(());
        let _ = bridge_stop_tx.send(());
        pipeline.shutdown();
        let _ = relay_handle.join();
        let _ = bridge_handle.join();
    }));

    core.push_layer(downstream_layer_name.clone());
    core.set_settled(downstream_layer_name, pass_id);

    (next_output_rx, downstream_query)
}

impl ComposedCompiler {
    pub fn from_pass<U>(pass: ExpectPass<U>) -> Self
    where
        U: IR + Send + 'static,
        U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        U::Value: Clone + Send + Sync + Serialize + 'static,
        U::Error: Send + fmt::Debug + 'static,
    {
        Self::from_pass_with_events(pass, None)
    }

    pub fn from_pass_with_events<U>(
        pass: ExpectPass<U>,
        event_sender: Option<channel::Sender<RuntimeEvent>>,
    ) -> Self
    where
        U: IR + Send + 'static,
        U::Ix: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
        U::Value: Clone + Send + Sync + Serialize + 'static,
        U::Error: Send + fmt::Debug + 'static,
    {
        if let Some(sender) = event_sender {
            let _ = pass.core.event_sender.set(sender);
        }

        // Populate query handles for any terminal-pass observers and consume
        // the terminal input_rx so the pipeline does not back up.
        let terminal_listeners = pass.listeners;
        let terminal_rx = pass.input_rx;
        if !terminal_listeners.is_empty() {
            if let Some(qh) = &pass.query_handle {
                for (_, lock) in &terminal_listeners {
                    let _ = lock.set(qh.clone());
                }
            }
            let (term_stop_tx, term_stop_rx) = channel::unbounded::<()>();
            let term_handle = thread::spawn(move || {
                loop {
                    crossbeam::select! {
                        recv(term_stop_rx) -> _ => break,
                        recv(terminal_rx) -> msg => match msg {
                            Ok((revision, txn)) => {
                                for (tx, _) in &terminal_listeners {
                                    let _ = tx.send((revision, Arc::clone(&txn)));
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            });
            pass.core.push_shutdown_hook(Box::new(move || {
                let _ = term_stop_tx.send(());
                let _ = term_handle.join();
            }));
        }

        let (settled_layer, settled_milestone) = pass.core.settled_snapshot();

        ComposedCompiler {
            submit_top: pass.core.submit_top,
            queries: pass.core.queries,
            layer_names: pass.core.layer_names,
            settled_layer,
            settled_milestone,
            source_layer: pass.core.top_layer,
            next_revision: AtomicU64::new(1),
            source_len: 0,
            shutdown_hooks: pass.core.shutdown_hooks,
            event_sender: pass.core.event_sender,
        }
    }
}

pub struct ComposedCompiler {
    submit_top: SubmitTopFn,
    queries: SharedQueries,
    layer_names: SharedLayerNames,
    settled_layer: LayerName,
    settled_milestone: PassId,
    source_layer: LayerName,
    next_revision: AtomicU64,
    source_len: usize,
    shutdown_hooks: SharedShutdownHooks,
    event_sender: SharedEventSender,
}

impl ComposedCompiler {
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

    pub fn query_json(&self, layer: impl Into<LayerName>, index: Value) -> RuntimeResult<Value> {
        let layer = layer.into();
        let query = self
            .queries
            .lock()
            .ok()
            .and_then(|queries| queries.get(&layer).cloned())
            .ok_or_else(|| runtime_invalid(format!("unknown layer: {layer}")))?;

        query(index)
    }

    pub fn source_text(&self) -> Option<String> {
        let span = Span::new(0, self.source_len);
        let query = serde_json::to_value(span).ok()?;
        let value = self.query_json(self.source_layer.clone(), query).ok()?;
        serde_json::from_value(value).ok()
    }

    pub fn layer_names(&self) -> Vec<LayerName> {
        self.layer_names
            .lock()
            .map(|layers| layers.clone())
            .unwrap_or_default()
    }

    pub fn settled_layer(&self) -> Option<&LayerName> {
        Some(&self.settled_layer)
    }

    pub fn settled_milestone(&self) -> Option<&PassId> {
        Some(&self.settled_milestone)
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
}

impl Drop for ComposedCompiler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn query_handle_json<R>(handle: &QueryHandle<R>, index: Value) -> RuntimeResult<Value>
where
    R: IR,
    R::Ix: DeserializeOwned,
    R::Value: Serialize,
    R::Error: fmt::Debug,
{
    let typed_index: R::Ix = serde_json::from_value(index)
        .map_err(|err| runtime_invalid(format!("query index decode failed: {err}")))?;

    let result = handle
        .query(typed_index)
        .ok_or(RuntimeError::ChannelClosed)?;

    let value = result.map_err(|err| runtime_invalid(format!("query failed: {err:?}")))?;

    serde_json::to_value(value)
        .map_err(|err| runtime_invalid(format!("query encode failed: {err}")))
}

fn clone_event_sender(shared: &SharedEventSender) -> Option<channel::Sender<RuntimeEvent>> {
    shared.get().cloned()
}

fn send_layer_event<R>(
    sender: &SharedEventSender,
    revision: RevisionId,
    layer: LayerName,
    milestone: PassId,
    txn: &scheme::Transaction<R>,
) where
    R: IR,
    R::Ix: Serialize,
    R::Value: Serialize,
{
    let Some(sender) = clone_event_sender(sender) else {
        return;
    };

    match serde_json::to_value(txn) {
        Ok(payload) => {
            let _ = sender.send(RuntimeEvent {
                revision,
                layer,
                milestone,
                payload,
            });
        }
        Err(err) => {
            send_runtime_error_text(
                &Some(sender),
                revision,
                format!("failed to encode layer event payload: {err}"),
            );
        }
    }
}

fn send_runtime_error(sender: &SharedEventSender, revision: RevisionId, err: &RuntimeError) {
    send_runtime_error_text(
        &clone_event_sender(sender),
        revision,
        runtime_error_message(err),
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

    let _ = sender.send(RuntimeEvent {
        revision,
        layer: LayerName::runtime(),
        milestone: PassId::runtime_error(),
        payload: json!({
            "message": message.into(),
        }),
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
                if index.start > len {
                    return Err(runtime_invalid(format!(
                        "insert offset {} out of bounds for text length {}",
                        index.start, len
                    )));
                }
                let frag_len = staged
                    .get(*id)
                    .and_then(|v| *v)
                    .ok_or_else(|| runtime_invalid(format!("unknown staging id: {id}")))?;
                len = len.saturating_add(frag_len);
            }
            scheme::Command::Delete { index } => {
                validate_span(*index, len)?;
                len = len.saturating_sub(index.end - index.start);
            }
            scheme::Command::Replace { index, id } => {
                validate_span(*index, len)?;
                let frag_len = staged
                    .get(*id)
                    .and_then(|v| *v)
                    .ok_or_else(|| runtime_invalid(format!("unknown staging id: {id}")))?;
                len = len - (index.end - index.start) + frag_len;
            }
            scheme::Command::SetRoot { .. } => {}
        }
    }

    Ok(len)
}

fn validate_span(span: Span, len: usize) -> RuntimeResult<()> {
    if span.start <= span.end && span.end <= len {
        Ok(())
    } else {
        Err(runtime_invalid(format!(
            "invalid span [{}, {}] for text length {}",
            span.start, span.end, len
        )))
    }
}

fn runtime_invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidRequest {
        message: message.into(),
    }
}

fn runtime_error_message(err: &RuntimeError) -> String {
    match err {
        RuntimeError::QueueFull => "runtime queue full".to_string(),
        RuntimeError::ChannelClosed => "runtime channel closed".to_string(),
        RuntimeError::InvalidRequest { message } => message.clone(),
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
