// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::any::TypeId;

use accesskit::{Node, Role, Toggled};
use include_doc_path::include_doc_path;
use tracing::{Span, trace_span};

use crate::core::{
    AccessCtx, ChildrenIds, EventCtx, LayoutCtx, MeasureCtx, PaintCtx, PropertiesMut,
    PropertiesRef, RegisterCtx, Update, UpdateCtx, UsesProperty, Widget, WidgetId, WidgetMut,
};
use crate::imaging::Painter;
use crate::kurbo::{Axis, Circle, Join, Point, Rect, Size, Stroke};
use crate::layout::{LenReq, Length};
use crate::properties::{
    Background, BorderColor, BorderWidth, CornerRadius, ThumbColor, ThumbRadius, TrackThickness,
};

/// A switch switch that can be turned on or off.
///
#[doc = concat!(
    "![Switch in on state](",
    include_doc_path!("screenshots/switch_on_initial.png"),
    ")",
)]
///
/// This is a boolean control similar to a checkbox, but with a sliding switch appearance.
/// The switch displays a track with a circular thumb that sits on the left when off
/// and on the right when on.
///
/// Emits [`SwitchToggled`] when the user activates it.
/// Note that the on state does not automatically switch, and so one of
/// the responses to a `SwitchToggled` is to call [`Switch::set_on`]
/// on the originating widget.
///
/// This allows higher-level components to choose how the switch responds,
/// and ensure that its value is based on their correct source of truth.
///
/// # Classes
///
/// When toggled, this widget will have the `#toggled` [class].
///
/// [class]: masonry_core::doc::masonry_concepts#classes
pub struct Switch {
    on: bool,
    activation: super::selection_control::Activation,
}

// --- MARK: BUILDERS
impl Switch {
    /// Creates a new `Switch` with the given initial state.
    #[must_use = "Use the returned value."]
    pub const fn new(on: bool) -> Self {
        Self {
            on,
            activation: super::selection_control::Activation::new(),
        }
    }

    /// Returns whether the switch is currently on.
    #[must_use = "Use the returned value."]
    pub const fn is_on(&self) -> bool {
        self.on
    }
}

// --- MARK: WIDGETMUT
impl Switch {
    /// Sets the switch state.
    pub fn set_on(this: &mut WidgetMut<'_, Self>, on: bool) {
        if this.widget.on == on {
            return;
        }
        this.widget.on = on;
        this.ctx.request_render();
        if on {
            this.ctx.add_class("#toggled");
        } else {
            this.ctx.remove_class("#toggled");
        }
    }
}

// --- MARK: HELPERS
impl Switch {
    /// Calculates the track dimensions based on properties.
    ///
    /// Returns `(track_width, track_height)`.
    const fn track_dimensions(track_thickness: Length, thumb_radius: Length) -> (Length, Length) {
        // The track height is the larger of track_thickness or thumb diameter
        let track_height = track_thickness.max(thumb_radius.saturating_add(thumb_radius));
        // The track width is approximately 2x the height (pill shape)
        let track_width = track_height.saturating_add(track_height);

        (track_width, track_height)
    }
}

// Resolve the track in the allocated content box. The stroke may occupy
// the surrounding border insets, but the thumb stays inside the track.
#[derive(Debug)]
struct SwitchGeometry {
    track: Rect,
    thumb_radius: f64,
    border_width: f64,
}

impl SwitchGeometry {
    fn fit(size: Size, thickness: Length, radius: Length, border: Length) -> Option<Self> {
        let (width, height) = Switch::track_dimensions(thickness, radius);
        if width == Length::ZERO
            || height == Length::ZERO
            || size.width <= 0.0
            || size.height <= 0.0
        {
            return None;
        }
        let scale = (size.width / width.get())
            .min(size.height / height.get())
            .min(1.0);
        let width = width.get() * scale;
        let height = height.get() * scale;
        let border_width = border.get().min(width.min(height) / 3.0);
        let thumb_radius = (radius.get() * scale).min((width.min(height) - border_width) / 2.0);
        let origin = Point::new((size.width - width) / 2.0, (size.height - height) / 2.0);
        Some(Self {
            track: Rect::from_origin_size(origin, Size::new(width, height)),
            thumb_radius,
            border_width,
        })
    }

