// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

// After you edit the crate's doc comment, run this command, then check README.md for any missing links
// cargo rdme --workspace-project=placehero

//! A mastodon client written in [Xilem][xilem].
//!
//! We're assuming that all Mastodon servers supported are trusted (and so it's not a
//! privacy violation for them to know that two accounts you log in to are linked).
//! This link survives even if you log out of one and into the other, even in different sessions.
//! If this doesn't apply to you, we recommend not using Placehero.
//!
//! # Feature flags
//!
//! The following crate [feature flags](https://doc.rust-lang.org/cargo/reference/features.html#dependency-features) are available:
//!
//! - `default`: Enables the default features of [Xilem][xilem].

use std::sync::Arc;

use crate::mastodon::{Context, Instance, Mastodon, Status};
use tokio::sync::mpsc::{Receiver, Sender};
use xilem::core::one_of::{Either, OneOf, OneOf3, OneOf6};
use xilem::core::{NoElement, View, fork, lens, map_action, map_state};
use xilem::masonry::layout::AsUnit;
use xilem::style::Style;
use xilem::view::{
    CrossAxisAlignment, FlexExt, MainAxisAlignment, flex_col, flex_row, label, prose, sized_box,
    spinner, split, task_raw, text_button, text_input, worker_raw,
};
use xilem::{EventLoopBuilder, ViewCtx, WidgetView, WindowOptions, Xilem};

#[expect(
    clippy::redundant_pub_crate,
    reason = "Internal types must remain crate-visible, not public API."
)]
mod actions;
#[expect(
    clippy::redundant_pub_crate,
    reason = "Internal types must remain crate-visible, not public API."
)]
mod avatars;
#[expect(
    clippy::redundant_pub_crate,
    reason = "Internal types must remain crate-visible, not public API."
)]
mod components;
#[expect(
    clippy::redundant_pub_crate,
    reason = "Internal types must remain crate-visible, not public API."
)]
mod html_content;
#[expect(
    clippy::redundant_pub_crate,
    reason = "Internal types must remain crate-visible, not public API."
)]
mod login_flow;

pub(crate) use avatars::Avatars;
pub(crate) use html_content::status_html_to_plaintext;

use crate::actions::Navigation;
use crate::components::{Timeline, thread};
use crate::login_flow::PlaceheroWithLogin;

#[expect(
    clippy::redundant_pub_crate,
    reason = "Internal types must remain crate-visible, not public API."
)]
mod mastodon;

/// Execute the app in the given winit event loop.
///
/// # Errors
/// Returns an error if the application runtime or window event loop fails.
pub fn run(event_loop: EventLoopBuilder) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let handle = runtime.handle().clone();
    Xilem::new_simple(
        MainState::Selecting(None),
        move |state| select_app(state, &handle),
        WindowOptions::new("Placehero: A placeholder named Mastodon client"),
    )
    .run_in(event_loop)?;
    Ok(())
}

/// We are developing a version of Placehero which supports login.
///
/// This requires some quite gnarly refactors, so for now we built it "alongside"
/// the main Placehero.
#[expect(clippy::large_enum_variant, reason = "Not passed around.")]
enum MainState {
    Selecting(Option<String>),
    Old(Placehero),
    New(PlaceheroWithLogin),
}

fn select_app(
    state: &mut MainState,
    runtime: &tokio::runtime::Handle,
) -> impl WidgetView<MainState> + use<> {
    match state {
        MainState::Selecting(error) => OneOf3::A(
            flex_col((
                prose("Welcome to Placehero. This is an example of the Xilem GUI framework, which is a Mastodon client.\n\
                    We currently have decent support for browsing anonymously, and are currently developing our logged-in support in parallel to avoid regressions.")
                    .text_alignment(xilem::TextAlign::Center)
                    .flex(CrossAxisAlignment::Center),
                error.as_ref().map(|err| prose(err.clone())),
                flex_row((
                    text_button("Browse Anonymously", |state: &mut MainState| {
                        *state = match Placehero::new() {
                            Ok(app) => MainState::Old(app),
                            Err(err) => MainState::Selecting(Some(format!("Could not initialize Mastodon client: {err}"))),
                        };
                    }),
                    text_button("Log In", |state: &mut MainState| {
                        *state = MainState::New(PlaceheroWithLogin::new());
                    }),
                ))
                .main_axis_alignment(MainAxisAlignment::Center),
            ))
            .main_axis_alignment(MainAxisAlignment::Center),
        ),
        MainState::Old(_) => {
            let runtime = runtime.clone();
            OneOf::B(lens(move |state| app_logic(state, &runtime), |state: &mut MainState| {
            let MainState::Old(placehero) = state else {
                unreachable!()
            };
            placehero
        }))
        },
        MainState::New(_) => OneOf::C(lens(login_flow::app_logic, |state: &mut MainState| {
            let MainState::New(placehero) = state else {
                unreachable!()
            };
            placehero
        })),
    }
}

