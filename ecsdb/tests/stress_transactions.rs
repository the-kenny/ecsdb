//! Stress tests targeting the transaction handling introduced by the
//! `immediate_tx` (`rusqlite::Transaction::new_unchecked`) based attach/detach
//! helpers.
//!
//! `Ecs` owns a single, non-`Sync` `rusqlite::Connection`, so two threads can
//! never drive transactions on the *same* connection concurrently. The
//! interesting failure mode for `new_unchecked` is therefore not threading but
//! *reentrancy*: opening a second transaction on a connection that already has
//! one open. `new_unchecked` cannot detect this, so SQLite rejects the nested
//! `BEGIN` at runtime.
//!
//! These tests construct several ways to get multiple overlapping transactions
//! on one connection and assert the observed behaviour.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ecsdb::{Component, Ecs};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Component, Default, PartialEq, Clone)]
struct Counter(u64);

#[derive(Debug, Serialize, Deserialize, Component, Default, PartialEq, Clone)]
struct Label(String);

/// A file-backed [`Ecs`] in a unique temporary directory. The directory (and
/// the SQLite database plus its WAL/SHM sidecar files) is removed on drop.
///
/// The stress tests deliberately avoid `Ecs::open_in_memory` so that the real
/// on-disk transaction/journal machinery (WAL, `IMMEDIATE` locking) is
/// exercised.
struct TmpDb {
    dir: PathBuf,
    db: Ecs,
}

impl TmpDb {
    fn new() -> Result<Self, anyhow::Error> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ecsdb-stress-{nanos}-{seq}"));
        std::fs::create_dir_all(&dir)?;
        let db = Ecs::open(dir.join("stress.sqlite"))?;
        Ok(Self { dir, db })
    }
}

impl std::ops::Deref for TmpDb {
    type Target = Ecs;
    fn deref(&self) -> &Ecs {
        &self.db
    }
}

impl Drop for TmpDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A tight sequential loop of transactional operations on a single connection.
///
/// Every public mutating call opens its own `IMMEDIATE` transaction and commits
/// before returning, so transactions never overlap. This must complete without
/// a single error even under heavy looping.
#[test]
fn sequential_transactions_never_error() -> Result<(), anyhow::Error> {
    let db = TmpDb::new()?;

    const ITERS: u64 = 20_000;

    let entity = db.new_entity().try_attach(Counter(0))?;
    let id = entity.id();

    for i in 1..=ITERS {
        // attach (upsert) -> read-modify-write -> attach a second component ->
        // detach it. All of these go through immediate_tx internally.
        db.entity(id).try_attach(Counter(i))?;

        db.entity(id)
            .try_modify_component::<Counter>(|c| {
                c.0 += 1;
                Ok(())
            })
            .map_err(anyhow::Error::from)?;

        db.entity(id).try_attach(Label(format!("iter-{i}")))?;
        db.entity(id).try_detach::<Label>()?;
    }

    // Counter was set to ITERS then bumped once more by modify_component.
    assert_eq!(
        db.entity(id).try_component::<Counter>()?,
        Some(Counter(ITERS + 1))
    );
    assert_eq!(db.entity(id).try_component::<Label>()?, None);

    Ok(())
}

/// Reentrancy through `try_modify_component`'s closure.
///
/// `try_modify_component` opens an `IMMEDIATE` transaction, then runs the
/// user-supplied closure *inside* that transaction. If the closure performs
/// another mutating operation on the same `Ecs`, that opens a second
/// transaction on the already-busy connection. `new_unchecked` cannot detect
/// the overlap, so the inner `BEGIN` is expected to fail.
#[test]
fn reentrant_mutation_inside_modify_closure_overlaps() -> Result<(), anyhow::Error> {
    let db = TmpDb::new()?;
    let id = db.new_entity().try_attach(Counter(0))?.id();

    let result = db.entity(id).try_modify_component::<Counter>(|c| {
        c.0 += 1;
        // Reentrant mutating call: opens a nested transaction on the same
        // connection while the outer modify transaction is still open.
        db.entity(id).try_attach(Label("nested".into()))?;
        Ok(())
    });

    // The nested BEGIN cannot succeed on a connection that already has an open
    // transaction.
    assert!(
        result.is_err(),
        "nested transaction opened from within modify closure must error, got {result:?}"
    );

    // The outer transaction was rolled back when the helper returned Err, so
    // neither the increment nor the nested attach are visible.
    assert_eq!(db.entity(id).try_component::<Counter>()?, Some(Counter(0)));
    assert_eq!(db.entity(id).try_component::<Label>()?, None);

    // Crucially, the connection is still usable afterwards.
    db.entity(id).try_attach(Counter(42))?;
    assert_eq!(db.entity(id).try_component::<Counter>()?, Some(Counter(42)));

    Ok(())
}

/// Reentrancy while iterating a query.
///
/// A query iterator holds a live prepared statement (an active read cursor) on
/// the connection. Mutating an entity in the middle of iteration opens a
/// transaction while that cursor is still active, i.e. another overlapping
/// transaction on the same connection.
#[test]
fn mutation_during_query_iteration_overlaps() -> Result<(), anyhow::Error> {
    let db = TmpDb::new()?;

    for n in 0..16 {
        db.new_entity().try_attach(Counter(n))?;
    }

    let mut iterated = 0u64;
    let mut first_err: Option<ecsdb::Error> = None;

    for entity in db.try_query::<ecsdb::EntityId, ()>()? {
        iterated += 1;
        // Mutate while the iterator's statement is still live.
        if let Err(e) = db
            .entity(entity)
            .try_attach(Label(format!("seen-{entity}")))
        {
            first_err = Some(e);
            break;
        }
    }

    eprintln!(
        "query-iteration: iterated={iterated} first_err={:?}",
        first_err.as_ref().map(|e| e.to_string())
    );

    // The connection must remain usable regardless of whether the overlap
    // produced an error mid-iteration.
    let id = db.new_entity().try_attach(Counter(99))?.id();
    assert_eq!(db.entity(id).try_component::<Counter>()?, Some(Counter(99)));

    Ok(())
}

/// The most explicit form: a transaction opened directly on the connection,
/// then a public mutating call that opens its own (nested) transaction.
///
/// Documents the invariant that mutating methods must not be called while
/// another transaction is open on the same connection.
#[test]
fn nested_transaction_on_same_connection_errors() -> Result<(), anyhow::Error> {
    let db = TmpDb::new()?;

    // Open an outer transaction directly on the connection.
    db.raw_sql().execute_batch("begin immediate")?;

    // A public mutating call now tries to BEGIN a second transaction on the
    // same connection, which must fail.
    let result = db.new_entity().try_attach(Counter(1));
    assert!(
        result.is_err(),
        "nested transaction on the same connection should error, got {result:?}"
    );

    // Clean up the outer transaction so the connection is left usable.
    db.raw_sql().execute_batch("rollback")?;

    // After rollback the connection works again.
    let e = db.new_entity().try_attach(Counter(2))?;
    assert_eq!(e.try_component::<Counter>()?, Some(Counter(2)));

    Ok(())
}
