use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    sync::Arc,
};

use crossbeam::channel;
use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort, rpc::CallResult};

use crate::{
    scheme::{self, DocumentSpan, SourceAtom, Span, URI},
    utils::{self},
};

use super::{
    compiler::{ComposedCompiler, TypedTree},
    protocol::{
        RevisionId, RuntimeError, RuntimeEvent, RuntimePath, RuntimeRequest, RuntimeSignal,
        RuntimeWireResult,
    },
};

type SourceTxn = scheme::Transaction<scheme::layers::SourceText>;

struct PendingQuery {
    layer_path: RuntimePath,
    index: utils::Payload,
    reply: RpcReplyPort<RuntimeWireResult>,
}

#[derive(Default)]
struct WaitingRoom {
    queries_by_revision: BTreeMap<RevisionId, Vec<PendingQuery>>,
    fetches_by_key: HashMap<(RevisionId, RuntimePath), Vec<RpcReplyPort<RuntimeWireResult>>>,
}

impl WaitingRoom {
    fn park_query(
        &mut self,
        revision: RevisionId,
        layer_path: RuntimePath,
        index: utils::Payload,
        reply: RpcReplyPort<RuntimeWireResult>,
    ) {
        self.queries_by_revision
            .entry(revision)
            .or_default()
            .push(PendingQuery {
                layer_path,
                index,
                reply,
            });
    }

    fn park_fetch(
        &mut self,
        revision: RevisionId,
        layer_path: RuntimePath,
        reply: RpcReplyPort<RuntimeWireResult>,
    ) {
        self.fetches_by_key
            .entry((revision, layer_path))
            .or_default()
            .push(reply);
    }

    fn take_ready_queries(&mut self, settled_revision: RevisionId) -> Vec<PendingQuery> {
        let keys: Vec<RevisionId> = self
            .queries_by_revision
            .range(..=settled_revision)
            .map(|(revision, _)| *revision)
            .collect();

        let mut ready = Vec::new();
        for key in keys {
            if let Some(mut queries) = self.queries_by_revision.remove(&key) {
                ready.append(&mut queries);
            }
        }
        ready
    }

    fn take_fetches_for(
        &mut self,
        revision: RevisionId,
        layer_path: &RuntimePath,
    ) -> Vec<RpcReplyPort<RuntimeWireResult>> {
        self.fetches_by_key
            .remove(&(revision, layer_path.clone()))
            .unwrap_or_default()
    }

    fn fail_all(&mut self) {
        let queries = std::mem::take(&mut self.queries_by_revision);
        for (_, queued) in queries {
            for pending in queued {
                let _ = pending.reply.send(Err(RuntimeError::ChannelClosed));
            }
        }

        let fetches = std::mem::take(&mut self.fetches_by_key);
        for (_, queued) in fetches {
            for reply in queued {
                let _ = reply.send(Err(RuntimeError::ChannelClosed));
            }
        }
    }
}

#[derive(Clone)]
pub struct GlobalEventDispatcher {
    actor: ActorRef<GedMessage>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl GlobalEventDispatcher {
    pub(crate) fn start<Tree: TypedTree + 'static>(
        compiler: ComposedCompiler<Tree>,
        evt_rx: channel::Receiver<RuntimeEvent>,
    ) -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to initialize runtime supervisor executor"),
        );

        let spawned: Result<(ActorRef<GedMessage>, ractor::concurrency::JoinHandle<()>), _> =
            runtime.block_on(async {
                let state = GedState::new(compiler);
                Actor::spawn(
                    Some("runtime-supervisor".to_string()),
                    GedActor::<Tree> {
                        _marker: PhantomData,
                    },
                    state,
                )
                .await
            });

        let (actor, _actor_handle) = spawned.expect("failed to spawn runtime supervisor actor");

        let ingestor_spawned: Result<
            (ActorRef<RuntimeEvent>, ractor::concurrency::JoinHandle<()>),
            _,
        > = runtime.block_on(async {
            Actor::spawn_linked(
                Some("runtime-event-ingestor".to_string()),
                EventIngestorActor,
                actor.clone(),
                actor.get_cell(),
            )
            .await
        });
        let (ingestor, _ingestor_handle) =
            ingestor_spawned.expect("failed to spawn runtime event ingestor actor");

        runtime.spawn_blocking(move || {
            while let Ok(event) = evt_rx.recv() {
                if ingestor.cast(event).is_err() {
                    break;
                }
            }
        });

        Self { actor, runtime }
    }

    pub(crate) fn request(&self, request: RuntimeRequest) -> RuntimeWireResult {
        let actor = self.actor.clone();
        self.runtime.block_on(async move {
            match actor
                .call(|reply| GedMessage::Request { request, reply }, None)
                .await
            {
                Ok(CallResult::Success(result)) => result,
                Ok(CallResult::SenderError) | Ok(CallResult::Timeout) | Err(_) => {
                    Err(RuntimeError::ChannelClosed)
                }
            }
        })
    }
}