struct Placehero {
    mastodon: Mastodon,
    instance: Option<Instance>,
    timeline: Timeline,
    show_context: Option<Arc<Status>>,
    context: Option<Context>,
    context_sender: Option<Sender<String>>,
    account_sender: Option<Sender<String>>,
    timeline_box_contents: String,
    loading_timeline: bool,
    not_found_acct: Option<String>,
    account_error: Option<String>,
    context_error: Option<String>,
    instance_error: Option<String>,
}

impl Placehero {
    fn new() -> anyhow::Result<Self> {
        // TODO: Configurable server?
        let mastodon = Mastodon::new("https://mastodon.online")?;

        Ok(Self {
            mastodon,
            instance: None,
            timeline: Timeline::default(),
            show_context: None,
            context: None,
            context_sender: None,
            account_sender: None,
            timeline_box_contents: "raph".to_string(),
            loading_timeline: false,
            not_found_acct: None,
            account_error: None,
            context_error: None,
            instance_error: None,
        })
    }
}

impl Placehero {
    fn sidebar(&self) -> impl WidgetView<Self, Navigation> + use<> {
        let Some(instance) = &self.instance else {
            return Either::B(prose(
                self.instance_error.as_deref().unwrap_or("Connecting…"),
            ));
        };
        let back = if self.show_context.is_some() {
            // TODO: Make the ⬅️ arrow not be available to screen readers.
            Either::A(text_button("⬅️ Back to Timeline", |_: &mut Self| {
                Navigation::Home
            }))
        } else {
            // Sized box of a flex because nested flexes aren't supported (? is this true)
            // TODO: Ideally, we'd be able to use Either with fragments here
            Either::B(sized_box(flex_col((
                text_input(
                    self.timeline_box_contents.clone(),
                    |state: &mut Self, string| {
                        state.timeline_box_contents = string;
                        Navigation::None
                    },
                )
                .on_enter(|_, user| Navigation::LoadUser(user))
                .disabled(self.loading_timeline),
                self.loading_timeline.then(|| spinner().dims(50.px())),
                text_button("Go", |state: &mut Self| {
                    Navigation::LoadUser(state.timeline_box_contents.clone())
                })
                .disabled(self.loading_timeline),
            ))))
        };
        Either::A(flex_col((
            label("Connected to:"),
            // TODO: We should probably use an ArcStr for this?
            prose(instance.title.as_str()),
            back,
        )))
    }

    fn main_view(&self) -> impl WidgetView<Self, Navigation> + use<> {
        if let Some(show_context) = self.show_context.as_ref() {
            return self.context.as_ref().map_or_else(
                || {
                    OneOf::B(prose(
                        self.context_error.as_deref().unwrap_or("Loading thread"),
                    ))
                },
                |context| OneOf6::A(thread(show_context, context)),
            );
        }
        if self.loading_timeline {
            OneOf::C(
                flex_col(spinner().dims(50.px())).cross_axis_alignment(CrossAxisAlignment::Center),
            )
        } else if let Some(error) = self.account_error.as_ref() {
            OneOf::D(prose(error.clone()))
        } else if let Some(acct) = self.not_found_acct.as_ref() {
            OneOf::D(prose(format!(
                "Could not find account @{acct} on this server. \
                 You might need to include the server name of the account, if it's on a different server."
            )))
        } else if self.timeline.has_account() {
            OneOf::E(map_state(self.timeline.view(), |this: &mut Self| {
                &mut this.timeline
            }))
        } else {
            OneOf::F(prose("No statuses yet loaded"))
        }
    }
}

fn app_logic(
    app_state: &mut Placehero,
    runtime: &tokio::runtime::Handle,
) -> impl WidgetView<Placehero> + use<> {
    Avatars::provide(
        runtime,
        app_state.mastodon.client().clone(),
        fork(
            map_action(
                split(app_state.sidebar(), app_state.main_view()).split_point(0.2),
                |state, action| match action {
                    Navigation::LoadContext(status) => {
                        let requested = state
                            .context_sender
                            .as_ref()
                            .is_some_and(|sender| sender.try_send(status.id.clone()).is_ok());
                        state.show_context = Some(status);
                        state.context_error =
                            (!requested).then(|| "Thread worker is unavailable".to_owned());
                        state.context = None;
                    }
                    Navigation::LoadUser(user) => {
                        // It's fine to set `timeline_box_contents` manually here, because we
                        // set state.loading_timeline (i.e. the box will become disabled).
                        state.timeline_box_contents.clone_from(&user);
                        let requested = state
                            .account_sender
                            .as_ref()
                            .is_some_and(|sender| sender.try_send(user).is_ok());
                        state.loading_timeline = requested;
                        state.account_error =
                            (!requested).then(|| "Account worker is unavailable".to_owned());
                        state.not_found_acct = None;
                        state.context = None;
                        state.show_context = None;
                    }
                    Navigation::Home => {
                        state.context = None;
                        state.show_context = None;
                    }
                    Navigation::None => {}
                },
            ),
            (
                load_instance(runtime, app_state.mastodon.clone()),
                load_account(runtime, app_state.mastodon.clone()),
                load_contexts(runtime, app_state.mastodon.clone()),
                map_state(
                    Timeline::worker(runtime, app_state.mastodon.clone()),
                    |state: &mut Placehero| &mut state.timeline,
                ),
            ),
        ),
    )
}

