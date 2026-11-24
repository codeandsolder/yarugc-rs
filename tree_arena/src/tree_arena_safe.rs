// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! The types defined in this module don't *actually* implement an arena. They use
//! 100% safe code, which has a significant performance overhead.
//! The version in `tree_arena_unsafe.rs` uses unsafe code, but should have the
//! exact same exported API as this module.

use alloc::vec::Vec;

use hashbrown::HashMap;

use crate::{NodeId, ReparentError, parent_path, validate_reparent};

#[derive(Debug)]
struct TreeNode<T> {
    id: NodeId,
    item: T,
    children: HashMap<NodeId, Self>,
}

/// A container type for a tree of items.
///
/// This type is used to store zero, one or many trees of a given item type. It
/// will keep track of parent-child relationships, lets you efficiently find
/// an item anywhere in the tree hierarchy, and give you mutable access to this item
/// and its children.
#[derive(Debug)]
pub struct TreeArena<T> {
    roots: HashMap<NodeId, TreeNode<T>>,
    parents_map: HashMap<NodeId, Option<NodeId>>,
}

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

/// A handle giving shared access to a set of arena items.
///
/// See [`ArenaRef`] for more information.
#[derive(Debug)]
pub struct ArenaRefList<'arena, T> {
    parent_id: Option<NodeId>,
    children: &'arena HashMap<NodeId, TreeNode<T>>,
    parents_map: ArenaMapRef<'arena>,
}

/// A reference type giving mutable access to an arena item and its children.
///
/// When you borrow an item from a [`TreeArena`], it returns an `ArenaMut`.
/// This struct holds three fields:
///  - the id of its parent.
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

/// A handle giving mutable access to a set of arena items.
///
/// See [`ArenaMut`] for more information.
#[derive(Debug)]
pub struct ArenaMutList<'arena, T> {
    parent_id: Option<NodeId>,
    children: &'arena mut HashMap<NodeId, TreeNode<T>>,
    parents_map: ArenaMapMut<'arena>,
}

/// A shared reference to the parent father map
#[derive(Clone, Copy, Debug)]
struct ArenaMapRef<'arena> {
    parents_map: &'arena HashMap<NodeId, Option<NodeId>>,
}

/// A mutable reference to the parent father map
#[derive(Debug)]
struct ArenaMapMut<'arena> {
    parents_map: &'arena mut HashMap<NodeId, Option<NodeId>>,
}

// -- MARK: IMPLS

impl<T> Clone for ArenaRef<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ArenaRef<'_, T> {}

impl<T> Clone for ArenaRefList<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ArenaRefList<'_, T> {}

impl<T> Default for TreeArena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> TreeArena<T> {
    /// Creates an empty tree.
    #[must_use = "The newly created arena must be retained to store nodes."]
    pub fn new() -> Self {
        Self {
            roots: HashMap::new(),
            parents_map: HashMap::new(),
        }
    }

    /// Returns the number of items in the arena.
    #[must_use = "Returns the number of stored nodes."]
    pub fn len(&self) -> usize {
        self.parents_map.len()
    }

    /// Returns `true` if the arena is empty.
    #[must_use = "Reports whether the arena contains any nodes."]
    pub fn is_empty(&self) -> bool {
        self.parents_map.is_empty()
    }