#[derive(Debug)]
enum GedMessage {
    Request {
        request: RuntimeRequest,
        reply: RpcReplyPort<RuntimeWireResult>,
    },
    PipelineEvent(RuntimeEvent),
}

#[derive(Default)]
struct EventIngestorActor;

impl Actor for EventIngestorActor {
    type Msg = RuntimeEvent;
    type State = ActorRef<GedMessage>;
    type Arguments = ActorRef<GedMessage>;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(args)
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        event: Self::Msg,
        sink: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        if sink.cast(GedMessage::PipelineEvent(event)).is_err() {
            myself.stop(None);
        }
        Ok(())
    }
}

struct GedState<Tree: TypedTree> {
    compiler: ComposedCompiler<Tree>,
    settled_layer_path: RuntimePath,
    settled_pass_path: RuntimePath,
    settled_revision: RevisionId,
    waiting: WaitingRoom,
}

impl<Tree: TypedTree> GedState<Tree> {
    fn new(compiler: ComposedCompiler<Tree>) -> Self {
        let settled_layer_path = compiler
            .settled_layer_path()
            .cloned()
            .unwrap_or_else(RuntimePath::root);
        let settled_pass_path = compiler
            .settled_pass_path()
            .cloned()
            .unwrap_or_else(RuntimePath::root);
        Self {
            compiler,
            settled_layer_path,
            settled_pass_path,
            settled_revision: 0,
            waiting: WaitingRoom::default(),
        }
    }
}

struct GedActor<Tree: TypedTree> {
    _marker: PhantomData<fn() -> Tree>,
}

enum GedEffect {
    SubmitEditAndAccept {
        txn: SourceTxn,
        reply: RpcReplyPort<RuntimeWireResult>,
    },
    SubmitEditAndParkFetch {
        txn: SourceTxn,
        layer_path: RuntimePath,
        reply: RpcReplyPort<RuntimeWireResult>,
    },
    ReplyImmediateQuery {
        layer_path: RuntimePath,
        index: utils::Payload,
        reply: RpcReplyPort<RuntimeWireResult>,
    },
    FlushReadyQueries,
    ResolveFetches {
        revision: RevisionId,
        layer_path: RuntimePath,
        payload: utils::Payload,
    },
    Shutdown {
        reply: RpcReplyPort<RuntimeWireResult>,
    },
}

impl<Tree: TypedTree + 'static> Actor for GedActor<Tree> {
    type Msg = GedMessage;
    type State = GedState<Tree>;
    type Arguments = GedState<Tree>;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(args)
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        let effects = match message {
            GedMessage::Request { request, reply } => reduce_request(state, request, reply),
            GedMessage::PipelineEvent(event) => reduce_event(state, event),
        };

        if apply_effects(state, effects) {
            myself.stop(None);
        }

        Ok(())
    }
}

fn reduce_request<Tree: TypedTree>(
    state: &mut GedState<Tree>,
    request: RuntimeRequest,
    reply: RpcReplyPort<RuntimeWireResult>,
) -> Vec<GedEffect> {
    match request {
        RuntimeRequest::ApplyTextEdit { uri, span, text } => vec![GedEffect::SubmitEditAndAccept {
            txn: edit_to_txn(uri, span, text),
            reply,
        }],
        RuntimeRequest::ApplyAndFetch {
            uri,
            span,
            text,
            layer_path,
        } => vec![GedEffect::SubmitEditAndParkFetch {
            txn: edit_to_txn(uri, span, text),
            layer_path,
            reply,
        }],
        RuntimeRequest::QueryLayer {
            layer_path,
            revision,
            index,
        } => {
            if revision.is_some_and(|target| target > state.settled_revision) {
                state
                    .waiting
                    .park_query(revision.unwrap(), layer_path, index, reply);
                Vec::new()
            } else {
                vec![GedEffect::ReplyImmediateQuery {
                    layer_path,
                    index,
                    reply,
                }]
            }
        }
        RuntimeRequest::Shutdown => vec![GedEffect::Shutdown { reply }],
    }
}