fn load_contexts(
    runtime: &tokio::runtime::Handle,
    mastodon: Mastodon,
) -> impl View<Placehero, (), ViewCtx, Element = NoElement> + use<> {
    worker_raw(
        runtime,
        std::num::NonZeroUsize::MIN,
        move |result, mut recv: Receiver<String>| {
            let mastodon = mastodon.clone();
            async move {
                let mut task = tokio::task::JoinSet::new();
                while let Some(req) = recv.recv().await {
                    // Only the newest thread request is useful. Abort and
                    // reap the superseded task before starting another.
                    task.shutdown().await;
                    let mastodon = mastodon.clone();
                    let result = result.clone();
                    task.spawn(async move {
                        let context_result = mastodon.context(&req).await;
                        drop(result.message((context_result, req)));
                    });
                }
            }
        },
        |app_state: &mut Placehero, sender| app_state.context_sender = Some(sender),
        |app_state: &mut Placehero, (event, id)| match event {
            Ok(context)
                if app_state
                    .show_context
                    .as_ref()
                    .is_some_and(|it| it.id == id) =>
            {
                app_state.context = Some(context);
                app_state.context_error = None;
            }
            Ok(_) => {
                tracing::warn!("Dropping context whose request was superseded.");
            }
            Err(error) => {
                if app_state
                    .show_context
                    .as_ref()
                    .is_some_and(|status| status.id == id)
                {
                    tracing::warn!(%error, "Could not load thread");
                    app_state.context_error = Some(format!("Could not load thread: {error}"));
                }
            }
        },
    )
}

fn load_instance(
    runtime: &tokio::runtime::Handle,
    mastodon: Mastodon,
) -> impl View<Placehero, (), ViewCtx, Element = NoElement> + use<> {
    task_raw(
        runtime,
        move |result, _| {
            let mastodon = mastodon.clone();
            async move {
                // We choose not to handle the case where the event loop has ended
                let instance_result = mastodon.instance().await;
                // Note that error handling is deferred to the on_event handler
                drop(result.message(instance_result));
            }
        },
        |app_state: &mut Placehero, event| match event {
            Ok(instance) => app_state.instance = Some(instance),
            Err(error) => {
                tracing::warn!(%error, "Could not load instance");
                app_state.instance_error = Some(format!("Could not connect: {error}"));
            }
        },
    )
}

fn load_account(
    runtime: &tokio::runtime::Handle,
    mastodon: Mastodon,
) -> impl View<Placehero, (), ViewCtx, Element = NoElement> + use<> {
    worker_raw(
        runtime,
        std::num::NonZeroUsize::MIN,
        move |result, mut recv: Receiver<String>| {
            let mastodon = mastodon.clone();
            async move {
                while let Some(req) = recv.recv().await {
                    let instance_result = mastodon.account(&req).await;
                    // We choose not to handle the case where the event loop has ended
                    // Note that error handling is deferred to the on_event handler
                    drop(result.message((instance_result, req)));
                }
            }
        },
        |app_state: &mut Placehero, sender| app_state.account_sender = Some(sender),
        |app_state: &mut Placehero, (event, acct)| {
            if !app_state.loading_timeline || app_state.timeline_box_contents != acct {
                return;
            }
            match event {
                Ok(instance) => {
                    app_state.timeline.set_account(instance);
                    app_state.loading_timeline = false;
                    app_state.not_found_acct = None;
                }
                Err(e)
                    if e.downcast_ref::<reqwest::Error>()
                        .and_then(reqwest::Error::status)
                        == Some(reqwest::StatusCode::NOT_FOUND) =>
                {
                    tracing::warn!(error = %e, "Account not found");
                    // TODO: Handle more gracefully/surface to the user.
                    // This at least lets the user retry.
                    // Note that we don't unset the timeline here, because it's technically
                    // possible for this response to arrive extremely quickly
                    app_state.loading_timeline = false;
                    app_state.not_found_acct = Some(acct);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Could not load account");
                    app_state.loading_timeline = false;
                    app_state.account_error = Some(format!("Could not load @{acct}: {e}"));
                }
            }
        },
    )
}