    fn thumb(&self, on: bool) -> Circle {
        let inset = self.thumb_radius + self.border_width / 2.0;
        let x = if on {
            self.track.x1 - inset
        } else {
            self.track.x0 + inset
        };
        Circle::new(Point::new(x, self.track.center().y), self.thumb_radius)
    }
}

impl UsesProperty<ThumbRadius> for Switch {}
impl UsesProperty<ThumbColor> for Switch {}
impl UsesProperty<TrackThickness> for Switch {}

/// The action type emitted by [`Switch`] when it is activated.
///
/// The field is the target switch state (i.e. true is "this switch would like to become on").
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct SwitchToggled(pub bool);

impl Switch {
    fn activate(&self, ctx: &mut EventCtx<'_>) {
        ctx.submit_action::<SwitchToggled>(SwitchToggled(!self.on));
    }
}

// --- MARK: IMPL WIDGET
impl Widget for Switch {
    type Action = SwitchToggled;

    super::selection_control::activation_methods!();

    fn accepts_focus(&self) -> bool {
        true
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        self.activation.update(event);
        match event {
            Update::WidgetAdded if self.on => {
                ctx.add_class("#toggled");
            }
            Update::HoveredChanged(_)
            | Update::ActiveChanged(_)
            | Update::FocusChanged(_)
            | Update::DisabledChanged(_) => {
                ctx.request_paint_only();
            }

            _ => {}
        }
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        ThumbRadius::prop_changed(ctx, property_type);
        ThumbColor::prop_changed(ctx, property_type);
        TrackThickness::prop_changed(ctx, property_type);
    }

    fn measure(
        &mut self,
        ctx: &mut MeasureCtx<'_>,
        props: &PropertiesRef<'_>,
        axis: Axis,
        _len_req: LenReq,
        _cross_length: Option<Length>,
    ) -> Length {
        let cache = ctx.property_cache();
        let track_thickness = props.get::<TrackThickness>(cache).0;
        let thumb_radius = props.get::<ThumbRadius>(cache).0;
        let (track_width, track_height) = Self::track_dimensions(track_thickness, thumb_radius);

        match axis {
            Axis::Horizontal => track_width,
            Axis::Vertical => track_height,
        }
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, _size: Size) {}

    fn pre_paint(
        &mut self,
        _ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        _painter: &mut Painter<'_>,
    ) {
        // TODO: Make Switch painting work with generic shadow/background/border
    }

    fn paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let is_disabled = ctx.is_disabled();
        let size = ctx.content_box().size();
        let cache = ctx.property_cache();
        let thickness = props.get::<TrackThickness>(cache).0;
        let radius = props.get::<ThumbRadius>(cache).0;
        let border = props.get::<BorderWidth>(cache).width;
        let Some(geometry) = SwitchGeometry::fit(size, thickness, radius, border) else {
            return;
        };

        let corner_radius = props.get::<CornerRadius>(cache).radius.get();
        let thumb_color = props.get::<ThumbColor>(cache).0;
        let track_bg = props.get::<Background>(cache);

        let track_rect = geometry.track;
        let track_corner_radius = corner_radius
            .min(track_rect.height() / 2.0)
            .min(track_rect.width() / 2.0);
        let track_rounded = track_rect.to_rounded_rect(track_corner_radius);
        let brush = track_bg.get_peniko_brush_for_rect(track_rect);
        painter.fill(track_rounded, &brush).draw();

        if geometry.border_width > 0.0 {
            let border_color = props.get::<BorderColor>(cache);
            let stroke = Stroke::new(geometry.border_width).with_join(Join::Miter);
            painter
                .stroke(track_rounded, &stroke, border_color.color)
                .draw();
        }