    /// Returns a handle giving access to the roots of the tree.
    #[must_use = "Use the returned handle to inspect the roots."]
    pub const fn roots(&self) -> ArenaRefList<'_, T> {
        ArenaRefList {
            parent_id: None,
            children: &self.roots,
            parents_map: ArenaMapRef {
                parents_map: &self.parents_map,
            },
        }
    }

    /// Returns a handle giving access to the roots of the tree.
    ///
    /// Using [`insert`](ArenaMutList::insert) on this handle
    /// will add a new root to the tree.
    pub const fn roots_mut(&mut self) -> ArenaMutList<'_, T> {
        ArenaMutList {
            parent_id: None,
            children: &mut self.roots,
            parents_map: ArenaMapMut {
                parents_map: &mut self.parents_map,
            },
        }
    }

    /// An iterator visiting all root ids in arbitrary order.
    pub fn root_ids(&self) -> impl Iterator<Item = NodeId> {
        self.roots.keys().copied()
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
        self.roots().find_inner(id.into())
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
        self.roots_mut().find_mut_inner(id.into())
    }

    /// Returns the path of items from the given item to the root of the tree.
    ///
    /// The path is in order from the bottom to the top, starting at the given item and ending at
    /// the root.
    ///
    /// If the id is not in the tree, returns an empty vector.
    pub fn get_id_path(&self, id: impl Into<NodeId>) -> Vec<NodeId> {
        let parents_map = ArenaMapRef {
            parents_map: &self.parents_map,
        };
        parents_map.get_id_path(id.into(), None)
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
        let old_parent = validate_reparent(&self.parents_map, child, new_parent)?;
        if old_parent == Some(new_parent) {
            return Ok(());
        }
        let source_path = parent_path(&self.parents_map, child, None);
        let destination_path = parent_path(&self.parents_map, new_parent, None);
        let parent = self
            .parents_map
            .get_mut(&child)
            .ok_or(ReparentError::InvalidTopology)?;
        TreeNode::move_subtree(&mut self.roots, &source_path, &destination_path)
            .ok_or(ReparentError::InvalidTopology)?;
        *parent = Some(new_parent);
        Ok(())
    }
}

impl<T> TreeNode<T> {
    // Both paths are ordered from the destination/source up to the root. Descend
    // the common branch once, then borrow distinct branches if possible.
    fn move_subtree(
        children: &mut HashMap<NodeId, Self>,
        source: &[NodeId],
        destination: &[NodeId],
    ) -> Option<()> {
        let (&source_id, source_rest) = source.split_last()?;
        let Some((&destination_id, destination_rest)) = destination.split_last() else {
            let node = Self::detach(children, source)?;
            children.insert(node.id, node);
            return Some(());
        };
        if source_id == destination_id {
            return Self::move_subtree(
                &mut children.get_mut(&source_id)?.children,
                source_rest,
                destination_rest,
            );
        }
        if source_rest.is_empty() {
            // Removing a root of this submap requires exclusive access to the
            // submap itself. Restore it if the destination path is invalid.
            let node = children.remove(&source_id)?;
            if let Some(target) = Self::children_at_path(children, destination) {
                target.insert(source_id, node);
            } else {
                children.insert(source_id, node);
                return None;
            }
        } else {
            let [Some(source_root), Some(destination_root)] =
                children.get_disjoint_mut([&source_id, &destination_id])
            else {
                return None;
            };
            let target = Self::children_at_path(&mut destination_root.children, destination_rest)?;
            let node = Self::detach(&mut source_root.children, source_rest)?;
            target.insert(node.id, node);
        }
        Some(())
    }

    fn children_at_path<'a>(
        mut children: &'a mut HashMap<NodeId, Self>,
        path: &[NodeId],
    ) -> Option<&'a mut HashMap<NodeId, Self>> {
        for id in path.iter().rev() {
            children = &mut children.get_mut(id)?.children;
        }
        Some(children)
    }

    fn detach(children: &mut HashMap<NodeId, Self>, path: &[NodeId]) -> Option<Self> {
        let (id, parent_path) = path.split_first()?;
        Self::children_at_path(children, parent_path)?.remove(id)
    }

    fn remove_parent_links(&self, parents: &mut HashMap<NodeId, Option<NodeId>>) {
        for child in self.children.values() {
            child.remove_parent_links(parents);
        }
        parents.remove(&self.id);
    }

    const fn arena_ref<'arena>(
        &'arena self,
        parent_id: Option<NodeId>,
        parents_map: &'arena HashMap<NodeId, Option<NodeId>>,
    ) -> ArenaRef<'arena, T> {
        ArenaRef {
            id: self.id,
            parent_id,
            item: &self.item,
            children: ArenaRefList {
                parent_id: Some(self.id),
                children: &self.children,
                parents_map: ArenaMapRef { parents_map },
            },
        }
    }

    const fn arena_mut<'arena>(
        &'arena mut self,
        parent_id: Option<NodeId>,
        parents_map: &'arena mut HashMap<NodeId, Option<NodeId>>,
    ) -> ArenaMut<'arena, T> {
        ArenaMut {
            id: self.id,
            parent_id,
            item: &mut self.item,
            children: ArenaMutList {
                parent_id: Some(self.id),
                children: &mut self.children,
                parents_map: ArenaMapMut { parents_map },
            },
        }
    }
}

