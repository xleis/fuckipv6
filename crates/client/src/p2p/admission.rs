//! Bounded admission control for incoming peer handshakes.
//!
//! The guard reserves capacity before expensive responder state is allocated.
//! Reservations are released explicitly when a handshake completes and by
//! `Drop` on every rejection/cancellation path. Expired reservations are
//! reclaimed lazily on admission or metrics reads; active reservations are
//! never evicted to make room for a new peer.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Stable admission key for one authenticated (or claimed) peer identity.
pub(crate) type AdmissionIdentity = [u8; 32];

/// Limits applied to one shared handshake admission table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HandshakeAdmissionConfig {
    /// Maximum active reservations from one source IP.
    pub per_source: usize,
    /// Maximum active reservations for one peer identity.
    pub per_identity: usize,
    /// Maximum active reservations across all sources and identities.
    pub global: usize,
    /// Maximum rows retained in the bounded reservation table.
    pub max_entries: usize,
    /// Lifetime of a reservation while key confirmation is pending.
    pub ttl: Duration,
}

impl Default for HandshakeAdmissionConfig {
    fn default() -> Self {
        Self {
            per_source: 8,
            per_identity: 2,
            global: 128,
            max_entries: 256,
            ttl: Duration::from_secs(10),
        }
    }
}

impl HandshakeAdmissionConfig {
    fn validate(self) -> Result<Self, HandshakeAdmissionError> {
        if self.per_source == 0 {
            return Err(HandshakeAdmissionError::InvalidConfig(
                "per-source limit must be greater than zero",
            ));
        }
        if self.per_identity == 0 {
            return Err(HandshakeAdmissionError::InvalidConfig(
                "per-identity limit must be greater than zero",
            ));
        }
        if self.global == 0 {
            return Err(HandshakeAdmissionError::InvalidConfig(
                "global limit must be greater than zero",
            ));
        }
        if self.max_entries == 0 {
            return Err(HandshakeAdmissionError::InvalidConfig(
                "table limit must be greater than zero",
            ));
        }
        if self.ttl.is_zero() {
            return Err(HandshakeAdmissionError::InvalidConfig(
                "reservation TTL must be greater than zero",
            ));
        }
        Ok(self)
    }
}

/// Fail-closed reason an incoming handshake was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum HandshakeAdmissionError {
    #[error("handshake admission state is unavailable")]
    Unavailable,
    #[error("global concurrent-handshake limit reached")]
    GlobalLimit,
    #[error("per-source concurrent-handshake limit reached")]
    SourceLimit,
    #[error("per-identity concurrent-handshake limit reached")]
    IdentityLimit,
    #[error("handshake reservation table is full of active entries")]
    TableFull,
    #[error("handshake reservation has expired")]
    Expired,
    #[error("handshake reservation identity does not match")]
    IdentityMismatch,
    #[error("invalid handshake admission configuration: {0}")]
    InvalidConfig(&'static str),
}

/// Monotonic counters and a current gauge for handshake admission.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HandshakeAdmissionMetrics {
    pub active_reservations: u64,
    pub source_entries: u64,
    pub identity_entries: u64,
    pub admitted: u64,
    pub dropped: u64,
    pub expired: u64,
    pub rejected_global: u64,
    pub rejected_source: u64,
    pub rejected_identity: u64,
    pub rejected_table_full: u64,
    pub rejected_unavailable: u64,
}

impl HandshakeAdmissionMetrics {
    /// Total rejected handshakes across all fail-closed admission reasons.
    pub fn rejected_total(self) -> u64 {
        self.rejected_global
            .saturating_add(self.rejected_source)
            .saturating_add(self.rejected_identity)
            .saturating_add(self.rejected_table_full)
            .saturating_add(self.rejected_unavailable)
    }
}

#[derive(Default)]
struct Counters {
    admitted: AtomicU64,
    dropped: AtomicU64,
    expired: AtomicU64,
    rejected_global: AtomicU64,
    rejected_source: AtomicU64,
    rejected_identity: AtomicU64,
    rejected_table_full: AtomicU64,
    rejected_unavailable: AtomicU64,
}

#[derive(Debug, Clone)]
struct Reservation {
    source: IpAddr,
    identity: Option<AdmissionIdentity>,
    expires_at: Instant,
}

#[derive(Default)]
struct AdmissionState {
    next_id: u64,
    reservations: HashMap<u64, Reservation>,
    expirations: BTreeMap<(Instant, u64), ()>,
    source_counts: HashMap<IpAddr, usize>,
    identity_counts: HashMap<AdmissionIdentity, usize>,
}