        if geometry.thumb_radius > 0.0 {
            let thumb_brush = if is_disabled {
                thumb_color.with_alpha(0.5)
            } else {
                thumb_color
            };
            painter.fill(geometry.thumb(self.on), thumb_brush).draw();
        }
    }

    fn accessibility_role(&self) -> Role {
        Role::Switch
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.add_action(accesskit::Action::Click);
        if self.on {
            node.set_toggled(Toggled::True);
        } else {
            node.set_toggled(Toggled::False);
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("Switch", id = id.trace())
    }

    fn get_debug_text(&self) -> Option<String> {
        if self.on {
            Some("[ON]".to_string())
        } else {
            Some("[OFF]".to_string())
        }
    }
}

// --- MARK: TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{TextEvent, keyboard::Key};
    use crate::properties::types::{CrossAxisAlignment, MainAxisAlignment};
    use crate::testing::{TestHarness, assert_render_snapshot};
    use crate::theme::test_property_set;
    use crate::widgets::Flex;

    // --- MARK: NON-RENDERING BEHAVIOR TESTS

    #[test]
    fn switch_track_and_thumb_fit_constrained_content() {
        for size in [
            Size::new(40.0, 20.0),
            Size::new(4.0, 40.0),
            Size::new(40.0, 3.0),
            Size::new(1.0, 1.0),
        ] {
            let geometry =
                SwitchGeometry::fit(size, Length::px(20.0), Length::px(10.0), Length::px(50.0))
                    .unwrap();
            let track = geometry.track;
            assert!(track.x0 >= -1e-9 && track.y0 >= -1e-9);
            assert!(track.x1 <= size.width + 1e-9 && track.y1 <= size.height + 1e-9);
            for on in [false, true] {
                let thumb = geometry.thumb(on);
                let margin = thumb.radius + geometry.border_width / 2.0;
                assert!(thumb.center.x - margin >= track.x0 - 1e-9);
                assert!(thumb.center.x + margin <= track.x1 + 1e-9);
                assert!(thumb.center.y - margin >= track.y0 - 1e-9);
                assert!(thumb.center.y + margin <= track.y1 + 1e-9);
            }
        }
        assert!(
            SwitchGeometry::fit(Size::ZERO, Length::px(20.0), Length::px(10.0), Length::ZERO)
                .is_none()
        );
        assert!(
            SwitchGeometry::fit(
                Size::new(40.0, 20.0),
                Length::ZERO,
                Length::ZERO,
                Length::ZERO
            )
            .is_none()
        );
    }

    #[test]
    fn click_emits_action_and_focuses() {
        let widget = Switch::new(false).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (60, 40));
        let switch_id = harness.root_id();

        // Initially not focused, and no actions
        assert!(harness.focused_widget().is_none());
        // Initially no actions
        assert!(harness.pop_action_erased().is_none());

        // Click on switch (off -> wants to be on)
        harness.mouse_click_on(switch_id, None).unwrap();
        assert_eq!(harness.focused_widget().map(|w| w.id()), Some(switch_id));
        assert_eq!(
            harness.pop_action::<SwitchToggled>().unwrap(),
            Some((SwitchToggled(true), switch_id))
        );

        // Update state to on
        harness
            .edit_root_widget(|mut switch| Switch::set_on(&mut switch, true))
            .unwrap();

        // Click again (on -> wants to be off)
        harness.mouse_click_on(switch_id, None).unwrap();
        assert_eq!(
            harness.pop_action::<SwitchToggled>().unwrap(),
            Some((SwitchToggled(false), switch_id))
        );
    }

    #[test]
    fn space_emits_action_when_focused() {
        let widget = Switch::new(false).prepare();
        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (60, 40));
        let switch_id = harness.root_id();

        // Focus via tab
        harness.focus_on(None);
        harness.press_tab_key(false);
        assert_eq!(harness.focused_widget().map(|w| w.id()), Some(switch_id));

        // Space key should switch (off -> wants on)
        harness.process_text_event(&TextEvent::key_down(Key::Character(" ".into())));
        harness.process_text_event(&TextEvent::key_up(Key::Character(" ".into())));
        assert_eq!(
            harness.pop_action::<SwitchToggled>().unwrap(),
            Some((SwitchToggled(true), switch_id))
        );

        // Update state
        harness
            .edit_root_widget(|mut switch| Switch::set_on(&mut switch, true))
            .unwrap();

        // Space again (on -> wants off)
        harness.process_text_event(&TextEvent::key_down(Key::Character(" ".into())));
        harness.process_text_event(&TextEvent::key_up(Key::Character(" ".into())));
        assert_eq!(
            harness.pop_action::<SwitchToggled>().unwrap(),
            Some((SwitchToggled(false), switch_id))
        );
    }

    #[test]
    fn state_change_updates_accessibility_without_class_styles() {
        let widget = Switch::new(false).prepare();
        let mut harness =
            TestHarness::create_with_size(crate::core::DefaultProperties::new(), widget, (60, 40));
        let id = harness.root_id();
        let _ = harness.redraw();
        assert_eq!(
            harness.access_node(id).unwrap().toggled(),
            Some(Toggled::False)
        );

        for (on, toggled) in [(true, Toggled::True), (false, Toggled::False)] {
            harness
                .edit_root_widget(|mut switch| Switch::set_on(&mut switch, on))
                .unwrap();
            let _ = harness.redraw();
            assert_eq!(harness.access_node(id).unwrap().toggled(), Some(toggled));
        }
    }

    #[test]
    fn measure_dimensions() {
        // Test that the switch measures to expected dimensions based on theme properties.
        // Theme defaults: ThumbRadius(8px), TrackThickness(20px), BorderWidth(1px)
        // Expected: track_height = max(20, 8*2) = 20, track_width = 20*2 = 40
        // With borders: width = 42, height = 22
        let switch = Switch::new(false).prepare();
        let switch_id = switch.id();

        // Wrap in Flex with Start alignment so it doesn't stretch the switch
        let flex = Flex::row()
            .with_fixed(switch)
            .main_axis_alignment(MainAxisAlignment::Start)
            .cross_axis_alignment(CrossAxisAlignment::Start);

        // Give it much more space than needed
        let harness =
            TestHarness::create_with_size(test_property_set(), flex.prepare(), (200, 100));

        let size = harness
            .get_widget_with_id(switch_id)
            .unwrap()
            .ctx()
            .border_box()
            .size();

        // Switch should maintain its intrinsic size, not fill available space
        assert_eq!(
            size.width, 42.0,
            "Switch width should be 2*track_height + 2*border"
        );
        assert_eq!(
            size.height, 22.0,
            "Switch height should be track_height + 2*border"
        );
    }

    // --- MARK: SNAPSHOT TESTS

    #[test]
    fn simple_switch() {
        let widget = Switch::new(false).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (60, 40));
        let switch_id = harness.root_id();

        assert_render_snapshot!(harness, "switch_off");

        assert!(harness.pop_action_erased().is_none());

        // Hover without clicking to show hovered state (no focus)
        harness.mouse_move_to(switch_id).unwrap();
        assert_render_snapshot!(harness, "switch_off_hovered");

        // Now click to switch
        harness.mouse_click_on(switch_id, None).unwrap();
        assert_eq!(
            harness.pop_action::<SwitchToggled>().unwrap(),
            Some((SwitchToggled(true), switch_id))
        );

        harness
            .edit_root_widget(|mut switch| Switch::set_on(&mut switch, true))
            .unwrap();

        // After clicking, the switch is both focused and hovered
        assert_render_snapshot!(harness, "switch_on_focused_hovered");
    }

    #[test]
    fn focus_visual() {
        let widget = Switch::new(false).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (60, 40));
        let switch_id = harness.root_id();

        // Focus directly (not via click) to get focused-but-not-hovered state
        harness.focus_on(Some(switch_id));
        assert_eq!(harness.focused_widget().map(|w| w.id()), Some(switch_id));

        assert_render_snapshot!(harness, "switch_focused");
    }

    #[test]
    fn on_state() {
        let widget = Switch::new(true).prepare();

        let mut harness = TestHarness::create_with_size(test_property_set(), widget, (60, 40));

        assert_render_snapshot!(harness, "switch_on_initial");
    }
}
