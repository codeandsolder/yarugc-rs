// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::core::{AppendVec, MessageCtx, MessageResult, ViewId};
use crate::elements::DomChildrenSplice;
use crate::{AnyPod, DomError, DomFragment, DynMessage, ViewCtx};
use wasm_bindgen::{JsCast, UnwrapThrowExt};

pub(crate) struct AppMessage {
    pub id_path: Rc<[ViewId]>,
    pub body: DynMessage,
}

/// The type responsible for running your app.
pub struct App<State: 'static, Fragment: DomFragment<State>, InitFragment>(
    Rc<RefCell<AppInner<State, Fragment, InitFragment>>>,
);

struct AppInner<State: 'static, Fragment: DomFragment<State>, InitFragment> {
    data: State,
    root: web_sys::Node,
    app_logic: InitFragment,
    fragment: Option<(Fragment, Fragment::SeqState)>,
    fragment_append_scratch: AppendVec<AnyPod>,
    vec_splice_scratch: Vec<AnyPod>,
    elements: Vec<AnyPod>,
    ctx: ViewCtx,
}

pub(crate) trait AppRunner {
    fn handle_message(&self, message: AppMessage);

    fn clone_box(&self) -> Box<dyn AppRunner>;
}

impl<State, Fragment: DomFragment<State>, InitFragment> Clone
    for App<State, Fragment, InitFragment>
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<State, Fragment, InitFragment> App<State, Fragment, InitFragment>
where
    State: 'static,
    Fragment: DomFragment<State> + 'static,
    InitFragment: FnMut(&mut State) -> Fragment + 'static,
{
    /// Create an instance of your app with the given logic and initial state.
    ///
    /// # Errors
    /// Returns an error if the root does not belong to a document.
    pub fn new(
        root: impl AsRef<web_sys::Node>,
        data: State,
        app_logic: InitFragment,
    ) -> Result<Self, DomError> {
        let root = root.as_ref().clone();
        let document = root
            .owner_document()
            .or_else(|| root.dyn_ref::<web_sys::Document>().cloned())
            .ok_or(DomError::DocumentUnavailable)?;
        let app = Rc::new_cyclic(|weak| {
            let ctx = ViewCtx::new(document, Box::new(WeakApp(weak.clone())));
            RefCell::new(AppInner::new(root, data, app_logic, ctx))
        });
        Ok(Self(app))
    }

    /// Run the app.
    ///
    /// Because we don't want to block the render thread, we return immediately here. The app is
    /// forgotten, and will continue to respond to events in the background.
    pub fn run(self) {
        self.0.borrow_mut().ensure_app();
        // The running app owns its state for the lifetime of the page.
        std::mem::forget(self);
    }
}

impl<State, Fragment: DomFragment<State>, InitFragment: FnMut(&mut State) -> Fragment>
    AppInner<State, Fragment, InitFragment>
{
    fn new(root: web_sys::Node, data: State, app_logic: InitFragment, ctx: ViewCtx) -> Self {
        Self {
            data,
            root,
            app_logic,
            fragment: None,
            elements: Vec::new(),
            ctx,
            fragment_append_scratch: AppendVec::default(),
            vec_splice_scratch: Vec::default(),
        }
    }

    fn ensure_app(&mut self) {
        if self.fragment.is_none() {
            let fragment = (self.app_logic)(&mut self.data);
            let state = fragment.seq_build(
                &mut self.ctx,
                &mut self.fragment_append_scratch,
                &mut self.data,
            );
            self.fragment = Some((fragment, state));

            // TODO should the element provide a separate method to access reference instead?
            let append_vec = std::mem::take(&mut self.fragment_append_scratch);

            self.elements = append_vec.into_inner();
            for pod in &self.elements {
                self.root.append_child(pod.node.as_ref()).unwrap_throw();
            }
        }
    }
}

impl<State, Fragment, InitFragment> AppRunner for App<State, Fragment, InitFragment>
where
    State: 'static,
    Fragment: DomFragment<State> + 'static,
    InitFragment: FnMut(&mut State) -> Fragment + 'static,
{
    // For now we handle the message synchronously, but it would also
    // make sense to to batch them (for example with requestAnimFrame).
    fn handle_message(&self, message: AppMessage) {
        let Ok(mut inner_guard) = self.0.try_borrow_mut() else {
            // DOM callbacks can run synchronously while applying a rebuild.
            // Defer reentrant messages until the current operation has completed.
            let app = self.clone();
            wasm_bindgen_futures::spawn_local(async move {
                app.handle_message(message);
            });
            return;
        };
        let inner = &mut *inner_guard;
        if let Some((fragment, fragment_state)) = &mut inner.fragment {
            let message_result;
            {
                let env = std::mem::take(&mut inner.ctx.environment);
                let mut message_context =
                    MessageCtx::new(env, (*message.id_path).into(), message.body);
                let mut dom_children_splice = DomChildrenSplice::new(
                    &mut inner.fragment_append_scratch,
                    &mut inner.elements,
                    &mut inner.vec_splice_scratch,
                    &inner.root,
                    inner.ctx.fragment.clone(),
                    false,
                    false,
                );
                message_result = fragment.seq_message(
                    fragment_state,
                    &mut message_context,
                    &mut dom_children_splice,
                    &mut inner.data,
                );
                let (env, _path, _message) = message_context.finish();
                inner.ctx.environment = env;
            }

            // Unit callbacks may mutate app state even when returning Nop.
            // Stale messages, however, must not force a rebuild.
            if matches!(message_result, MessageResult::Stale) {
                return;
            }

            let new_fragment = (inner.app_logic)(&mut inner.data);
            let mut dom_children_splice = DomChildrenSplice::new(
                &mut inner.fragment_append_scratch,
                &mut inner.elements,
                &mut inner.vec_splice_scratch,
                &inner.root,
                inner.ctx.fragment.clone(),
                false,
                false,
            );
            new_fragment.seq_rebuild(
                fragment,
                fragment_state,
                &mut inner.ctx,
                &mut dom_children_splice,
                &mut inner.data,
            );
            *fragment = new_fragment;
        }
    }

    fn clone_box(&self) -> Box<dyn AppRunner> {
        Box::new(self.clone())
    }
}

struct WeakApp<State: 'static, Fragment: DomFragment<State>, InitFragment>(
    Weak<RefCell<AppInner<State, Fragment, InitFragment>>>,
);
impl<State, Fragment, InitFragment> AppRunner for WeakApp<State, Fragment, InitFragment>
where
    State: 'static,
    Fragment: DomFragment<State> + 'static,
    InitFragment: FnMut(&mut State) -> Fragment + 'static,
{
    fn handle_message(&self, message: AppMessage) {
        if let Some(app) = self.0.upgrade() {
            App(app).handle_message(message);
        }
    }
    fn clone_box(&self) -> Box<dyn AppRunner> {
        Box::new(Self(self.0.clone()))
    }
}
