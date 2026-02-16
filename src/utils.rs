use rustc_hash::FxHashMap;
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
