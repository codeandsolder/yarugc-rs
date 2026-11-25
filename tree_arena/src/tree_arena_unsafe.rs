// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! The types defined in this module *still* don't implement an arena.
//! Items are stored in a hashmap (and then boxed!) instead of in a buffer.
//! A future version will likely use some kind of slotmap.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use hashbrown::HashMap;

use crate::{NodeId, ReparentError, parent_path, validate_reparent};

#[derive(Debug)]
struct TreeNode<T> {
    item: T,
    children: Vec<NodeId>,
}

/// Mapping of data for the Tree Arena
#[derive(Debug)]
struct DataMap<T> {
    /// The items in the tree
    items: HashMap<NodeId, Box<UnsafeCell<TreeNode<T>>>>,
    /// The parent of each node, or `None` if it is the root
    parents: HashMap<NodeId, Option<NodeId>>,
}

/// A container type for a tree of items.
///
/// This type is used to store zero, one or many trees of a given item type. It
/// will keep track of parent-child relationships, lets you efficiently find
/// an item anywhere in the tree hierarchy, and give you mutable access to this item
/// and its children.
#[derive(Debug)]
pub struct TreeArena<T> {
    /// The items in the tree
    data_map: DataMap<T>,
    /// The roots of the tree
    roots: Vec<NodeId>,
}

define_node_handles!();

/// A handle giving shared access to a set of arena items.
///
/// See [`ArenaRef`] for more information.
#[derive(Debug)]
pub struct ArenaRefList<'arena, T> {
    /// The associated data arena
    parent_arena: &'arena DataMap<T>,
    child_ids: &'arena [NodeId],
    /// The parent id for the items
    parent_id: Option<NodeId>,
}

/// A handle giving mutable access to a set of arena items.
///
/// See [`ArenaMut`] for more information.
#[derive(Debug)]
pub struct ArenaMutList<'arena, T> {
    /// The associated data arena
    parent_arena: &'arena mut DataMap<T>,
    /// The parent id for these items
    parent_id: Option<NodeId>,
    /// Array of items
    child_arr: &'arena mut Vec<NodeId>,
}

// -- MARK: IMPLS

impl<T> Clone for ArenaRefList<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Item> Copy for ArenaRefList<'_, Item> {}

impl<T> DataMap<T> {
    fn new() -> Self {
        Self {
            items: HashMap::new(),
            parents: HashMap::new(),
        }
    }

    fn find_node(&self, id: NodeId) -> Option<&TreeNode<T>> {
        let node_cell = self.items.get(&id)?;

        #[expect(
            unsafe_code,
            reason = "Access the node through its shared arena borrow"
        )]
        // SAFETY: We need there to be no mutable access to the node
        // Mutable access to the node would imply there is some &mut self
        // As we are taking &self, there can be no mutable access to the node
        // Thus this is safe
        let node = unsafe { &*node_cell.get() };
        Some(node)
    }

    /// Finds an item in the tree.
    ///
    /// Returns a shared reference to the item if present.
    ///
    /// Time Complexity O(1)
    fn find_inner(&self, id: NodeId) -> Option<ArenaRef<'_, T>> {
        let parent_id = *self.parents.get(&id)?;

        let TreeNode {
            item,
            children: child_ids,
        } = self.find_node(id)?;

        let children = ArenaRefList {
            parent_arena: self,
            child_ids,
            parent_id: Some(id),
        };

        Some(ArenaRef {
            id,
            parent_id,
            item,
            children,
        })
    }

    /// Finds an item in the tree.
    ///
    /// Returns a mutable reference to the item if present.
    ///
    /// Time Complexity O(1)
    fn find_mut_inner(&mut self, id: NodeId) -> Option<ArenaMut<'_, T>> {
        let parent_id = *self.parents.get(&id)?;
        let node_cell = self.items.get(&id)?;

        #[expect(
            unsafe_code,
            reason = "Access the node through its exclusive arena borrow"
        )]
        // SAFETY: When using this on [`ArenaMutList`] associated with some node,
        // must ensure that `id` is a descendant of that node, otherwise can
        // obtain two mutable references to the same node
        //
        // Similarly we cannot take any other actions that would affect this node,
        // such as removing it or removing a parent (and thus this node) or violate
        // exclusivity by creating a shared reference to the node
        let TreeNode { item, children } = unsafe { &mut *node_cell.get() };

        let children = ArenaMutList {
            parent_arena: self,
            parent_id: Some(id),
            child_arr: children,
        };

        Some(ArenaMut {
            id,
            parent_id,
            item,
            children,
        })
    }

    /// Returns the path of items from the given item to the root of the tree.
    ///
    /// The path is in order from the bottom to the top, starting at the given item and ending at
    /// the root.
    ///
    /// If `start_id` is Some, the path ends just before that id instead; `start_id` is not included.
    ///
    /// If there is no path from `start_id` to id, returns the empty vector.
    fn get_id_path(&self, id: NodeId, start_id: Option<NodeId>) -> Vec<NodeId> {
        parent_path(&self.parents, id, start_id)
    }
}

