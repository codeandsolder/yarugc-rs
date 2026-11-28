// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! A stopwatch to display elapsed time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use masonry::dpi::LogicalSize;
use masonry::layout::AsUnit;
use masonry::properties::types::{CrossAxisAlignment, MainAxisAlignment};
use masonry_winit::app::{EventLoop, EventLoopBuilder};
use tokio::time;
use xilem::RunError;
use xilem::core::fork;
use xilem::core::one_of::Either;
use xilem::masonry::usvg::{self, Tree};
use xilem::style::Style;
use xilem::view::{FlexSequence, FlexSpacer, flex_col, flex_row, label, svg, task, text_button};
use xilem::{WidgetView, WindowOptions, Xilem};

const STOPWATCH_SVG: &str = r#"
<svg xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    fill="none"
    stroke="rgb(255, 255, 255)"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round">
        <circle cx="12" cy="13" r="7"/>
        <path d="M12 13l3-2M9 2h6M12 6V4M17 8l1-1"/>
</svg>
"#;

/// The state of the entire application.
///
/// This is owned by Xilem, used to construct the view tree, and updated by event handlers.
struct Stopwatch {
    active: bool,
    /// The duration to add to the duration since the last instant.
    /// This is needed since you can pause a timer, and we need to account for all
    /// time the timer was active before the last start.
    added_duration: Duration,
    /// The absolute time of the last start for calculating elapsed time.
    last_start_time: Option<Instant>,
    /// The duration displayed; updated by by `update_display()`
    displayed_duration: Duration,
    /// A list of the length of all completed splits. Does not include the current split.
    completed_lap_splits: Vec<Duration>,
    /// The duration of the main timer when the split was started.
    split_start_time: Duration,
    /// The SVG stopwatch icon.
    tree: Option<Arc<Tree>>,
}

impl Stopwatch {
    fn start(&mut self) {
        self.last_start_time = Some(Instant::now());
        self.active = true;
        self.update_display();
    }

    fn stop(&mut self) {
        if let Some(start) = self.last_start_time {
            self.added_duration = self.added_duration.saturating_add(start.elapsed());
        }
        self.last_start_time = None;
        self.active = false;
        self.update_display();
    }

    fn reset(&mut self) {
        self.active = false;
        self.last_start_time = None;
        self.added_duration = Duration::ZERO;
        self.completed_lap_splits.clear();
        self.split_start_time = Duration::ZERO;
        self.update_display();
    }

    fn lap(&mut self) {
        let split_end = self.get_current_duration();
        self.completed_lap_splits
            .push(split_end.saturating_sub(self.split_start_time));
        self.split_start_time = split_end;
    }

    fn get_current_duration(&self) -> Duration {
        self.last_start_time.map_or(self.added_duration, |start| {
            self.added_duration.saturating_add(start.elapsed())
        })
    }

    fn update_display(&mut self) {
        self.displayed_duration = self.get_current_duration();
    }
}

fn get_formatted_duration(dur: Duration) -> String {
    let seconds = dur.as_secs_f64() % 60.0;
    let minutes = (dur.as_secs() / 60) % 60;
    let hours = (dur.as_secs() / 60) / 60;
    format!("{hours}:{minutes:0>2}:{seconds:0>4.1}")
}

fn app_logic(data: &mut Stopwatch) -> impl WidgetView<Stopwatch> + use<> {
    fork(
        flex_col((
            FlexSpacer::Fixed(5.px()),
            flex_row((
                data.tree
                    .as_ref()
                    .map(|tree| svg(tree.clone()).dims((75.px(), 75.px()))),
                label(get_formatted_duration(data.displayed_duration)).text_size(70.0),
            ))
            .main_axis_alignment(MainAxisAlignment::Center)
            .cross_axis_alignment(CrossAxisAlignment::Center),
            flex_row((lap_reset_button(data), start_stop_button(data)))
                .main_axis_alignment(MainAxisAlignment::Center),
            FlexSpacer::Fixed(1.px()),
            laps_section(data),
        )),
        data.active.then(|| {
            // Only update while active.
            task(
                |proxy, _| async move {
                    let mut interval = time::interval(Duration::from_millis(50));
                    loop {
                        interval.tick().await;
                        let Ok(()) = proxy.message(()) else {
                            break;
                        };
                    }
                },
                |data: &mut Stopwatch, ()| {
                    data.update_display();
                },
            )
        }),
    )
}

/// Creates a list of items that shows the lap number, split time, and total cumulative time.
fn laps_section(data: &Stopwatch) -> impl FlexSequence<Stopwatch> + use<> {
    let mut items = Vec::new();
    let mut total_dur = Duration::ZERO;
    let current_lap = data.completed_lap_splits.len();
    for (i, split_dur) in data.completed_lap_splits.iter().enumerate() {
        total_dur = total_dur.saturating_add(*split_dur);
        items.push(single_lap(i, *split_dur, total_dur));
    }
    let current_split_duration = data.get_current_duration().saturating_sub(total_dur);
    // Add the current lap, which is not stored in completed_lap_splits
    items.push(single_lap(
        current_lap,
        current_split_duration,
        data.get_current_duration(),
    ));
    items.reverse();
    items
}

fn single_lap(
    lap_id: usize,
    split_dur: Duration,
    total_dur: Duration,
) -> impl WidgetView<Stopwatch> {
    flex_row((
        FlexSpacer::Flex(1.0),
        label(format!("Lap {}", lap_id + 1)),
        label(get_formatted_duration(split_dur)),
        label(get_formatted_duration(total_dur)),
        FlexSpacer::Flex(1.0),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .main_axis_alignment(MainAxisAlignment::Start)
}

fn start_stop_button(data: &Stopwatch) -> impl WidgetView<Stopwatch> + use<> {
    if data.active {
        Either::A(text_button("Stop", |data: &mut Stopwatch| {
            data.stop();
        }))
    } else {
        Either::B(text_button("Start", |data: &mut Stopwatch| {
            data.start();
        }))
    }
}

fn lap_reset_button(data: &Stopwatch) -> impl WidgetView<Stopwatch> + use<> {
    if data.active {
        Either::A(text_button("  Lap  ", |data: &mut Stopwatch| {
            data.lap();
        }))
    } else {
        Either::B(text_button("Reset", |data: &mut Stopwatch| {
            data.reset();
        }))
    }
}

pub(crate) fn run(event_loop: EventLoopBuilder) -> Result<(), RunError> {
    let tree = Tree::from_str(STOPWATCH_SVG, &usvg::Options::default())
        .map(Arc::new)
        .map_err(|err| tracing::warn!(%err, "Could not load stopwatch icon"))
        .ok();
    let mut data = Stopwatch {
        active: false,
        added_duration: Duration::ZERO,
        last_start_time: None,
        displayed_duration: Duration::ZERO,
        completed_lap_splits: Vec::new(),
        split_start_time: Duration::ZERO,
        tree,
    };
    data.update_display();

    let window_options = WindowOptions::new("Stopwatch")
        .with_min_inner_size(LogicalSize::new(300., 200.))
        .with_initial_inner_size(LogicalSize::new(450., 300.));
    let app = Xilem::new_simple(data, app_logic, window_options);
    app.run_in(event_loop)?;
    Ok(())
}

// Boilerplate code: Identical across all applications which support Android

fn main() -> Result<(), RunError> {
    run(EventLoop::with_user_event())
}
