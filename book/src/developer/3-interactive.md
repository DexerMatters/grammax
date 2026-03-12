# Interactive

We say a compiler is **interactive** when it can continuously handle updates from the world outside and output what we want by observing it.

In this chapter, we will introduce how to make your compiler interactive. We will analyze the existing interfaces defined in Grammax (`BasicInterface`, `WebPreviewInterface`, and `CliInterface`) and learn how to implement an interface for your own frontend. After understanding the interactive design, you will be able to make your compiler interactive and build your own interface for it.

## Runtime service

**Runtime service** is a wrapper around the compiler that continuously observes the world outside, receives updates from it, feeds them into the compiler, and outputs the results back to the caller. 

```rust
let pass = CompilerBuilder::new()
    .then_pass(ParserPass::new(grammar))
    .then_layer(ParseTreeIR::default())

let runtime = RuntimeService::<WebPreviewInterface>::new(grammar, move |evt_tx| {
    ComposedCompiler::from_pass_with_events(pass, evt_tx)
});

runtime.run().expect("Runtime failed unexpectedly");
```

The `WebPreviewInterface` here is a generic parameter that specifies the type of interface to use. `RuntimeService` implements the interface and provides APIs. It is initialized with a closure that offers an event sender `evt_tx` to the compiler. The compiler can use this sender to send events to the interface, which will then output the results to the caller. Finally, we call `run` method to start the runtime service.

Different interfaces provide different APIs through the service. For example,

```rust

let runtime = RuntimeService::<BasicInterface>::new(grammar, move |evt_tx| {
    ComposedCompiler::from_pass_with_events(pass, evt_tx)
});

runtime.insert(0, "1 + 2 * 3").unwrap();
runtime.update(0, 1, "4").unwrap();
```
Here we use `BasicInterface`, which provides simple APIs for inserting and updating the source text. We can call `insert` and `update` methods on the runtime service to send updates to the compiler, which will then process them and output the results back to the caller.

## Protocol

We need a unified protocol to define the communication between the runtime service and the outside world. 

```rust
pub enum RuntimeRequest {
    ApplyTextEdit {
        span: Span,
        text: String,
    },
    QueryLayer {
        layer_path: RuntimePath,
        revision: Option<RevisionId>,
        index: Payload,
    },
    Shutdown,
}

pub enum RuntimeSignal {
    Accepted {
        revision: RevisionId,
    },
    Event {
        event: RuntimeEvent,
    },
    QueryResult {
        layer_path: RuntimePath,
        value: Payload,
    },
    Ack,
}
```

Request is the message sent from the outside world to the runtime service. It can be a request to apply a text edit, query a layer, or shut down the service. Signal is the response from the runtime service. 

### Apply a text edit

The caller sends a request to apply a text edit, which includes the span of the text to be edited and the new text. The runtime service will process this request, update the source layer accordingly, and then send an `Event` signal back to the caller to indicate that the edit has been applied. An event can be defined as follows:

```rust
pub struct RuntimeEvent {
    pub revision: RevisionId,
    pub layer_path: RuntimePath,
    pub pass_path: RuntimePath,
    pub is_error: bool,
    pub payload: Payload,
}
```

Edit request triggers a runtime event from the bottommost layer to the caller. It responses with (1) the revision of the edit, the bottommost layer path, (2) the pass path that generates this event, (3) whether there is an error in this event, and (4) the payload of this event, which can be the error message or the output transaction of the pass.

**Revision** is a unique number that identifies the edit. It is helpful especially when there are multiple edits in a short time, the caller's query can be specified with a revision to indicate after which edit the query is followed. For example, if the caller wants to query the CST layer after the edit with revision 5, it can send a query request with `revision: Some(5)`. The runtime service will then ensure that the query is processed after the event corresponding to revision 5 has been sent.

### Query a layer
The caller sends a request to query a layer, which includes the path of the layer to be queried, an optional revision, and the index for the query. The runtime service will process this request, retrieve the requested information from the specified layer, and then send a `QueryResult` signal back to the caller with the result of the query. The `layer_path` in the `QueryResult` signal indicates which layer the query result is from, and the `value` is the result of the query. The `value` is necessarily of the value type defined in the IR of the layer.

### Shutdown

The caller sends a shutdown request to the runtime service, which will then clean up any resources and terminate the service gracefully. The runtime service will respond with an `Ack` signal to acknowledge the shutdown request.

### Envelope

```rust
pub struct RuntimeEnvelope {
    pub request: RuntimeRequest,
    pub reply: channel::Sender<Result<RuntimeSignal, RuntimeError>>,
}
```

Each request establishes a one-shot channel for the runtime service to send back the response. The `RuntimeEnvelope` struct encapsulates both the request and the channel sender for the response, allowing for asynchronous communication between the caller and the runtime service. When the caller sends a request, it includes a `RuntimeEnvelope` that contains the request and a sender for the response. The runtime service processes the request and uses the sender to send back the appropriate signal as a response.

## Interface

**Interface** is a trait that allows developers to implement their own way of communication with the compiler, just like the predefined `BasicInterface`, `WebPreviewInterface`, and `CliInterface` in Grammax.

```rust
pub trait Interface {
    fn new(ged: GlobalEventDispatcher, grammar: &'static Grammar) -> Self
    where
        Self: Sized;
    fn sender(&self) -> &channel::Sender<RuntimeEnvelope>;
    fn request(&self, request: RuntimeRequest) -> RuntimeResult {
        ...
    }
}
```

`Interface` is initialized with the global event dispatcher and the grammar (both provided by the runtime service). 