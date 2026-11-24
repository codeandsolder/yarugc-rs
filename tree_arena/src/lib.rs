// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

// After you edit the crate's doc comment, run this command, then check README.md for any missing links
// cargo rdme --workspace-project=tree_arena

//! This crate contains two implementations of a tree for use in [Masonry][], one safe and the other unsafe. The safe tree is known to work, and serves as the baseline implementation and is used by default.
//! The unsafe tree leverages a hashmap as an arena and is designed for higher performance: it leverages unsafe code to achieve this. The unsafe tree is not yet fully tested, and is not used by default.
//!
//! The safe tree is the priority. This means:
//!
//! * The safe version may have features / APIs that the unsafe version doesn't yet have.
//! * If both versions are at feature parity, [Masonry][] can switch on the unsafe version for best performance.
//! * Otherwise, [Masonry][] uses the safe version.
//!
//! # Features
//!
//! The following crate [feature flags](https://doc.rust-lang.org/cargo/reference/features.html#dependency-features) are available:
//!
//! - `std` (enabled by default): Enable future features which require the standard library.
//!   This feature is provided for forwards compatibility only, and current behaviour is the same whether or not it is enabled.
//! - `safe_tree` (enabled by default): Use the safe tree implementation instead of the unsafe one.
//!
//! This crate is `no_std` compatible; to use it without the standard library, disable default features.
//! Note that this also disables `safe_tree`, so re-enable it explicitly if you want the safe implementation.
//!
//! # Architecture
//!
//! Reparenting validates the entire move before changing parent links and returns a typed error for missing nodes or cycles. Node handles retain the identity of their payload independently of their child-list handles.
//!
//! ## Safe Tree
//!
//! The safe tree contains a root `TreeArena` which owns the root nodes as `HashMap<NodeId, TreeNode<T>>`, and a `parents_map` tracking the parent of every node.
//! Each `TreeNode` subsequently owns its own children as `HashMap<NodeId, TreeNode<T>>`. This model of ownership is thus checked by the rust compiler,
//! but has the downside of requiring passing through every ancestor node to access the descendant -
//! this requires an O(depth) determination of whether the node is a descendant, followed by expected O(1) lookup at each level to traverse the path to the child.
//!
//! ## Unsafe Tree
//!
//! The unsafe tree arena contains a `DataMap` which **owns** all nodes. The `DataMap` contains:
//!
//! * A `HashMap` associating `NodeId` with `Box<UnsafeCell<TreeNode<T>>>`, owning the node data (boxed to prevent movement of the node when the `HashMap` is resized and `UnsafeCell` to express the interior mutability);
//! * A `HashMap` associating `NodeId` with `Option<NodeId>`, containing the parent information for the nodes.
//!
//! The `TreeArena` also stores a `Vec<NodeId>` containing the roots of the tree.
//!
//! It is possible to get shared (immutable) access or exclusive (mutable) access to the tree. These return `ArenaRef<'arena, T>` or `ArenaMut<'arena, T>` respectively.
//! We do this by leveraging a hash map to store the nodes: from this we can obtain either shared or exclusive access to nodes.
//! To ensure that only one item is allowed to create new exclusive access to nodes, this action requires mutable access to the arena as a whole (and so is checked by the compiler) -
//! what the compiler cannot check is that the nodes accessed mutably are distinct from one another - this is done by only allowing access to descendants of the node being accessed mutably.
//! The aim of this is to reduce the time needed to access node, as given a node, we only need to determine whether it is a descendant of the node being accessed mutably,
//! and do not need to iterate over the children and to flatten the overall tree graph into a hash map.
//!
//! ### Shared References
//!
//! `ArenaRef<'arena, T>` contains the identity of the parent node, a reference to the node data, and `ArenaRefChildren<'arena, T>`.
//! The `ArenaRefChildren<'arena, T>` contains the ids of the children of the node, the id of the node, and a reference to the arena. From this `ArenaRefChildren<'arena, T>` it is possible to get shared access to children of the node.
//!
//! ### Exclusive References
//!
//! `ArenaMut<'arena, T>` contains the identity of the parent node, a mutable reference to the node data, and `ArenaMutChildren<'arena, T>`.
//! The `ArenaMutChildren<'arena, T>` contains the ids of the children of the node, the id of the node, and a mutable reference to the arena.
//! From this `ArenaMutChildren<'arena, T>` it is possible to get exclusive access to children of the node.
//!
//! ### Safety
//!
//! From the `ArenaMutChildren<'arena, T>`, it is important that we can only access descendants of that node,
//! such that we can only ever have exclusive mutable access to the contents of a node, and never have multiple mutable references.
//! This invariant is not checked by the compiler and thus relies on the logic to determine whether a node is a descendant being correct.
//!
//! ## Complexity
//!
//! |Operation  | Safe         | Unsafe   |
//! |   ---     |      ---     |   ---    |
//! |Find child | O(1)         | O(1)     |
//! |Descendant | O(Depth)     | O(Depth) |
//! |From root  | O(Depth)     | O(1)     |
//!
//! [Masonry]: https://crates.io/crates/masonry

