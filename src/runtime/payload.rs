use std::sync::Arc;

// ── Type tag ─────────────────────────────────────────────────────────────────
//
// We use the address of a generic function as a per-type identifier.
// This avoids `std::any::TypeId`, which requires `T: 'static`.

/// Returns a stable, process-unique integer that identifies `T`.
/// Works for any `T`, including non-`'static` types.
#[inline(always)]
fn type_tag<T>() -> usize {
    // Each monomorphisation lives at a distinct address.
    type_tag::<T> as usize
}

// ── Erased vtable ─────────────────────────────────────────────────────────────

unsafe fn drop_erased<T>(ptr: *mut ()) {
    // SAFETY: ptr was produced by Box::into_raw(Box::new(T)).
    unsafe { drop(Box::from_raw(ptr as *mut T)) };
}

fn to_json_erased<T: serde::Serialize>(ptr: *mut ()) -> serde_json::Value {
    // SAFETY: ptr points to a live, aligned T behind an Arc.
    let val: &T = unsafe { &*(ptr as *const T) };
    serde_json::to_value(val).unwrap_or(serde_json::Value::Null)
}

// ── Inner storage ─────────────────────────────────────────────────────────────

struct PayloadInner {
    /// Heap-allocated value (produced by `Box::into_raw`; freed by `drop_fn`).
    data: *mut (),
    /// Process-unique integer identifying the concrete type.
    type_tag: usize,
    /// Destroys the heap allocation.
    drop_fn: unsafe fn(*mut ()),
    /// Serializes the value to JSON without deserializing first.
    json_fn: fn(*mut ()) -> serde_json::Value,
}

// SAFETY: The data pointer is heap-allocated and exclusively owned by this
// struct (shared via Arc; all access goes through `&self`/`Arc` invariants).
// `T: Send + Sync` is required by `Payload::new`.
unsafe impl Send for PayloadInner {}
unsafe impl Sync for PayloadInner {}

impl Drop for PayloadInner {
    fn drop(&mut self) {
        // SAFETY: `drop_fn` was set to `drop_erased::<T>` for the T stored in
        // `data`; called exactly once here.
        unsafe { (self.drop_fn)(self.data) };
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// A type-erased, reference-counted, serializable payload.
///
/// Unlike `Box<dyn Any>`, this does **not** require the stored type to be
/// `'static`.  The value is heap-allocated and kept alive by an `Arc`, so no
/// lifetimes escape.  Concrete types are recoverable via [`Payload::downcast_ref`].
///
/// `T: 'static` is **not** required by any method of this type.
#[derive(Clone)]
pub struct Payload(Arc<PayloadInner>);

impl Payload {
    /// Store `value` in a type-erased `Payload`.
    ///
    /// `T` only needs to be `Serialize + Send + Sync`; `'static` is **not**
    /// required.  The value is heap-allocated; downcast and serialize without
    /// paying for an intermediate JSON round-trip.
    pub fn new<T: serde::Serialize + Send + Sync>(value: T) -> Self {
        let ptr = Box::into_raw(Box::new(value)) as *mut ();
        Self(Arc::new(PayloadInner {
            data: ptr,
            type_tag: type_tag::<T>(),
            drop_fn: drop_erased::<T>,
            json_fn: to_json_erased::<T>,
        }))
    }

    /// Attempt to downcast to a shared reference of type `T`.
    ///
    /// Returns `None` if the payload was not created with type `T`.
    ///
    /// # Example
    /// ```rust,ignore
    /// let p = Payload::new(vec![1u32, 2, 3]);
    /// assert_eq!(p.downcast_ref::<Vec<u32>>(), Some(&vec![1, 2, 3]));
    /// ```
    pub fn downcast_ref<T>(&self) -> Option<&T> {
        if self.0.type_tag == type_tag::<T>() {
            // SAFETY: type tags match, so self.0.data was obtained from
            // Box::<T>::into_raw.  The Arc is still live, so the pointer
            // is valid for the lifetime of `&self`.
            Some(unsafe { &*(self.0.data as *const T) })
        } else {
            None
        }
    }

    /// Serialize the inner value to a [`serde_json::Value`].
    pub fn to_json(&self) -> serde_json::Value {
        (self.0.json_fn)(self.0.data)
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

/// Marker exported so call-sites can name the `Serialize + Send + Sync`
/// constraint without writing it out explicitly.
pub trait SerdeAny: serde::Serialize + Send + Sync {}