impl<T> Default for TreeArena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> TreeArena<T> {
    /// Creates a new empty tree
    #[must_use = "The newly created arena must be retained to store nodes."]
    pub fn new() -> Self {
        Self {
            data_map: DataMap::new(),
            roots: Vec::new(),
        }
    }

    /// Returns the number of items in the arena.
    #[must_use = "Returns the number of stored nodes."]
    pub fn len(&self) -> usize {
        self.data_map.items.len()
    }

    /// Returns `true` if the arena is empty.
    #[must_use = "Reports whether the arena contains any nodes."]
    pub fn is_empty(&self) -> bool {
        self.data_map.items.is_empty()
    }

    /// Returns a handle whose children are the roots, if any, of the tree.
    #[must_use = "Use the returned handle to inspect the roots."]
    pub const fn roots(&self) -> ArenaRefList<'_, T> {
        ArenaRefList {
            parent_arena: &self.data_map,
            child_ids: self.roots.as_slice(),
            parent_id: None,
        }
    }

    /// Returns a handle whose children are the roots, if any, of the tree.
    ///
    /// Using [`insert`](ArenaMutList::insert) on this handle
    /// will add a new root to the tree.
    pub const fn roots_mut(&mut self) -> ArenaMutList<'_, T> {
        // safe as the roots are derived from the arena itself (same as safety for find for non root nodes)
        let roots = &mut self.roots;
        ArenaMutList {
            parent_arena: &mut self.data_map,
            parent_id: None,
            child_arr: roots,
        }
    }

    /// An iterator visiting all root ids in arbitrary order.
    pub fn root_ids(&self) -> impl Iterator<Item = NodeId> {
        self.roots.iter().copied()
    }

    /// Finds an item in the tree.
    ///
    /// Returns a shared reference to the item if present.
    ///
    /// # Complexity
    ///
    /// - O(Depth) in the safe implementation.
    /// - O(1) in the unsafe implementation.
    pub fn find(&self, id: impl Into<NodeId>) -> Option<ArenaRef<'_, T>> {
        self.data_map.find_inner(id.into())
    }

    /// Finds an item in the tree.
    ///
    /// Returns a mutable reference to the item if present.
    ///
    /// # Complexity
    ///
    /// - O(Depth) in the safe implementation.
    /// - O(1) in the unsafe implementation.
    pub fn find_mut(&mut self, id: impl Into<NodeId>) -> Option<ArenaMut<'_, T>> {
        // safe as derived from the arena itself and has assoc lifetime with the arena
        self.data_map.find_mut_inner(id.into())
    }

    /// Returns the path of items from the given item to the root of the tree.
    ///
    /// The path is in order from the bottom to the top, starting at the given item and ending at
    /// the root.
    ///
    /// If the id is not in the tree, returns an empty vector.
    pub fn get_id_path(&self, id: impl Into<NodeId>) -> Vec<NodeId> {
        self.data_map.get_id_path(id.into(), None)
    }

    /// Moves the given child and its entire subtree to the new parent.
    ///
    /// # Errors
    ///
    /// Returns an error if either node is missing or the move would create a cycle.
    /// Invalid requests leave the arena unchanged.
    pub fn reparent(
        &mut self,
        child: impl Into<NodeId>,
        new_parent: impl Into<NodeId>,
    ) -> Result<(), ReparentError> {
        let child = child.into();
        let new_parent = new_parent.into();
        let old_parent = validate_reparent(&self.data_map.parents, child, new_parent)?;
        if old_parent == Some(new_parent) {
            return Ok(());
        }
        let parent = self
            .data_map
            .parents
            .get_mut(&child)
            .ok_or(ReparentError::InvalidTopology)?;
        if let Some(old_parent) = old_parent {
            let [Some(old), Some(new)] = self
                .data_map
                .items
                .get_disjoint_mut([&old_parent, &new_parent])
            else {
                return Err(ReparentError::InvalidTopology);
            };
            let siblings = &mut old.get_mut().children;
            if !siblings.contains(&child) {
                return Err(ReparentError::InvalidTopology);
            }
            siblings.retain(|id| *id != child);
            new.get_mut().children.push(child);
        } else {
            let new = self
                .data_map
                .items
                .get_mut(&new_parent)
                .ok_or(ReparentError::InvalidTopology)?;
            if !self.roots.contains(&child) {
                return Err(ReparentError::InvalidTopology);
            }
            self.roots.retain(|id| *id != child);
            new.get_mut().children.push(child);
        }
        *parent = Some(new_parent);
        Ok(())
    }
}