// LINEBENDER LINT SET - lib.rs - v3
// See https://linebender.org/wiki/canonical-lints/
// These lints shouldn't apply to examples or tests.
#![cfg_attr(not(test), deny(unused_crate_dependencies))]
// These lints shouldn't apply to examples.
#![deny(clippy::print_stdout, clippy::print_stderr)]
// Targeting e.g. 32-bit means structs containing usize can give false positives for 64-bit.
#![cfg_attr(target_pointer_width = "64", deny(clippy::trivially_copy_pass_by_ref))]
// END LINEBENDER LINT SET
#![cfg_attr(docsrs, feature(doc_cfg))]
#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
// Ensure that we don't compile if you're using the std feature on a platform without `std`
extern crate std as _;

type NodeId = u64;

use alloc::vec::Vec;
use core::fmt;
use hashbrown::HashMap;

/// Why a subtree could not be moved to a new parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReparentError {
    /// The requested child does not exist.
    MissingChild(u64),
    /// The requested new parent does not exist.
    MissingParent(u64),
    /// The move would make a node its own ancestor.
    Cycle {
        /// Node being moved.
        child: u64,
        /// Requested parent.
        new_parent: u64,
    },
    /// The stored parent links and node storage disagree.
    InvalidTopology,
}

impl fmt::Display for ReparentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingChild(id) => write!(f, "no node found for child id #{id}"),
            Self::MissingParent(id) => write!(f, "no node found for new_parent id #{id}"),
            Self::Cycle { child, new_parent } => {
                write!(f, "cannot move node #{child} below #{new_parent}: cycle")
            }
            Self::InvalidTopology => f.write_str("arena parent links disagree with node storage"),
        }
    }
}

impl core::error::Error for ReparentError {}

/// Walks the parent map up to (but excluding) the requested ancestor.
fn parent_path(
    parents: &HashMap<NodeId, Option<NodeId>>,
    id: NodeId,
    ancestor: Option<NodeId>,
) -> Vec<NodeId> {
    let mut path = Vec::new();
    let mut current = Some(id);
    while let Some(node) = current {
        if current == ancestor {
            return path;
        }
        let Some(parent) = parents.get(&node) else {
            return Vec::new();
        };
        path.push(node);
        current = *parent;
    }
    if ancestor.is_none() { path } else { Vec::new() }
}

/// Validate the request before either implementation mutates its storage.
fn validate_reparent(
    parents: &HashMap<NodeId, Option<NodeId>>,
    child: NodeId,
    new_parent: NodeId,
) -> Result<Option<NodeId>, ReparentError> {
    if child == new_parent {
        return Err(ReparentError::Cycle { child, new_parent });
    }
    let old_parent = *parents
        .get(&child)
        .ok_or(ReparentError::MissingChild(child))?;
    let mut current = Some(new_parent);
    while let Some(node) = current {
        if node == child {
            return Err(ReparentError::Cycle { child, new_parent });
        }
        current = *parents
            .get(&node)
            .ok_or(ReparentError::MissingParent(node))?;
    }
    Ok(old_parent)
}

