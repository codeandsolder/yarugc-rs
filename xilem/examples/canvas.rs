// Copyright 2024 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! An `imaging` recording can be used directly in Xilem.

use std::sync::atomic::{AtomicU32, Ordering};

use xilem::RunError;
use xilem::kurbo::{Circle, Size};
use xilem::masonry::imaging::{Painter, record::Scene};
use xilem::peniko::Color;
use xilem::view::{canvas, text_button, zstack};
use xilem::{EventLoop, WidgetView, WindowOptions, Xilem};

/// Simple but suboptimal pseudo-random function, you shouldn't use this in production
fn rand_f32() -> f32 {
    static SEED: AtomicU32 = AtomicU32::new(123);
    let previous = SEED
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |x| {
            Some(x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223))
        })
        .unwrap_or_else(|value| value);
    let [high, low, ..] = previous.to_be_bytes();
    f32::from(u16::from_be_bytes([high, low])) / 65_536.0
}

#[derive(Default)]
struct Circles {
    circles: Vec<(Circle, Color)>,
    current_canvas_size: Size,
}

impl Circles {
    fn push_random_circle(&mut self) {
        let position = (
            f64::from(rand_f32()) * self.current_canvas_size.width,
            f64::from(rand_f32()) * self.current_canvas_size.height,
        );
        let radius = f64::from(rand_f32()) * self.current_canvas_size.height / 2.0;
        let color = Color::new([rand_f32(), rand_f32(), rand_f32(), 1.0]);
        self.circles.push((Circle::new(position, radius), color));
    }
    fn view(_: &mut Self) -> impl WidgetView<Self> + use<> {
        zstack((
            canvas(|state: &mut Self, _ctx, scene: &mut Scene, size: Size| {
                let mut painter = Painter::new(scene);
                for (circle, color) in &state.circles {
                    painter.fill(*circle, *color).draw();
                }
                state.current_canvas_size = size;
            }),
            text_button("Add Circle", Self::push_random_circle),
        ))
    }
}

fn main() -> Result<(), RunError> {
    Xilem::new_simple(
        Circles::default(),
        Circles::view,
        WindowOptions::new("Canvas - Circles"),
    )
    .run_in(EventLoop::with_user_event())
}