impl<'arena, T> ArenaRefList<'arena, T> {
    fn ids(&self) -> impl Iterator<Item = NodeId> {
        self.child_ids.iter().copied()
    }

    /// Checks if id is a descendant of self
    /// O(depth) and the limiting factor for find methods
    /// not from the root
    fn is_descendant(&self, id: NodeId) -> bool {
        let Some(mut current) = self.parent_arena.parents.get(&id).copied() else {
            return false;
        };
        if self.parent_id.is_none() {
            return true;
        }
        while let Some(parent) = current {
            if current == self.parent_id {
                return true;
            }
            let Some(next) = self.parent_arena.parents.get(&parent) else {
                return false;
            };
            current = *next;
        }
        false
    }

    /// Returns `true` if the list has an element with the given id.
    pub fn has(self, id: impl Into<NodeId>) -> bool {
        let child_id = id.into();
        let parent_id = self.parent_id;
        self.parent_arena
            .parents
            .get(&child_id)
            .is_some_and(|parent| *parent == parent_id)
    }

    /// Returns a shared handle to the element of the list with the given id.
    ///
    /// Returns a new [`ArenaRef`].
    pub fn item(self, id: impl Into<NodeId>) -> Option<ArenaRef<'arena, T>> {
        let id = id.into();
        if self.has(id) {
            self.parent_arena.find_inner(id)
        } else {
            None
        }
    }

    /// Finds an arena item among the list's items and their descendants.
    ///
    /// Returns a shared reference to the item if present.
    ///
    /// # Complexity
    ///
    /// - O(Depth) in the safe implementation.
    /// - O(Depth) in the unsafe implementation, when not called from the root.
    /// - O(1) in the unsafe implementation, when called from the root.
    pub fn find(self, id: impl Into<NodeId>) -> Option<ArenaRef<'arena, T>> {
        // the id to search for
        let id: NodeId = id.into();

        if self.is_descendant(id) {
            self.parent_arena.find_inner(id)
        } else {
            None
        }
    }
}

impl<'arena, T> ArenaMutList<'arena, T> {
    /// Checks if id is a descendant of self
    /// O(depth) and the limiting factor for find methods
    /// not from the root
    fn is_descendant(&self, id: NodeId) -> bool {
        self.reborrow().is_descendant(id)
    }

    /// Returns `true` if the list has an element with the given id.
    pub fn has(&self, id: impl Into<NodeId>) -> bool {
        self.reborrow().has(id)
    }