/// Shared, bounded handshake admission guard.
pub(crate) struct HandshakeAdmission {
    config: HandshakeAdmissionConfig,
    state: Mutex<AdmissionState>,
    counters: Counters,
}

impl HandshakeAdmission {
    /// Build a guard from explicit limits. Invalid or zero limits fail closed.
    pub(crate) fn new(
        config: HandshakeAdmissionConfig,
    ) -> Result<Arc<Self>, HandshakeAdmissionError> {
        let config = config.validate()?;
        Ok(Arc::new(Self {
            config,
            state: Mutex::new(AdmissionState::default()),
            counters: Counters::default(),
        }))
    }

    /// Build the production guard with conservative default limits.
    pub(crate) fn shared_default() -> Arc<Self> {
        Self::new(HandshakeAdmissionConfig::default())
            .expect("the built-in handshake admission configuration is valid")
    }

    /// Reserve one handshake slot before allocating responder crypto state.
    pub(crate) fn try_reserve(
        self: &Arc<Self>,
        source: IpAddr,
        identity: AdmissionIdentity,
        now: Instant,
    ) -> Result<HandshakeAdmissionPermit, HandshakeAdmissionError> {
        self.try_reserve_optional_identity(source, Some(identity), now)
    }

    /// Reserve a pre-authentication QUIC handshake by source only.
    pub(crate) fn try_reserve_source(
        self: &Arc<Self>,
        source: IpAddr,
        now: Instant,
    ) -> Result<HandshakeAdmissionPermit, HandshakeAdmissionError> {
        self.try_reserve_optional_identity(source, None, now)
    }

    fn try_reserve_optional_identity(
        self: &Arc<Self>,
        source: IpAddr,
        identity: Option<AdmissionIdentity>,
        now: Instant,
    ) -> Result<HandshakeAdmissionPermit, HandshakeAdmissionError> {
        let source = canonical_ip(source);
        let expires_at = now
            .checked_add(self.config.ttl)
            .ok_or(HandshakeAdmissionError::Unavailable)?;
        let mut state = self.lock_state()?;
        self.prune_expired_locked(&mut state, now);

        if state.reservations.len() >= self.config.global {
            self.counters
                .rejected_global
                .fetch_add(1, Ordering::Relaxed);
            return Err(HandshakeAdmissionError::GlobalLimit);
        }
        if state
            .source_counts
            .get(&source)
            .copied()
            .unwrap_or_default()
            >= self.config.per_source
        {
            self.counters
                .rejected_source
                .fetch_add(1, Ordering::Relaxed);
            return Err(HandshakeAdmissionError::SourceLimit);
        }
        if let Some(identity) = identity {
            if state
                .identity_counts
                .get(&identity)
                .copied()
                .unwrap_or_default()
                >= self.config.per_identity
            {
                self.counters
                    .rejected_identity
                    .fetch_add(1, Ordering::Relaxed);
                return Err(HandshakeAdmissionError::IdentityLimit);
            }
        }
        if state.reservations.len() >= self.config.max_entries {
            self.counters
                .rejected_table_full
                .fetch_add(1, Ordering::Relaxed);
            return Err(HandshakeAdmissionError::TableFull);
        }

        let id = self.allocate_id_locked(&mut state)?;
        let reservation = Reservation {
            source,
            identity,
            expires_at,
        };
        state.reservations.insert(id, reservation);
        state.expirations.insert((expires_at, id), ());
        *state.source_counts.entry(source).or_default() += 1;
        if let Some(identity) = identity {
            *state.identity_counts.entry(identity).or_default() += 1;
        }
        self.counters.admitted.fetch_add(1, Ordering::Relaxed);

        Ok(HandshakeAdmissionPermit {
            admission: Arc::clone(self),
            id,
            #[cfg(test)]
            identity,
            expires_at,
        })
    }

