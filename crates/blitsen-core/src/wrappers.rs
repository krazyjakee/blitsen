//! The wrapper table: one JavaScript object per node, for as long as it lives.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use blitsen_js::{ExternalId, JsEngine, JsError};

/// Weak-reference operations needed by the wrapper identity table.
///
/// Every complete [`JsEngine`] implements this automatically. The smaller
/// boundary also permits deterministic identity-table tests without mocking
/// the rest of a JavaScript runtime.
pub trait WrapperEngine {
    /// JavaScript object handle.
    type Value: Clone;
    /// Engine-owned weak reference.
    type WeakRef;

    /// Creates a weak reference to a wrapper.
    fn downgrade_wrapper(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError>;
    /// Upgrades a weak reference while its wrapper remains live.
    fn upgrade_wrapper(
        &mut self,
        reference: &Self::WeakRef,
    ) -> Result<Option<Self::Value>, JsError>;
}

impl<E: JsEngine> WrapperEngine for E {
    type Value = E::Value;
    type WeakRef = E::WeakRef;

    fn downgrade_wrapper(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError> {
        self.downgrade(value)
    }

    fn upgrade_wrapper(
        &mut self,
        reference: &Self::WeakRef,
    ) -> Result<Option<Self::Value>, JsError> {
        self.upgrade(reference)
    }
}

struct WrapperEntry<W> {
    weak: W,
    token: u64,
}

/// Preserves one JavaScript wrapper identity for each live node handle.
///
/// The table holds only weak JavaScript references. A wrapper finalizer removes
/// its own entry, so JavaScript reachability—not the cache—controls collection.
pub struct WrapperTable<N, W> {
    entries: Rc<RefCell<HashMap<N, WrapperEntry<W>>>>,
    next_token: Cell<u64>,
}

impl<N, W> Default for WrapperTable<N, W> {
    fn default() -> Self {
        Self {
            entries: Rc::new(RefCell::new(HashMap::new())),
            next_token: Cell::new(0),
        }
    }
}

impl<N, W> WrapperTable<N, W>
where
    N: Clone + Eq + Hash + 'static,
    W: 'static,
{
    /// Creates an empty identity table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the existing live wrapper or creates exactly one replacement.
    ///
    /// `create` receives the finalizer that must be attached to the new native
    /// JavaScript object. It may compose that callback with node-lifetime work,
    /// but must invoke it once when the wrapper is collected.
    pub fn get_or_create<E, F>(
        &self,
        engine: &mut E,
        node: N,
        create: F,
    ) -> Result<E::Value, JsError>
    where
        E: WrapperEngine<WeakRef = W>,
        F: FnOnce(&mut E, Box<dyn FnOnce(ExternalId) + 'static>) -> Result<E::Value, JsError>,
    {
        let existing = {
            let entries = self.entries.borrow();
            match entries.get(&node) {
                Some(entry) => engine.upgrade_wrapper(&entry.weak)?,
                None => None,
            }
        };
        if let Some(wrapper) = existing {
            return Ok(wrapper);
        }
        self.entries.borrow_mut().remove(&node);

        let token = self.next_token.get();
        self.next_token.set(token.wrapping_add(1));
        let entries = Rc::downgrade(&self.entries);
        let finalizer_node = node.clone();
        let finalizer = Box::new(move |_external: ExternalId| {
            let Some(entries) = entries.upgrade() else {
                return;
            };
            let mut entries = entries.borrow_mut();
            if entries
                .get(&finalizer_node)
                .is_some_and(|entry| entry.token == token)
            {
                entries.remove(&finalizer_node);
            }
        });

        let wrapper = create(engine, finalizer)?;
        let weak = engine.downgrade_wrapper(&wrapper)?;
        self.entries
            .borrow_mut()
            .insert(node, WrapperEntry { weak, token });
        Ok(wrapper)
    }

    /// Removes entries whose JavaScript wrappers have already been collected.
    ///
    /// Finalizers normally keep the table current. This is a defensive sweep
    /// for hosts that defer finalizer callbacks until a later loop turn.
    pub fn prune_collected<E>(&self, engine: &mut E) -> Result<usize, JsError>
    where
        E: WrapperEngine<WeakRef = W>,
    {
        let collected = {
            let entries = self.entries.borrow();
            let mut collected = Vec::new();
            for (node, entry) in entries.iter() {
                if engine.upgrade_wrapper(&entry.weak)?.is_none() {
                    collected.push((node.clone(), entry.token));
                }
            }
            collected
        };
        let count = collected.len();
        let mut entries = self.entries.borrow_mut();
        for (node, token) in collected {
            if entries.get(&node).is_some_and(|entry| entry.token == token) {
                entries.remove(&node);
            }
        }
        Ok(count)
    }

    /// Returns the number of cached weak wrappers.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// Reports whether no node currently has a cached wrapper.
    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }
}
