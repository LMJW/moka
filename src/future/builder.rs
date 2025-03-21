use super::Cache;
use crate::{
    common::{builder_utils, concurrent::Weigher},
    notification::{self, DeliveryMode, EvictionListener, RemovalCause},
};

use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hash},
    marker::PhantomData,
    sync::Arc,
    time::Duration,
};

/// Builds a [`Cache`][cache-struct] with various configuration knobs.
///
/// [cache-struct]: ./struct.Cache.html
///
/// # Example: Expirations
///
/// ```rust
/// // Cargo.toml
/// //
/// // [dependencies]
/// // moka = { version = "0.9", features = ["future"] }
/// // tokio = { version = "1", features = ["rt-multi-thread", "macros" ] }
/// // futures = "0.3"
///
/// use moka::future::Cache;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() {
///     let cache = Cache::builder()
///         // Max 10,000 entries
///         .max_capacity(10_000)
///         // Time to live (TTL): 30 minutes
///         .time_to_live(Duration::from_secs(30 * 60))
///         // Time to idle (TTI):  5 minutes
///         .time_to_idle(Duration::from_secs( 5 * 60))
///         // Create the cache.
///         .build();
///
///     // This entry will expire after 5 minutes (TTI) if there is no get().
///     cache.insert(0, "zero").await;
///
///     // This get() will extend the entry life for another 5 minutes.
///     cache.get(&0);
///
///     // Even though we keep calling get(), the entry will expire
///     // after 30 minutes (TTL) from the insert().
/// }
/// ```
///
#[must_use]
pub struct CacheBuilder<K, V, C> {
    name: Option<String>,
    max_capacity: Option<u64>,
    initial_capacity: Option<usize>,
    weigher: Option<Weigher<K, V>>,
    eviction_listener: Option<EvictionListener<K, V>>,
    eviction_listener_conf: Option<notification::Configuration>,
    time_to_live: Option<Duration>,
    time_to_idle: Option<Duration>,
    invalidator_enabled: bool,
    cache_type: PhantomData<C>,
}

impl<K, V> Default for CacheBuilder<K, V, Cache<K, V, RandomState>>
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            name: None,
            max_capacity: None,
            initial_capacity: None,
            weigher: None,
            eviction_listener: None,
            eviction_listener_conf: None,
            time_to_live: None,
            time_to_idle: None,
            invalidator_enabled: false,
            cache_type: Default::default(),
        }
    }
}

impl<K, V> CacheBuilder<K, V, Cache<K, V, RandomState>>
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Construct a new `CacheBuilder` that will be used to build a `Cache` holding
    /// up to `max_capacity` entries.
    pub fn new(max_capacity: u64) -> Self {
        Self {
            max_capacity: Some(max_capacity),
            ..Default::default()
        }
    }

    /// Builds a `Cache<K, V>`.
    ///
    /// # Panics
    ///
    /// Panics if configured with either `time_to_live` or `time_to_idle` higher than
    /// 1000 years. This is done to protect against overflow when computing key
    /// expiration.
    pub fn build(self) -> Cache<K, V, RandomState> {
        let build_hasher = RandomState::default();
        builder_utils::ensure_expirations_or_panic(self.time_to_live, self.time_to_idle);
        Cache::with_everything(
            self.name,
            self.max_capacity,
            self.initial_capacity,
            build_hasher,
            self.weigher,
            self.eviction_listener,
            self.eviction_listener_conf,
            self.time_to_live,
            self.time_to_idle,
            self.invalidator_enabled,
            builder_utils::housekeeper_conf(true),
        )
    }

    /// Builds a `Cache<K, V, S>`, with the given `hasher`.
    ///
    /// # Panics
    ///
    /// Panics if configured with either `time_to_live` or `time_to_idle` higher than
    /// 1000 years. This is done to protect against overflow when computing key
    /// expiration.
    pub fn build_with_hasher<S>(self, hasher: S) -> Cache<K, V, S>
    where
        S: BuildHasher + Clone + Send + Sync + 'static,
    {
        builder_utils::ensure_expirations_or_panic(self.time_to_live, self.time_to_idle);
        Cache::with_everything(
            self.name,
            self.max_capacity,
            self.initial_capacity,
            hasher,
            self.weigher,
            self.eviction_listener,
            self.eviction_listener_conf,
            self.time_to_live,
            self.time_to_idle,
            self.invalidator_enabled,
            builder_utils::housekeeper_conf(true),
        )
    }
}

impl<K, V, C> CacheBuilder<K, V, C> {
    /// Sets the name of the cache. Currently the name is used for identification
    /// only in logging messages.
    pub fn name(self, name: &str) -> Self {
        Self {
            name: Some(name.to_string()),
            ..self
        }
    }

    /// Sets the max capacity of the cache.
    pub fn max_capacity(self, max_capacity: u64) -> Self {
        Self {
            max_capacity: Some(max_capacity),
            ..self
        }
    }

    /// Sets the initial capacity (number of entries) of the cache.
    pub fn initial_capacity(self, number_of_entries: usize) -> Self {
        Self {
            initial_capacity: Some(number_of_entries),
            ..self
        }
    }