    /// Snapshot counters after reclaiming reservations whose TTL elapsed.
    pub fn metrics(&self, now: Instant) -> HandshakeAdmissionMetrics {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => {
                self.counters
                    .rejected_unavailable
                    .fetch_add(1, Ordering::Relaxed);
                return HandshakeAdmissionMetrics {
                    rejected_unavailable: self
                        .counters
                        .rejected_unavailable
                        .load(Ordering::Relaxed),
                    ..HandshakeAdmissionMetrics::default()
                };
            }
        };
        self.prune_expired_locked(&mut state, now);
        HandshakeAdmissionMetrics {
            active_reservations: state.reservations.len() as u64,
            source_entries: state.source_counts.len() as u64,
            identity_entries: state.identity_counts.len() as u64,
            admitted: self.counters.admitted.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            expired: self.counters.expired.load(Ordering::Relaxed),
            rejected_global: self.counters.rejected_global.load(Ordering::Relaxed),
            rejected_source: self.counters.rejected_source.load(Ordering::Relaxed),
            rejected_identity: self.counters.rejected_identity.load(Ordering::Relaxed),
            rejected_table_full: self.counters.rejected_table_full.load(Ordering::Relaxed),
            rejected_unavailable: self.counters.rejected_unavailable.load(Ordering::Relaxed),
        }
    }

    fn lock_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, AdmissionState>, HandshakeAdmissionError> {
        self.state.lock().map_err(|_| {
            self.counters
                .rejected_unavailable
                .fetch_add(1, Ordering::Relaxed);
            HandshakeAdmissionError::Unavailable
        })
    }

    fn allocate_id_locked(
        &self,
        state: &mut AdmissionState,
    ) -> Result<u64, HandshakeAdmissionError> {
        let start = state.next_id;
        loop {
            let id = state.next_id;
            state.next_id = state.next_id.wrapping_add(1);
            if !state.reservations.contains_key(&id) {
                return Ok(id);
            }
            if state.next_id == start {
                self.counters
                    .rejected_table_full
                    .fetch_add(1, Ordering::Relaxed);
                return Err(HandshakeAdmissionError::TableFull);
            }
        }
    }

    fn prune_expired_locked(&self, state: &mut AdmissionState, now: Instant) {
        while let Some((&(expires_at, id), _)) = state.expirations.first_key_value() {
            if expires_at > now {
                break;
            }
            state.expirations.remove(&(expires_at, id));
            if let Some(reservation) = state.reservations.get(&id) {
                if reservation.expires_at == expires_at {
                    let reservation = state
                        .reservations
                        .remove(&id)
                        .expect("reservation checked above");
                    decrement_source(&mut state.source_counts, reservation.source);
                    if let Some(identity) = reservation.identity {
                        decrement_identity(&mut state.identity_counts, identity);
                    }
                    self.counters.expired.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    fn release(&self, id: u64, expires_at: Instant) {
        let Ok(mut state) = self.state.lock() else {
            self.counters
                .rejected_unavailable
                .fetch_add(1, Ordering::Relaxed);
            return;
        };
        state.expirations.remove(&(expires_at, id));
        if let Some(reservation) = state.reservations.remove(&id) {
            decrement_source(&mut state.source_counts, reservation.source);
            if let Some(identity) = reservation.identity {
                decrement_identity(&mut state.identity_counts, identity);
            }
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        IpAddr::V4(_) => ip,
    }
}

fn decrement_source(counts: &mut HashMap<IpAddr, usize>, source: IpAddr) {
    if let Some(count) = counts.get_mut(&source) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(&source);
        }
    }
}

fn decrement_identity(counts: &mut HashMap<AdmissionIdentity, usize>, identity: AdmissionIdentity) {
    if let Some(count) = counts.get_mut(&identity) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(&identity);
        }
    }
}

/// One active admission reservation. Dropping it releases capacity.
pub(crate) struct HandshakeAdmissionPermit {
    admission: Arc<HandshakeAdmission>,
    id: u64,
    #[cfg(test)]
    identity: Option<AdmissionIdentity>,
    expires_at: Instant,
}

impl HandshakeAdmissionPermit {
    #[cfg(test)]
    pub(crate) fn identity(&self) -> Option<AdmissionIdentity> {
        self.identity
    }

    /// Whether this reservation still owns a live row in the bounded table.
    pub(crate) fn is_active(&self, now: Instant) -> bool {
        let Ok(mut state) = self.admission.state.lock() else {
            self.admission
                .counters
                .rejected_unavailable
                .fetch_add(1, Ordering::Relaxed);
            return false;
        };
        self.admission.prune_expired_locked(&mut state, now);
        state
            .reservations
            .get(&self.id)
            .is_some_and(|reservation| reservation.expires_at > now)
    }

