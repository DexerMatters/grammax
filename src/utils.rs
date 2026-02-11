use rustc_hash::FxHashMap;
use std::collections::VecDeque;
use std::hash::Hash;
use std::ops;

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
    data: FxHashMap<K, V>,
    order: VecDeque<K>,
}

impl<K: Clone + Eq + std::hash::Hash, V: Clone> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            data: FxHashMap::with_capacity_and_hasher(capacity, Default::default()),
            order: VecDeque::with_capacity(capacity),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<V> {
        if let Some(value) = self.data.get(key) {
            let value = value.clone();
            // Move to end (most recently used)
            if let Some(pos) = self.order.iter().position(|k| k == key) {
                self.order.remove(pos);
            }
            self.order.push_back(key.clone());
            Some(value)
        } else {
            None
        }
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.data.contains_key(&key) {
            self.data.insert(key.clone(), value);
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
        } else {
            if self.data.len() >= self.capacity && self.capacity > 0 {
                if let Some(lru_key) = self.order.pop_front() {
                    self.data.remove(&lru_key);
                }
            }
            if self.capacity > 0 {
                self.data.insert(key.clone(), value);
                self.order.push_back(key);
            }
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.data.contains_key(key)
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.order.clear();
    }

    pub fn rebuild<F>(&mut self, mut mapper: F)
    where
        F: FnMut(K, V) -> Option<(K, V)>,
    {
        let mut new_entries = Vec::new();

        for key in self.order.iter() {
            if let Some(val) = self.data.get(key) {
                if let Some((new_key, new_val)) = mapper(key.clone(), val.clone()) {
                    new_entries.push((new_key, new_val));
                }
            }
        }

        self.data.clear();
        self.order.clear();

        for (k, v) in new_entries {
            self.data.insert(k.clone(), v);
            self.order.push_back(k);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }
    pub fn new_len(offset: usize, len: usize) -> Self {
        Span {
            start: offset,
            end: offset + len,
        }
    }
    pub fn empty() -> Self {
        Span { start: 0, end: 0 }
    }
    pub fn len(&self) -> usize {
        if self.end >= self.start {
            self.end - self.start
        } else {
            0
        }
    }
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl ops::Add for Span {
    type Output = Span;

    fn add(self, other: Span) -> Span {
        Span {
            start: self.start,
            end: other.end,
        }
    }
}

impl From<Span> for ops::Range<usize> {
    fn from(span: Span) -> Self {
        span.start..span.end
    }
}
