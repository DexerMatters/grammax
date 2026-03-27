use rustc_hash::FxHashMap;
use std::hash::Hash;

#[macro_export]
macro_rules! impl_listener {
    ($name: ty, $($field: ident ($($typ : ty),*)),+) => {
        impl $name {
            pub fn new() -> Self {
                Self::default()
            }
            $(
                pub fn $field(mut self, callback: impl Fn($($typ),*) + Send + 'static) -> Self {
                    self.$field = Some(Box::new(callback));
                    self
                }
            )+
        }
    };
}

#[derive(Clone, Debug, Default)]
pub struct LruCache<K: Clone + Eq + Hash, V: Clone> {
    capacity: usize,
    data: FxHashMap<K, LruEntry<K, V>>,
    head: Option<K>,
    tail: Option<K>,
}

#[derive(Clone, Debug)]
struct LruEntry<K: Clone, V: Clone> {
    value: V,
    prev: Option<K>,
    next: Option<K>,
}

impl<K: Clone + Eq + std::hash::Hash, V: Clone> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            data: FxHashMap::with_capacity_and_hasher(capacity, Default::default()),
            head: None,
            tail: None,
        }
    }

    fn detach(&mut self, key: &K) {
        let (prev, next) = match self.data.get(key) {
            Some(entry) => (entry.prev.clone(), entry.next.clone()),
            None => return,
        };

        if let Some(prev_key) = prev.clone() {
            if let Some(prev_entry) = self.data.get_mut(&prev_key) {
                prev_entry.next = next.clone();
            }
        } else {
            self.head = next.clone();
        }

        if let Some(next_key) = next.clone() {
            if let Some(next_entry) = self.data.get_mut(&next_key) {
                next_entry.prev = prev.clone();
            }
        } else {
            self.tail = prev.clone();
        }

        if let Some(entry) = self.data.get_mut(key) {
            entry.prev = None;
            entry.next = None;
        }
    }

    fn push_back(&mut self, key: &K) {
        let old_tail = self.tail.clone();

        if let Some(tail_key) = old_tail.clone() {
            if let Some(tail_entry) = self.data.get_mut(&tail_key) {
                tail_entry.next = Some(key.clone());
            }
        } else {
            self.head = Some(key.clone());
        }

        if let Some(entry) = self.data.get_mut(key) {
            entry.prev = old_tail;
            entry.next = None;
        }

        self.tail = Some(key.clone());
    }

    fn touch(&mut self, key: &K) {
        if self.tail.as_ref() == Some(key) {
            return;
        }
        self.detach(key);
        self.push_back(key);
    }

    pub fn peek(&self, key: &K) -> Option<&V> {
        self.data.get(key).map(|entry| &entry.value)
    }

    pub fn touch_key(&mut self, key: &K) {
        self.touch(key);
    }

    pub fn get(&mut self, key: &K) -> Option<V> {
        let value = self.data.get(key).map(|entry| entry.value.clone())?;
        self.touch(key);
        Some(value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }

        if let Some(entry) = self.data.get_mut(&key) {
            entry.value = value;
            self.touch(&key);
            return;
        }

        if self.data.len() >= self.capacity {
            if let Some(lru_key) = self.head.clone() {
                self.detach(&lru_key);
                self.data.remove(&lru_key);
            }
        }

        self.data.insert(
            key.clone(),
            LruEntry {
                value,
                prev: None,
                next: None,
            },
        );
        self.push_back(&key);
    }

    pub fn contains(&self, key: &K) -> bool {
        self.data.contains_key(key)
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.head = None;
        self.tail = None;
    }

    pub fn rebuild<F>(&mut self, mut mapper: F)
    where
        F: FnMut(K, V) -> Option<(K, V)>,
    {
        let mut new_entries = Vec::new();

        let mut cursor = self.head.clone();
        let mut seen = 0usize;
        while let Some(key) = cursor.clone() {
            if seen > self.data.len() {
                break;
            }
            if let Some(entry) = self.data.get(&key) {
                if let Some((new_key, new_val)) = mapper(key.clone(), entry.value.clone()) {
                    new_entries.push((new_key, new_val));
                }
                cursor = entry.next.clone();
            } else {
                break;
            }
            seen += 1;
        }

        self.data.clear();
        self.head = None;
        self.tail = None;

        for (k, v) in new_entries {
            self.insert(k, v);
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

use std::any::Any;

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

struct Inner {
    data: Box<dyn Any + Send + Sync>,
    to_json: Option<fn(&dyn Any) -> serde_json::Value>,
    type_name: &'static str,
}

pub(crate) struct Payload(Box<Inner>);

impl Payload {
    pub fn new<T: Send + Sync + 'static>(value: T) -> Self {
        Self(Box::new(Inner {
            data: Box::new(value),
            to_json: None,
            type_name: std::any::type_name::<T>(),
        }))
    }

    pub fn new_serializable<T: serde::Serialize + Send + Sync + 'static>(value: T) -> Self {
        Self(Box::new(Inner {
            data: Box::new(value),
            to_json: Some(json_for::<T>),
            type_name: std::any::type_name::<T>(),
        }))
    }
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        // Direct path (stored via `new`)
        if let Some(v) = (*self.0.data).downcast_ref::<T>() {
            return Some(v);
        }
        // ForceSync path (stored via `new_any`)
        (*self.0.data).downcast_ref::<ForceSync<T>>().map(|w| &w.0)
    }

    pub fn downcast<T: 'static>(self) -> Option<T> {
        let Inner { data, .. } = *self.0;
        // Coerce Box<dyn Any + Send + Sync> → Box<dyn Any> to access downcast().
        let data: Box<dyn Any> = data;
        match data.downcast::<T>() {
            Ok(boxed) => Some(*boxed),
            Err(data) => data.downcast::<ForceSync<T>>().ok().map(|b| b.0),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        self.0
            .to_json
            .map(|to_json| to_json(self.0.data.as_ref()))
            .unwrap_or(serde_json::Value::Null)
    }
}

impl serde::Serialize for Payload {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl std::fmt::Debug for Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.to_json.is_some() {
            write!(f, "Payload({:?})", self.to_json())
        } else {
            write!(f, "Payload(<opaque:{}>)", self.0.type_name)
        }
    }
}

// Kept for compatibility; not currently used for dynamic dispatch.
pub trait SerdeAny: serde::Serialize + Send + Sync {}