    /// Attach an authenticated identity after a pre-authentication TLS flight.
    pub(crate) fn bind_identity(
        &mut self,
        identity: AdmissionIdentity,
        now: Instant,
    ) -> Result<(), HandshakeAdmissionError> {
        let mut state = self.admission.lock_state()?;
        self.admission.prune_expired_locked(&mut state, now);
        let Some(existing_identity) = state
            .reservations
            .get(&self.id)
            .map(|reservation| reservation.identity)
        else {
            return Err(HandshakeAdmissionError::Expired);
        };
        if let Some(existing_identity) = existing_identity {
            return if existing_identity == identity {
                Ok(())
            } else {
                Err(HandshakeAdmissionError::IdentityMismatch)
            };
        }
        if state
            .identity_counts
            .get(&identity)
            .copied()
            .unwrap_or_default()
            >= self.admission.config.per_identity
        {
            self.admission
                .counters
                .rejected_identity
                .fetch_add(1, Ordering::Relaxed);
            return Err(HandshakeAdmissionError::IdentityLimit);
        }

        state
            .reservations
            .get_mut(&self.id)
            .expect("reservation checked above")
            .identity = Some(identity);
        *state.identity_counts.entry(identity).or_default() += 1;
        #[cfg(test)]
        {
            self.identity = Some(identity);
        }
        Ok(())
    }
}

impl Drop for HandshakeAdmissionPermit {
    fn drop(&mut self) {
        self.admission.release(self.id, self.expires_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn identity(value: u8) -> AdmissionIdentity {
        [value; 32]
    }

    fn source(value: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, value))
    }

    fn guard(
        per_source: usize,
        per_identity: usize,
        global: usize,
        max_entries: usize,
        ttl: Duration,
    ) -> Arc<HandshakeAdmission> {
        HandshakeAdmission::new(HandshakeAdmissionConfig {
            per_source,
            per_identity,
            global,
            max_entries,
            ttl,
        })
        .expect("valid admission limits")
    }

    #[test]
    fn spoofed_source_flood_is_bounded_and_rejected() {
        let admission = guard(2, 64, 64, 64, Duration::from_secs(30));
        let now = Instant::now();
        let first = admission
            .try_reserve(source(1), identity(1), now)
            .expect("first reservation");
        let second = admission
            .try_reserve(source(1), identity(2), now)
            .expect("second reservation");

        for value in 3..=64 {
            assert!(matches!(
                admission.try_reserve(source(1), identity(value), now),
                Err(HandshakeAdmissionError::SourceLimit)
            ));
        }

        let metrics = admission.metrics(now);
        assert_eq!(metrics.active_reservations, 2);
        assert_eq!(metrics.source_entries, 1);
        assert_eq!(metrics.rejected_source, 62);
        assert!(first.is_active(now));
        assert!(second.is_active(now));
    }

    #[test]
    fn quota_recovers_after_ttl_without_sleeping() {
        let ttl = Duration::from_secs(5);
        let admission = guard(1, 1, 4, 4, ttl);
        let now = Instant::now();
        let permit = admission
            .try_reserve(source(1), identity(1), now)
            .expect("initial reservation");
        assert!(matches!(
            admission.try_reserve(source(1), identity(2), now),
            Err(HandshakeAdmissionError::SourceLimit)
        ));

        let after_ttl = now + ttl;
        assert!(!permit.is_active(after_ttl));
        let recovered = admission
            .try_reserve(source(1), identity(2), after_ttl)
            .expect("expired quota must recover");
        assert!(recovered.is_active(after_ttl));
        let metrics = admission.metrics(after_ttl);
        assert_eq!(metrics.active_reservations, 1);
        assert_eq!(metrics.expired, 1);
    }

    #[test]
    fn active_entries_are_never_evicted_at_the_hard_table_cap() {
        let ttl = Duration::from_secs(5);
        let admission = guard(8, 8, 8, 2, ttl);
        let now = Instant::now();
        let first = admission
            .try_reserve(source(1), identity(1), now)
            .expect("first reservation");
        let second = admission
            .try_reserve(source(2), identity(2), now)
            .expect("second reservation");

        assert!(matches!(
            admission.try_reserve(source(3), identity(3), now),
            Err(HandshakeAdmissionError::TableFull)
        ));
        assert!(first.is_active(now));
        assert!(second.is_active(now));
        assert_eq!(admission.metrics(now).rejected_table_full, 1);

        let after_ttl = now + ttl;
        let replacement = admission
            .try_reserve(source(3), identity(3), after_ttl)
            .expect("only expired rows may be reclaimed");
        assert!(replacement.is_active(after_ttl));
        let metrics = admission.metrics(after_ttl);
        assert_eq!(metrics.active_reservations, 1);
        assert_eq!(metrics.expired, 2);
    }

