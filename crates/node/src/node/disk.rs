//! The node's own writes to its store, off the async workers.
//!
//! A synced write (a blob, a file segment, a node row) waits for the disk's fsync, and
//! RocksDB writes in groups: any write that arrives while another one, of any group, is in
//! its fsync joins or queues behind it. Every Raft group of the node, the system group and
//! the HTTP handlers share the async workers, so one such write left on a worker holds them
//! all up for as long as the disk takes (the log keeps its own writes off them the same way,
//! `calimero_log::store::off_workers`). Each write here runs through [`off_workers`]
//! (`block_in_place`, which hands the worker's other tasks to another worker, so it takes
//! borrowed bytes and works from sync code inside a run), and its caller still waits for it:
//! durable before it answers, in the order it was called.
//!
//! The node reaches the store's writes only through [`Disk`]: `crates/node/clippy.toml`
//! disallows calling them on [`Engine`] anywhere else.
//!
//! Other disk work runs off the workers where it is done: recovery and backup file reads and
//! the system state's seeding in `restore.rs` (`off_workers`), a view's run and
//! `sweep_segments` (`spawn_blocking`). Store reads stay inline, as the log's do.
#![allow(clippy::disallowed_methods)]

use calimero_log::Engine;
use serde::Serialize;

use super::blob_guards::{BlobGuards, Deletion};
use super::{off_workers, Node};

/// The store's writes, each off the async workers ([`off_workers`]).
#[derive(Clone, Copy)]
pub(crate) struct Disk<'a>(&'a Engine);

impl<'a> Disk<'a> {
    pub(crate) const fn of(engine: &'a Engine) -> Self {
        Self(engine)
    }

    /// Store code or a blob no deletion races (a bundle, a package release, a blob fetched
    /// from a peer).
    pub(crate) fn put_blob(self, id: &str, bytes: &[u8]) -> eyre::Result<()> {
        off_workers(|| self.0.put_blob(id, bytes))
    }

    /// Store a blob a deletion of the same id may race ([`BlobGuards::put`]).
    pub(crate) fn put_guarded_blob(
        self,
        guards: &BlobGuards,
        id: &str,
        bytes: &[u8],
    ) -> eyre::Result<()> {
        off_workers(|| guards.put(id, || self.0.put_blob(id, bytes)))
    }

    /// Delete blob `id` for `deletion`, unless a put of it landed since ([`Deletion::delete`]).
    pub(crate) fn delete_blob(self, deletion: &Deletion, id: &str) -> Option<eyre::Result<()>> {
        off_workers(|| deletion.delete(|| self.0.delete_blob(id)))
    }

    pub(crate) fn put_segment(self, hash: &str, bytes: &[u8]) -> eyre::Result<()> {
        off_workers(|| self.0.put_segment(hash, bytes))
    }

    pub(crate) fn delete_segment(self, hash: &str) -> eyre::Result<()> {
        off_workers(|| self.0.delete_segment(hash))
    }

    /// Delete each of `hashes`, calling `failed` for each that fails: one move off the
    /// workers for them all.
    pub(crate) fn delete_segments<'h>(
        self,
        hashes: impl IntoIterator<Item = &'h str>,
        mut failed: impl FnMut(&str, eyre::Report),
    ) {
        off_workers(|| {
            for hash in hashes {
                if let Err(e) = self.0.delete_segment(hash) {
                    failed(hash, e);
                }
            }
        });
    }

    pub(crate) fn put_node_meta<T: Serialize>(self, name: &str, value: &T) -> eyre::Result<()> {
        off_workers(|| self.0.put_node_meta(name, value))
    }
}

impl Node {
    /// This node's store writes, off the async workers.
    pub(crate) fn disk(&self) -> Disk<'_> {
        Disk::of(&self.engine)
    }
}