fn reduce_event<Tree: TypedTree>(
    state: &mut GedState<Tree>,
    event: RuntimeEvent,
) -> Vec<GedEffect> {
    let advances =
        event.layer_path == state.settled_layer_path || event.pass_path == state.settled_pass_path;

    let RuntimeEvent {
        revision,
        layer_path,
        payload,
        ..
    } = event;

    let mut effects = vec![GedEffect::ResolveFetches {
        revision,
        layer_path,
        payload,
    }];

    if advances {
        state.settled_revision = state.settled_revision.max(revision);
        effects.push(GedEffect::FlushReadyQueries);
    }

    effects
}

fn apply_effects<Tree: TypedTree>(state: &mut GedState<Tree>, effects: Vec<GedEffect>) -> bool {
    let mut should_stop = false;

    for effect in effects {
        match effect {
            GedEffect::SubmitEditAndAccept { txn, reply } => {
                let result = state
                    .compiler
                    .submit_source(txn)
                    .map(|revision| RuntimeSignal::Accepted { revision })
                    .map_err(to_wire_error);
                let _ = reply.send(result);
            }
            GedEffect::SubmitEditAndParkFetch {
                txn,
                layer_path,
                reply,
            } => match state.compiler.submit_source(txn) {
                Ok(revision) => state.waiting.park_fetch(revision, layer_path, reply),
                Err(err) => {
                    let _ = reply.send(Err(to_wire_error(err)));
                }
            },
            GedEffect::ReplyImmediateQuery {
                layer_path,
                index,
                reply,
            } => {
                let _ = reply.send(execute_query(&mut state.compiler, layer_path, index));
            }
            GedEffect::FlushReadyQueries => {
                for pending in state.waiting.take_ready_queries(state.settled_revision) {
                    let result =
                        execute_query(&mut state.compiler, pending.layer_path, pending.index);
                    let _ = pending.reply.send(result);
                }
            }
            GedEffect::ResolveFetches {
                revision,
                layer_path,
                payload,
            } => {
                let mut replies = state
                    .waiting
                    .take_fetches_for(revision, &layer_path)
                    .into_iter();
                if let Some(primary) = replies.next() {
                    let _ = primary.send(Ok(RuntimeSignal::EditResult {
                        revision,
                        layer_path,
                        value: payload,
                    }));
                    for reply in replies {
                        let _ = reply.send(Err(RuntimeError::ChannelClosed));
                    }
                }
            }
            GedEffect::Shutdown { reply } => {
                state.compiler.shutdown();
                let _ = reply.send(Ok(RuntimeSignal::Ack));
                should_stop = true;
            }
        }
    }

    if should_stop {
        state.waiting.fail_all();
    }

    should_stop
}

fn execute_query<Tree: TypedTree>(
    compiler: &mut ComposedCompiler<Tree>,
    layer_path: RuntimePath,
    index: utils::Payload,
) -> RuntimeWireResult {
    compiler
        .query(layer_path.clone(), index)
        .map(|value| RuntimeSignal::QueryResult { layer_path, value })
}

fn to_wire_error(err: RuntimeError) -> RuntimeError<utils::Payload> {
    match err {
        RuntimeError::QueueFull => RuntimeError::QueueFull,
        RuntimeError::ChannelClosed => RuntimeError::ChannelClosed,
        RuntimeError::InvalidQuery => RuntimeError::InvalidQuery,
        RuntimeError::InvalidRequest { message } => RuntimeError::InvalidRequest { message },
        RuntimeError::InvalidRequestFromTarget { err } => RuntimeError::InvalidRequestFromTarget {
            err: utils::Payload::new_serializable(err),
        },
        RuntimeError::ResourceAbsent => RuntimeError::ResourceAbsent,
        RuntimeError::UnexpectedRequestType => RuntimeError::UnexpectedRequestType,
        RuntimeError::UndefinedBehavior { message } => RuntimeError::UndefinedBehavior { message },
    }
}

fn edit_to_txn(uri: URI, span: Span, text: String) -> SourceTxn {
    use scheme::Command;
    let is_empty = text.is_empty();
    if span.start == span.end && is_empty {
        return Arc::new(Vec::new());
    }
    let doc_span = DocumentSpan { uri, span };
    if span.start == span.end {
        let staged_text: SourceAtom = text.into();
        return Arc::new(vec![
            Command::Create {
                id: 0,
                value: staged_text,
            },
            Command::Insert {
                index: doc_span,
                id: 0,
            },
        ]);
    }
    if is_empty {
        return Arc::new(vec![Command::Delete { index: doc_span }]);
    }
    let staged_text: SourceAtom = text.into();
    Arc::new(vec![
        Command::Create {
            id: 0,
            value: staged_text,
        },
        Command::Replace {
            index: doc_span,
            id: 0,
        },
    ])
}