    /// Sets the weigher closure to the cache.
    ///
    /// The closure should take `&K` and `&V` as the arguments and returns a `u32`
    /// representing the relative size of the entry.
    pub fn weigher(self, weigher: impl Fn(&K, &V) -> u32 + Send + Sync + 'static) -> Self {
        Self {
            weigher: Some(Arc::new(weigher)),
            ..self
        }
    }

    /// Sets the eviction listener closure to the cache.
    ///
    /// The closure should take `Arc<K>`, `V` and [`RemovalCause`][removal-cause] as
    /// the arguments. The [queued delivery mode][queued-mode] is used for the
    /// listener.
    ///
    /// # Panics
    ///
    /// It is very important to make the listener closure not to panic. Otherwise,
    /// the cache will stop calling the listener after a panic. This is an intended
    /// behavior because the cache cannot know whether is is memory safe or not to
    /// call the panicked lister again.
    ///
    /// [removal-cause]: ../notification/enum.RemovalCause.html
    /// [queued-mode]: ../notification/enum.DeliveryMode.html#variant.Queued
    pub fn eviction_listener_with_queued_delivery_mode(
        self,
        listener: impl Fn(Arc<K>, V, RemovalCause) + Send + Sync + 'static,
    ) -> Self {
        let conf = notification::Configuration::builder()
            .delivery_mode(DeliveryMode::Queued)
            .build();
        Self {
            eviction_listener: Some(Arc::new(listener)),
            eviction_listener_conf: Some(conf),
            ..self
        }
    }

    /// Sets the time to live of the cache.
    ///
    /// A cached entry will be expired after the specified duration past from
    /// `insert`.
    ///
    /// # Panics
    ///
    /// `CacheBuilder::build*` methods will panic if the given `duration` is longer
    /// than 1000 years. This is done to protect against overflow when computing key
    /// expiration.
    pub fn time_to_live(self, duration: Duration) -> Self {
        Self {
            time_to_live: Some(duration),
            ..self
        }
    }

    /// Sets the time to idle of the cache.
    ///
    /// A cached entry will be expired after the specified duration past from `get`
    /// or `insert`.
    ///
    /// # Panics
    ///
    /// `CacheBuilder::build*` methods will panic if the given `duration` is longer
    /// than 1000 years. This is done to protect against overflow when computing key
    /// expiration.
    pub fn time_to_idle(self, duration: Duration) -> Self {
        Self {
            time_to_idle: Some(duration),
            ..self
        }
    }

    /// Enables support for [Cache::invalidate_entries_if][cache-invalidate-if]
    /// method.
    ///
    /// The cache will maintain additional internal data structures to support
    /// `invalidate_entries_if` method.
    ///
    /// [cache-invalidate-if]: ./struct.Cache.html#method.invalidate_entries_if
    pub fn support_invalidation_closures(self) -> Self {
        Self {
            invalidator_enabled: true,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CacheBuilder;

    use std::time::Duration;

    #[tokio::test]
    async fn build_cache() {
        // Cache<char, String>
        let cache = CacheBuilder::new(100).build();
        let policy = cache.policy();

        assert_eq!(policy.max_capacity(), Some(100));
        assert_eq!(policy.time_to_live(), None);
        assert_eq!(policy.time_to_idle(), None);
        assert_eq!(policy.num_segments(), 1);

        cache.insert('a', "Alice").await;
        assert_eq!(cache.get(&'a'), Some("Alice"));

        let cache = CacheBuilder::new(100)
            .time_to_live(Duration::from_secs(45 * 60))
            .time_to_idle(Duration::from_secs(15 * 60))
            .build();
        let policy = cache.policy();

        assert_eq!(policy.max_capacity(), Some(100));
        assert_eq!(policy.time_to_live(), Some(Duration::from_secs(45 * 60)));
        assert_eq!(policy.time_to_idle(), Some(Duration::from_secs(15 * 60)));
        assert_eq!(policy.num_segments(), 1);

        cache.insert('a', "Alice").await;
        assert_eq!(cache.get(&'a'), Some("Alice"));
    }

    #[tokio::test]
    #[should_panic(expected = "time_to_live is longer than 1000 years")]
    async fn build_cache_too_long_ttl() {
        let thousand_years_secs: u64 = 1000 * 365 * 24 * 3600;
        let builder: CacheBuilder<char, String, _> = CacheBuilder::new(100);
        let duration = Duration::from_secs(thousand_years_secs);
        builder
            .time_to_live(duration + Duration::from_secs(1))
            .build();
    }

    #[tokio::test]
    #[should_panic(expected = "time_to_idle is longer than 1000 years")]
    async fn build_cache_too_long_tti() {
        let thousand_years_secs: u64 = 1000 * 365 * 24 * 3600;
        let builder: CacheBuilder<char, String, _> = CacheBuilder::new(100);
        let duration = Duration::from_secs(thousand_years_secs);
        builder
            .time_to_idle(duration + Duration::from_secs(1))
            .build();
    }
}