impl<T> ArenaRef<'_, T> {
    /// Id of the item this handle is associated with.
    #[must_use = "Identifies the node associated with this handle."]
    pub const fn id(&self) -> NodeId {
        self.id
    }

    /// An iterator visiting all child ids in arbitrary order.
    #[must_use = "Iterate the returned child identifiers."]
    pub fn child_ids(&self) -> impl IntoIterator<Item = NodeId> {
        self.children.children.keys().copied()
    }
}

impl<'arena, T> ArenaRefList<'arena, T> {
    /// Returns `true` if the list has an element with the given id.
    pub fn has(self, id: impl Into<NodeId>) -> bool {
        let id = id.into();
        self.children.contains_key(&id)
    }

    /// Returns a handle to the element of the list with the given id.
    pub fn item(self, id: impl Into<NodeId>) -> Option<ArenaRef<'arena, T>> {
        let id = id.into();
        self.children
            .get(&id)
            .map(|child| child.arena_ref(self.parent_id, self.parents_map.parents_map))
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
        self.find_inner(id.into())
    }

    fn find_inner(self, id: NodeId) -> Option<ArenaRef<'arena, T>> {
        let parent_id = self.parents_map.parents_map.get(&id)?;

        let id_path = parent_id.map_or_else(Vec::new, |parent_id| {
            self.parents_map.get_id_path(parent_id, self.parent_id)
        });

        let mut id_path = id_path.as_slice();
        let mut node_children = self.children;
        while let Some((ancestor_id, new_id_path)) = id_path.split_last() {
            id_path = new_id_path;
            node_children = &node_children.get(ancestor_id)?.children;
        }

        let node = node_children.get(&id)?;
        Some(node.arena_ref(*parent_id, self.parents_map.parents_map))
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

impl<'arena, T> ArenaMutList<'arena, T> {
    /// Returns `true` if the list has an element with the given id.
    pub fn has(&self, id: impl Into<NodeId>) -> bool {
        let id = id.into();
        self.children.contains_key(&id)
    }