    #[test]
    fn global_and_identity_hard_caps_fail_closed() {
        let admission = guard(8, 2, 3, 8, Duration::from_secs(30));
        let now = Instant::now();
        let _identity_first = admission
            .try_reserve(source(1), identity(1), now)
            .expect("first identity slot");
        let _identity_second = admission
            .try_reserve(source(2), identity(1), now)
            .expect("second identity slot");
        assert!(matches!(
            admission.try_reserve(source(3), identity(1), now),
            Err(HandshakeAdmissionError::IdentityLimit)
        ));

        let _global_last = admission
            .try_reserve(source(4), identity(2), now)
            .expect("last global slot");
        assert!(matches!(
            admission.try_reserve(source(5), identity(3), now),
            Err(HandshakeAdmissionError::GlobalLimit)
        ));
        let metrics = admission.metrics(now);
        assert_eq!(metrics.active_reservations, 3);
        assert_eq!(metrics.rejected_identity, 1);
        assert_eq!(metrics.rejected_global, 1);
    }

    #[test]
    fn valid_node_progresses_under_a_short_spoofed_burst() {
        let admission = guard(2, 8, 4, 8, Duration::from_secs(30));
        let now = Instant::now();
        let _attack_one = admission
            .try_reserve(source(1), identity(1), now)
            .expect("first attack reservation");
        let _attack_two = admission
            .try_reserve(source(1), identity(2), now)
            .expect("second attack reservation");
        for value in 0..16 {
            assert!(matches!(
                admission.try_reserve(source(1), identity(10 + value), now),
                Err(HandshakeAdmissionError::SourceLimit)
            ));
        }

        let valid = admission
            .try_reserve(source(2), identity(200), now)
            .expect("valid node from another source must make progress");
        assert!(valid.is_active(now));
        assert_eq!(admission.metrics(now).rejected_source, 16);
    }

    #[test]
    fn quic_source_reservation_binds_authenticated_identity() {
        let admission = guard(4, 1, 4, 4, Duration::from_secs(30));
        let now = Instant::now();
        let mut first = admission
            .try_reserve_source(source(1), now)
            .expect("source reservation");
        first
            .bind_identity(identity(9), now)
            .expect("identity bind");
        assert_eq!(first.identity(), Some(identity(9)));

        let mut second = admission
            .try_reserve_source(source(2), now)
            .expect("source reservation");
        assert!(matches!(
            second.bind_identity(identity(9), now),
            Err(HandshakeAdmissionError::IdentityLimit)
        ));
        assert_eq!(admission.metrics(now).rejected_identity, 1);
    }

    #[test]
    fn ipv4_mapped_sources_share_the_same_quota_bucket() {
        let admission = guard(1, 8, 8, 8, Duration::from_secs(30));
        let now = Instant::now();
        let _first = admission
            .try_reserve(source(1), identity(1), now)
            .expect("IPv4 reservation");
        let mapped: Ipv6Addr = "::ffff:192.0.2.1".parse().expect("mapped address");
        assert!(matches!(
            admission.try_reserve(IpAddr::V6(mapped), identity(2), now),
            Err(HandshakeAdmissionError::SourceLimit)
        ));
    }

    #[test]
    fn reservation_id_wrap_never_evicts_an_active_row() {
        let admission = guard(4, 4, 4, 4, Duration::from_secs(30));
        let now = Instant::now();
        let first = admission
            .try_reserve(source(1), identity(1), now)
            .expect("first reservation");
        admission.state.lock().expect("admission state").next_id = first.id;

        let second = admission
            .try_reserve(source(2), identity(2), now)
            .expect("wrapped allocator must skip the active id");
        assert_ne!(second.id, first.id);
        assert!(first.is_active(now));
        assert!(second.is_active(now));
        assert_eq!(admission.metrics(now).active_reservations, 2);

        drop(first);
        assert!(second.is_active(now));
        assert_eq!(admission.metrics(now).active_reservations, 1);
    }

    #[test]
    fn invalid_configuration_is_rejected() {
        let result = HandshakeAdmission::new(HandshakeAdmissionConfig {
            per_source: 1,
            per_identity: 1,
            global: 1,
            max_entries: 1,
            ttl: Duration::ZERO,
        });
        assert!(matches!(
            result,
            Err(HandshakeAdmissionError::InvalidConfig(_))
        ));
    }
}
