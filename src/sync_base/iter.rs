use std::{hash::Hash, sync::Arc};

// This trait is implemented by `sync::BaseCache` and `sync::Cache`.
pub(crate) trait ScanningGet<K, V> {
    /// Returns the number of segments in the concurrent hash table.
    fn num_cht_segments(&self) -> usize;

    /// Returns a _clone_ of the value corresponding to the key.
    ///
    /// Unlike the `get` method of cache, this method is not considered a cache read
    /// operation, so it does not update the historic popularity estimator or reset
    /// the idle timer for the key.
    fn scanning_get(&self, key: &Arc<K>) -> Option<V>;

    /// Returns a vec of keys in a specified segment of the concurrent hash table.
    fn keys(&self, cht_segment: usize) -> Option<Vec<Arc<K>>>;
}

pub struct Iter<'i, K, V> {
    keys: Option<Vec<Arc<K>>>,
    cache_segments: Box<[&'i dyn ScanningGet<K, V>]>,
    num_cht_segments: usize,
    cache_seg_index: usize,
    cht_seg_index: usize,
    is_done: bool,
}

impl<'i, K, V> Iter<'i, K, V> {
    pub(crate) fn with_single_cache_segment(
        cache: &'i dyn ScanningGet<K, V>,
        num_cht_segments: usize,
    ) -> Self {
        Self {
            keys: None,
            cache_segments: Box::new([cache]),
            num_cht_segments,
            cache_seg_index: 0,
            cht_seg_index: 0,
            is_done: false,
        }
    }

    #[cfg(feature = "sync")]
    pub(crate) fn with_multiple_cache_segments(
        cache_segments: Box<[&'i dyn ScanningGet<K, V>]>,
        num_cht_segments: usize,
    ) -> Self {
        Self {
            keys: None,
            cache_segments,
            num_cht_segments,
            cache_seg_index: 0,
            cht_seg_index: 0,
            is_done: false,
        }
    }
}

impl<'i, K, V> Iterator for Iter<'i, K, V>
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    type Item = (Arc<K>, V);

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_done {
            return None;
        }

        while let Some(key) = self.next_key() {
            if let Some(v) = self.cache().scanning_get(&key) {
                return Some((key, v));
            }
        }

        self.is_done = true;
        None
    }
}

impl<'i, K, V> Iter<'i, K, V>
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn cache(&self) -> &'i dyn ScanningGet<K, V> {
        self.cache_segments[self.cache_seg_index]
    }

    fn next_key(&mut self) -> Option<Arc<K>> {
        while let Some(keys) = self.current_keys() {
            if let key @ Some(_) = keys.pop() {
                return key;
            }
        }
        None
    }

    fn current_keys(&mut self) -> Option<&mut Vec<Arc<K>>> {
        // If keys is none or some but empty, try to get next keys.
        while self.keys.as_ref().map_or(true, Vec::is_empty) {
            // Adjust indices.
            if self.cht_seg_index >= self.num_cht_segments {
                self.cache_seg_index += 1;
                self.cht_seg_index = 0;
                if self.cache_seg_index >= self.cache_segments.len() {
                    // No more cache segments left.
                    return None;
                }
            }

            let cache_segment = self.cache_segments[self.cache_seg_index];
            self.keys = cache_segment.keys(self.cht_seg_index);
            self.num_cht_segments = cache_segment.num_cht_segments();

            self.cht_seg_index += 1;
        }

        self.keys.as_mut()
    }
}

unsafe impl<'a, 'i, K, V> Send for Iter<'i, K, V>
where
    K: 'a + Eq + Hash + Send,
    V: 'a + Send,
{
}

unsafe impl<'a, 'i, K, V> Sync for Iter<'i, K, V>
where
    K: 'a + Eq + Hash + Sync,
    V: 'a + Sync,
{
}