// Both storage backends expose the same payload handles. Keep identity and
// borrow semantics defined once, independent of the child-list storage.
macro_rules! define_node_handles {
    () => {
        /// A reference type giving shared access to an arena item and its children.
        ///
        /// When you borrow an item from a [`TreeArena`], it returns an `ArenaRef`.
        /// You can iterate over its children to get access to child `ArenaRef` handles.
        #[derive(Debug)]
        pub struct ArenaRef<'arena, T> {
            id: NodeId,
            /// The parent of this node
            pub parent_id: Option<NodeId>,
            /// The payload of the node
            pub item: &'arena T,
            /// Reference to the children of the node
            pub children: ArenaRefList<'arena, T>,
        }

        /// A reference type giving mutable access to an arena item and its children.
        ///
        /// When you borrow an item from a [`TreeArena`], it returns an `ArenaMut`.
        /// This struct holds four fields:
        ///  - the identity of the node and the id of its parent.
        ///  - a reference to the item itself.
        ///  - an [`ArenaMutList`] handle to access its children.
        ///
        /// Because the latter two are disjoint references, you can mutate the node's value
        /// and its children independently without invalidating the references.
        ///
        /// You can iterate over its children to get access to child `ArenaMut` handles.
        #[derive(Debug)]
        pub struct ArenaMut<'arena, T> {
            id: NodeId,
            /// The parent of the node
            pub parent_id: Option<NodeId>,
            /// The payload of the node
            pub item: &'arena mut T,
            /// Reference to the children of the node
            pub children: ArenaMutList<'arena, T>,
        }

        impl<T> Clone for ArenaRef<'_, T> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<T> Copy for ArenaRef<'_, T> {}

        impl<T> ArenaRef<'_, T> {
            /// Id of the item this handle is associated with.
            #[must_use = "Identifies the node associated with this handle."]
            pub const fn id(&self) -> NodeId {
                self.id
            }

            /// An iterator visiting all child ids in arbitrary order.
            #[must_use = "Iterate the returned child identifiers."]
            pub fn child_ids(&self) -> impl IntoIterator<Item = NodeId> {
                self.children.ids()
            }
        }

        impl<T> ArenaMut<'_, T> {
            /// Id of the item this handle is associated with.
            #[must_use = "Identifies the node associated with this handle."]
            pub const fn id(&self) -> NodeId {
                self.id
            }

            /// Returns a shared reference equivalent to this one.
            #[must_use = "Use the returned shared handle to inspect the node."]
            pub const fn reborrow(&self) -> ArenaRef<'_, T> {
                ArenaRef {
                    id: self.id,
                    parent_id: self.parent_id,
                    item: self.item,
                    children: self.children.reborrow(),
                }
            }

            /// Returns a mutable reference equivalent to this one.
            ///
            /// This is sometimes useful to work with the borrow checker.
            pub const fn reborrow_mut(&mut self) -> ArenaMut<'_, T> {
                ArenaMut {
                    id: self.id,
                    parent_id: self.parent_id,
                    item: self.item,
                    children: self.children.reborrow_mut(),
                }
            }
        }
    };
}

// We instantiate both modules when `cfg(test)` is true.
// This tricks IDEs (at least VSCode) into always building both modules
// and giving us completions, jump to definition, etc, at all times.
// We make both modules `pub` and `doc(hidden)` so we don't get warnings about unused items.

#[cfg(any(test, feature = "safe_tree"))]
#[doc(hidden)]
pub mod tree_arena_safe;
#[cfg(any(test, not(feature = "safe_tree")))]
#[doc(hidden)]
pub mod tree_arena_unsafe;

#[cfg(feature = "safe_tree")]
pub use tree_arena_safe::*;
#[cfg(not(feature = "safe_tree"))]
pub use tree_arena_unsafe::*;