    /// Returns a shared handle to the element of the list with the given id.
    pub fn item(&self, id: impl Into<NodeId>) -> Option<ArenaRef<'_, T>> {
        let id = id.into();
        self.children
            .get(&id)
            .map(|child| child.arena_ref(self.parent_id, self.parents_map.parents_map))
    }

    /// Returns a mutable handle to the element of the list with the given id.
    pub fn item_mut(&mut self, id: impl Into<NodeId>) -> Option<ArenaMut<'_, T>> {
        let id = id.into();
        self.children
            .get_mut(&id)
            .map(|child| child.arena_mut(self.parent_id, self.parents_map.parents_map))
    }

    /// Returns a shared handle to the element of the list with the given id.
    ///
    /// This is the same as [`item`](Self::item), except it consumes
    /// self. This is sometimes necessary to accommodate the borrow checker.
    pub fn into_item(self, id: impl Into<NodeId>) -> Option<ArenaRef<'arena, T>> {
        let id = id.into();
        self.children
            .get(&id)
            .map(|child| child.arena_ref(self.parent_id, self.parents_map.parents_map))
    }

    /// Returns a mutable handle to the element of the list with the given id.
    ///
    /// This is the same as [`item_mut`](Self::item_mut), except it consumes
    /// self. This is sometimes necessary to accommodate the borrow checker.
    pub fn into_item_mut(self, id: impl Into<NodeId>) -> Option<ArenaMut<'arena, T>> {
        let id = id.into();
        self.children
            .get_mut(&id)
            .map(|child| child.arena_mut(self.parent_id, self.parents_map.parents_map))
    }

    // TODO - Remove the child_id argument once creation of widgets is figured out.
    // Return the id instead.
    // TODO - Add #[must_use]
    /// Inserts a child into the tree under the common parent of this list's items.
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
        let child_id = child_id.into();
        assert!(
            !self.parents_map.parents_map.contains_key(&child_id),
            "Key already present"
        );
        self.parents_map
            .parents_map
            .insert(child_id, self.parent_id);

        self.children
            .entry(child_id)
            .insert(TreeNode {
                id: child_id,
                item: value,
                children: HashMap::new(),
            })
            .into_mut()
            .arena_mut(self.parent_id, self.parents_map.parents_map)
    }

    // TODO - How to handle when a subtree is removed?
    // Move children to the root?
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
        let child = self.children.remove(&child_id)?;

        child.remove_parent_links(self.parents_map.parents_map);

        Some(child.item)
    }

    /// Returns a shared handle equivalent to this one.
    #[must_use = "Use the returned shared handle to inspect the arena."]
    pub const fn reborrow(&self) -> ArenaRefList<'_, T> {
        ArenaRefList {
            parent_id: self.parent_id,
            children: &*self.children,
            parents_map: self.parents_map.reborrow(),
        }
    }

    /// Returns a mutable handle equivalent to this one.
    ///
    /// This is sometimes useful to work with the borrow checker.
    pub const fn reborrow_mut(&mut self) -> ArenaMutList<'_, T> {
        ArenaMutList {
            parent_id: self.parent_id,
            children: &mut *self.children,
            parents_map: self.parents_map.reborrow_mut(),
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
        self.find_mut_inner(id.into())
    }

    fn find_mut_inner(self, id: NodeId) -> Option<ArenaMut<'arena, T>> {
        let parent_id = self.parents_map.parents_map.get(&id)?;

        let id_path = parent_id.map_or_else(Vec::new, |parent_id| {
            self.parents_map.get_id_path(parent_id, self.parent_id)
        });

        let mut id_path = id_path.as_slice();
        let mut node_children: &'arena mut _ = &mut *self.children;
        while let Some((ancestor_id, new_id_path)) = id_path.split_last() {
            id_path = new_id_path;
            node_children = &mut node_children.get_mut(ancestor_id)?.children;
        }

        let node = node_children.get_mut(&id)?;
        Some(node.arena_mut(*parent_id, &mut *self.parents_map.parents_map))
    }

    /// Forces reallocation of the child map to exercise outstanding ancestor borrows.
    ///
    /// This is an unstable API which can only be used in tests of the `tree_arena` crate itself,
    /// and may change in any release.
    #[doc(hidden)]
    pub fn realloc_inner_storage(&mut self) {
        let capacity = core::hint::black_box(self.children.capacity());
        self.children.reserve(capacity.saturating_add(32));
        self.children.shrink_to_fit();
    }
}

impl ArenaMapRef<'_> {
    /// Returns the path of items from the given item to the root of the tree.
    ///
    /// The path is in order from the bottom to the top, starting at the given item and ending at
    /// the root.
    ///
    /// If `start_id` is Some, the path ends just before that id instead; `start_id` is not included.
    ///
    /// If there is no path from `start_id` to id, returns an empty vector.
    fn get_id_path(self, id: NodeId, start_id: Option<NodeId>) -> Vec<NodeId> {
        parent_path(self.parents_map, id, start_id)
    }
}

impl ArenaMapMut<'_> {
    /// Returns a shared handle equivalent to this one.
    const fn reborrow(&self) -> ArenaMapRef<'_> {
        ArenaMapRef {
            parents_map: self.parents_map,
        }
    }

    /// Returns a mutable handle equivalent to this one.
    ///
    /// This is sometimes useful to work with the borrow checker.
    const fn reborrow_mut(&mut self) -> ArenaMapMut<'_> {
        ArenaMapMut {
            parents_map: self.parents_map,
        }
    }

    /// Returns the path of items from the given item to the root of the tree.
    ///
    /// The path is in order from the bottom to the top, starting at the given item and ending at
    /// the root.
    ///
    /// If `start_id` is Some, the path ends just before that id instead; `start_id` is not included.
    ///
    /// If there is no path from `start_id` to id, returns an empty vector.
    fn get_id_path(&self, id: NodeId, start_id: Option<NodeId>) -> Vec<NodeId> {
        self.reborrow().get_id_path(id, start_id)
    }
}