    /// Returns a shared handle to the element of the list with the given id.
    pub fn item(&self, id: impl Into<NodeId>) -> Option<ArenaRef<'_, T>> {
        let id = id.into();
        if self.has(id) {
            self.parent_arena.find_inner(id)
        } else {
            None
        }
    }

    /// Returns a mutable handle to the element of the list with the given id.
    pub fn item_mut(&mut self, id: impl Into<NodeId>) -> Option<ArenaMut<'_, T>> {
        let id = id.into();
        if self.has(id) {
            // safe as we check the node is a direct child node
            self.parent_arena.find_mut_inner(id)
        } else {
            None
        }
    }

    /// Returns a shared handle to the element of the list with the given id.
    ///
    /// This is the same as [`item`](Self::item), except it consumes the
    /// handle. This is sometimes necessary to accommodate the borrow checker.
    pub fn into_item(self, id: impl Into<NodeId>) -> Option<ArenaRef<'arena, T>> {
        let id = id.into();
        if self.has(id) {
            self.parent_arena.find_inner(id)
        } else {
            None
        }
    }

    /// Returns a mutable handle to the element of the list with the given id.
    ///
    /// This is the same as [`item_mut`](Self::item_mut), except it consumes
    /// the handle. This is sometimes necessary to accommodate the borrow checker.
    pub fn into_item_mut(self, id: impl Into<NodeId>) -> Option<ArenaMut<'arena, T>> {
        let id = id.into();
        if self.has(id) {
            // safe as we check the node is a direct child node
            self.parent_arena.find_mut_inner(id)
        } else {
            None
        }
    }

    // TODO - Remove the child_id argument once creation of widgets is figured out.
    // Return the id instead.
    // TODO - Add #[must_use]
    /// Insert a child into the tree under the common parent of this list's items.
    ///
    /// If this list was returned from [`TreeArena::roots_mut()`], create a new tree root.
    ///
    /// The new child will have the given id.
    ///
    /// Returns a handle to the new child.
    ///
    /// # Panics
    ///
    /// If the arena already contains an item with the given id.
    pub fn insert(&mut self, child_id: impl Into<NodeId>, value: T) -> ArenaMut<'_, T> {
        let child_id: NodeId = child_id.into();
        assert!(
            !self.parent_arena.parents.contains_key(&child_id),
            "Key already present"
        );

        self.parent_arena.parents.insert(child_id, self.parent_id);

        self.child_arr.push(child_id);

        let node = TreeNode {
            item: value,
            children: Vec::new(),
        };

        let cell = self
            .parent_arena
            .items
            .entry(child_id)
            .insert(Box::new(UnsafeCell::new(node)))
            .into_mut();
        let pointer = cell.get();
        #[expect(
            unsafe_code,
            reason = "Borrow the newly allocated node and its disjoint descendant storage"
        )]
        // SAFETY: This is the newly inserted node, with no existing references.
        // The allocation stays stable while its descendant handle borrows the arena.
        let TreeNode { item, children } = unsafe { &mut *pointer };
        ArenaMut {
            id: child_id,
            parent_id: self.parent_id,
            item,
            children: ArenaMutList {
                parent_arena: self.parent_arena,
                parent_id: Some(child_id),
                child_arr: children,
            },
        }
    }

    // TODO - How to handle when a subtree is removed?
    // Move children to the root?
    // Should this be must use?
    /// Removes the item with the given id from the arena.
    ///
    /// If the id isn't in the list (even if it's e.g. a descendant), does nothing
    /// and returns `None`.
    ///
    /// Else, returns the removed item.
    ///
    /// This will also silently remove any recursive grandchildren of this item.
    #[must_use = "Use the returned value if the removed node payload is needed."]
    pub fn remove(&mut self, child_id: impl Into<NodeId>) -> Option<T> {
        let child_id = child_id.into();
        if !self.has(child_id) {
            return None;
        }
        let node = self.parent_arena.items.remove(&child_id)?.into_inner();
        self.child_arr.retain(|id| *id != child_id);
        self.parent_arena.parents.remove(&child_id);
        let mut pending = node.children;
        while let Some(id) = pending.pop() {
            if let Some(child) = self.parent_arena.items.remove(&id) {
                pending.extend(child.into_inner().children);
            }
            self.parent_arena.parents.remove(&id);
        }
        Some(node.item)
    }

    /// Returns a shared handle equivalent to this one.
    #[must_use = "Use the returned shared handle to inspect the arena."]
    pub const fn reborrow(&self) -> ArenaRefList<'_, T> {
        ArenaRefList {
            parent_arena: self.parent_arena,
            child_ids: self.child_arr.as_slice(),
            parent_id: self.parent_id,
        }
    }

    /// Returns a mutable handle equivalent to this one.
    ///
    /// This is sometimes useful to work with the borrow checker.
    pub const fn reborrow_mut(&mut self) -> ArenaMutList<'_, T> {
        ArenaMutList {
            parent_arena: self.parent_arena,
            parent_id: self.parent_id,
            child_arr: self.child_arr,
        }
    }

    /// Finds an arena item among the list's items and their descendants.
    ///
    /// Returns a shared reference to the item if present.
    ///
    /// # Complexity
    ///
    /// - O(Depth) in the safe implementation.
    /// - O(Depth) in the unsafe implementation, when not called from the root.
    /// - O(1) in the unsafe implementation, when called from the root.
    pub fn find(&self, id: impl Into<NodeId>) -> Option<ArenaRef<'_, T>> {
        self.reborrow().find(id)
    }

    /// Finds an arena item among the list's items and their descendants.
    ///
    /// Returns a mutable reference to the item if present.
    ///
    /// # Complexity
    ///
    /// - O(Depth) in the safe implementation.
    /// - O(Depth) in the unsafe implementation, when not called from the root.
    /// - O(1) in the unsafe implementation, when called from the root.
    pub fn find_mut(self, id: impl Into<NodeId>) -> Option<ArenaMut<'arena, T>> {
        let id = id.into();
        if self.is_descendant(id) {
            // safe as we check the node is a descendant
            self.parent_arena.find_mut_inner(id)
        } else {
            None
        }
    }

    /// Used in tests to simulate a call to `Self::insert` or `Self::remove` that
    /// triggers a realloc.
    ///
    /// This is an unstable API which can only be used in tests of the `tree_arena` crate itself,
    /// and may change in any release.
    /// It is used to surface potential Use-After-Free (UAF) errors in the code.
    #[doc(hidden)]
    pub fn realloc_inner_storage(&mut self) {
        // By doubling the required capacity (plus a small constant for small capacities),
        // we hopefully guarantee that a reallocation will happen no matter the original capacity.
        let capacity = self.parent_arena.items.capacity();
        let capacity = core::hint::black_box(capacity);
        self.parent_arena.items.reserve(capacity + 32);

        // We try to discard the extra memory.
        // We use black_box to hide the fact that the above call to reserve could be elided.
        if core::hint::black_box(true) {
            self.parent_arena.items.shrink_to_fit();
        }
    }
}
