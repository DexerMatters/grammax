use std::any::Any;
use std::sync::Arc;

// ── ForceSync wrapper ─────────────────────────────────────────────────────────
//
// Used only by `Payload::new_any` for values that are not Send/Sync (e.g.
// `TreeAllocRef = Rc<RefCell<...>>`).  The caller guarantees that the payload
// is only accessed on the thread that created it.

struct ForceSync<T>(T);
// SAFETY: Payload::new_any callers (e.g. the IR query path) ensure the payload
// is only materialised and consumed on the single IR worker thread.
unsafe impl<T> Send for ForceSync<T> {}
unsafe impl<T> Sync for ForceSync<T> {}

// ── JSON helpers ──────────────────────────────────────────────────────────────

fn json_for<T: serde::Serialize + 'static>(any: &dyn Any) -> serde_json::Value {
    match any.downcast_ref::<T>() {
        Some(v) => serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    }
}

fn json_unavailable(_: &dyn Any) -> serde_json::Value {
    serde_json::Value::String("<non-serializable>".to_string())
}

// ── Inner ─────────────────────────────────────────────────────────────────────

struct Inner {
    data: Arc<dyn Any + Send + Sync>,
    to_json: fn(&dyn Any) -> serde_json::Value,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Type-erased value that travels through the runtime pipeline.
///
/// Inside the framework payloads are passed as typed values and retrieved via
/// [`Payload::downcast_ref`].  JSON serialisation only happens at the HTTP
/// frontier via [`Payload::to_json`].
#[derive(Clone)]
pub struct Payload(Arc<Inner>);

impl Payload {
    /// Wrap a serialisable value.  `to_json` will work at the HTTP boundary.
    pub fn new<T: serde::Serialize + Send + Sync + 'static>(value: T) -> Self {
        Self(Arc::new(Inner {
            data: Arc::new(value),
            to_json: json_for::<T>,
        }))
    }

    /// Wrap a non-serialisable or non-Send value (e.g. `Rc`-based handles).
    ///
    /// `to_json` returns a placeholder string.  Access the value via
    /// [`downcast_ref`](Self::downcast_ref), which transparently unwraps the
    /// internal `ForceSync` wrapper.
    pub fn new_any<T: 'static>(value: T) -> Self {
        Self(Arc::new(Inner {
            data: Arc::new(ForceSync(value)),
            to_json: json_unavailable,
        }))
    }

    /// Borrow the inner value as `&T`.
    ///
    /// Works for values stored via both [`new`](Self::new) and
    /// [`new_any`](Self::new_any).
    pub fn downcast_ref<T: Any + 'static>(&self) -> Option<&T> {
        // Direct path (stored via `new`)
        if let Some(v) = (*self.0.data).downcast_ref::<T>() {
            return Some(v);
        }
        // ForceSync path (stored via `new_any`)
        (*self.0.data).downcast_ref::<ForceSync<T>>().map(|w| &w.0)
    }

    /// Serialise the inner value to JSON.  Only intended for the HTTP boundary.
    pub fn to_json(&self) -> serde_json::Value {
        (self.0.to_json)(self.0.data.as_ref())
    }
}

impl serde::Serialize for Payload {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl std::fmt::Debug for Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Payload({:?})", self.to_json())
    }
}

// Kept for compatibility; not currently used for dynamic dispatch.
pub trait SerdeAny: serde::Serialize + Send + Sync {}
