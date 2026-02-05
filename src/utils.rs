use dashmap::DashMap;
use std::collections::VecDeque;
use std::ops;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct LruCache<K: Clone + Eq + std::hash::Hash, V: Clone> {
    capacity: usize,
    data: Arc<DashMap<K, V>>,
    order: Arc<parking_lot::Mutex<VecDeque<K>>>,
}

impl<K: Clone + Eq + std::hash::Hash, V: Clone> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            data: Arc::new(DashMap::new()),
            order: Arc::new(parking_lot::Mutex::new(VecDeque::with_capacity(capacity))),
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        self.data.get(key).map(|entry| {
            let mut order = self.order.lock();
            // Move to end (most recently used)
            order.retain(|k| k != key);
            order.push_back(key.clone());
            entry.clone()
        })
    }

    pub fn insert(&self, key: K, value: V) {
        if self.data.contains_key(&key) {
            self.data.insert(key.clone(), value);
            let mut order = self.order.lock();
            order.retain(|k| k != &key);
            order.push_back(key);
        } else {
            if self.data.len() >= self.capacity && self.capacity > 0 {
                let mut order = self.order.lock();
                if let Some(lru_key) = order.pop_front() {
                    self.data.remove(&lru_key);
                }
            }
            self.data.insert(key.clone(), value);
            let mut order = self.order.lock();
            order.push_back(key);
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.data.contains_key(key)
    }

    pub fn clear(&self) {
        self.data.clear();
        self.order.lock().clear();
    }

    pub fn rebuild<F>(&self, mapper: F)
    where
        F: Fn(K, V) -> Option<(K, V)>,
    {
        // Lock order to preserve LRU and consistency
        let mut order = self.order.lock();

        // Collect entries to keep/update
        let mut new_entries = Vec::new();

        for key in order.iter() {
            if let Some(val) = self.data.get(key) {
                if let Some((new_key, new_val)) = mapper(key.clone(), val.clone()) {
                    new_entries.push((new_key, new_val));
                }
            }
        }

        // Clear and refill
        self.data.clear();
        order.clear();

        for (k, v) in new_entries {
            self.data.insert(k.clone(), v);
            order.push_back(k);
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
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
