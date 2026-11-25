// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::{
    LayoutCtx, MeasureCtx, PaintCtx, PrePaintProps, PropertiesRef, WidgetPod, paint_background,
    paint_box_shadow,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Cap, Dashes, Join, Point, Size, Stroke};
use crate::layout::{LayoutSize, LenReq, Length, SizeDef};
use crate::theme;
use crate::widgets::Label;

pub(super) fn measure(
    label: &WidgetPod<Label>,
    ctx: &mut MeasureCtx<'_>,
    axis: Axis,
    len_req: LenReq,
    cross_length: Option<Length>,
) -> Length {
    let check_side = theme::BASIC_WIDGET_HEIGHT;
    let check_padding = theme::WIDGET_CONTROL_COMPONENT_PADDING;

    let calc_other_length = |axis| match axis {
        Axis::Horizontal => check_side.saturating_add(check_padding),
        Axis::Vertical => Length::ZERO,
    };
    let other_length = calc_other_length(axis);

    let cross = axis.cross();
    let cross_space = cross_length.map(|cross_length| {
        let cross_other_length = calc_other_length(cross);
        cross_length.saturating_sub(cross_other_length)
    });

    let auto_length = len_req.reduce(other_length).into();
    let context_size = LayoutSize::maybe(cross, cross_space);

    let label_length = ctx.compute_length(label, auto_length, context_size, axis, cross_space);

    match axis {
        Axis::Horizontal => label_length.saturating_add(other_length),
        Axis::Vertical => label_length.max(check_side).saturating_add(other_length),
    }
}

pub(super) fn layout(label: &WidgetPod<Label>, ctx: &mut LayoutCtx<'_>, size: Size) {
    let check_side = theme::BASIC_WIDGET_HEIGHT.get();
    let check_padding = theme::WIDGET_CONTROL_COMPONENT_PADDING.get();

    let space = Size::new(
        (size.width - (check_side + check_padding)).max(0.),
        size.height,
    );

    let label_size = ctx.compute_size(label, SizeDef::fit(space), space.into());
    ctx.run_layout(label, label_size);

    let label_origin = Point::new(check_side + check_padding, 0.);
    ctx.place_child(label, label_origin);

    ctx.derive_baselines(label);
}

pub(super) fn pre_paint(
    ctx: &mut PaintCtx<'_>,
    props: &PropertiesRef<'_>,
    painter: &mut Painter<'_>,
) {
    let bbox = ctx.border_box();
    let cache = ctx.property_cache();
    let p = PrePaintProps::fetch(props, cache);

    paint_box_shadow(painter, bbox, p.box_shadow, p.corner_radius);
    paint_background(painter, bbox, p.background, p.border_width, p.corner_radius);

    // Paint focus indicator around the entire widget (box + label)
    if ctx.is_focus_target() || ctx.is_hovered() {
        // TODO: Replace this custom implementation with the general paint_border()

        let focus_rect = bbox.inflate(2.0, 2.0);

        let focus_color = p.border_color.color;
        let focus_width = 2.0;
        let focus_radius = 4.0;

        let focus_stroke = Stroke {
            width: focus_width,
            join: Join::Round,
            miter_limit: 10.0,
            start_cap: Cap::Round,
            end_cap: Cap::Round,
            dash_pattern: Dashes::default(),
            dash_offset: 0.0,
        };
        let focus_path = focus_rect.to_rounded_rect(focus_radius);
        painter
            .stroke(focus_path, &focus_stroke, focus_color)
            .draw();
    }
    // Skip painting the regular border while the check border uses that property
}

// Both selection controls expose the same measurement, layout and background painting.
macro_rules! control_methods {
    () => {
        fn measure(
            &mut self,
            ctx: &mut MeasureCtx<'_>,
            _props: &PropertiesRef<'_>,
            axis: Axis,
            len_req: LenReq,
            cross_length: Option<Length>,
        ) -> Length {
            super::selection_control::measure(&self.label, ctx, axis, len_req, cross_length)
        }

        fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
            super::selection_control::layout(&self.label, ctx, size);
        }

        fn pre_paint(
            &mut self,
            ctx: &mut PaintCtx<'_>,
            props: &PropertiesRef<'_>,
            painter: &mut Painter<'_>,
        ) {
            super::selection_control::pre_paint(ctx, props, painter);
        }
    };
}
pub(super) use control_methods;
