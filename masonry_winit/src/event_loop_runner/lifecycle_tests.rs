// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use super::*;
use masonry::{properties::ContentColor, widgets::Label};
use winit::platform::x11::EventLoopBuilderExtX11;

#[derive(Default)]
struct Driver {
    starts: usize,
    created: Vec<WindowId>,
    actions: Vec<(WindowId, u32)>,
    devices: Vec<wgpu::Device>,
    lost_devices: Vec<GpuDeviceId>,
}

impl AppDriver for Driver {
    fn on_action(
        &mut self,
        _: WindowId,
        _: &mut DriverCtx<'_>,
        _: masonry_core::core::WidgetId,
        _: ErasedAction,
    ) {
    }

    fn on_start(&mut self, _: &mut MasonryState) {
        self.starts += 1;
    }

    fn on_window_created(&mut self, id: WindowId, _: &mut Window) {
        self.created.push(id);
    }

    fn on_wgpu_ready(&mut self, wgpu: &WgpuContext<'_>) {
        self.devices.push(wgpu.device.clone());
    }

    fn on_wgpu_lost(&mut self, id: GpuDeviceId) {
        self.lost_devices.push(id);
    }

    fn on_async_action(&mut self, id: WindowId, ctx: &mut DriverCtx<'_>, action: ErasedAction) {
        assert!(
            self.created.contains(&id),
            "async action arrived before window {id:?} initialization"
        );
        let value = *action.downcast::<u32>().unwrap();
        self.actions.push((id, value));
        if value == 99 {
            ctx.exit();
        }
    }
}

struct Lifecycle {
    state: MasonryState,
    driver: Driver,
    initial: WindowId,
    ran: bool,
}

fn hidden_window() -> NewWindow {
    NewWindow::new(
        WindowAttributes::default().with_visible(false),
        Label::new("lifecycle test").prepare().erased(),
    )
}

impl Lifecycle {
    fn send_action(&mut self, event_loop: &ActiveEventLoop, id: WindowId, value: u32) {
        self.state.handle_user_event(
            event_loop,
            MasonryUserEvent::AsyncAction(id, Box::new(value)),
            &mut self.driver,
        );
    }

    fn resume(&mut self, event_loop: &ActiveEventLoop) {
        self.state.handle_resumed(event_loop, &mut self.driver);
    }

    fn prepare_surface(&mut self, id: WindowId) -> GpuDeviceId {
        let handle = self.state.handle_id(id).unwrap();
        let window = self.state.windows.get(&handle).unwrap();
        MasonryState::prepare_surface(
            &mut self.state.render_cx,
            &mut self.state.surfaces,
            window,
            &mut self.driver,
        )
        .unwrap()
        .dev_id
    }

    fn exercise_gpu_recovery(
        &mut self,
        event_loop: &ActiveEventLoop,
        first: WindowId,
        second: WindowId,
    ) {
        let old = self.prepare_surface(first);
        assert_eq!(
            self.prepare_surface(second),
            old,
            "compatible windows must share their initial GPU device"
        );
        assert_eq!(
            self.state.gpu_device_id(first),
            Some(old),
            "surface must expose its initial device identity"
        );
        assert_eq!(
            self.state.present_mode(first),
            Some(wgpu::PresentMode::AutoVsync),
            "initial surface must use the requested presentation mode"
        );
        assert_eq!(
            self.driver.devices.len(),
            1,
            "initial shared device must be announced only once"
        );
        let device = self.driver.devices[0].clone();

        // Explicit destruction follows the same WGPU callback path as a lost
        // physical device. Poll to drain callbacks before delivering the event.
        device.destroy();
        let _ = self.state.render_cx.instance.poll_all(true);
        assert!(
            self.state.render_cx.devices[&old].is_lost(),
            "destroying the GPU device must trigger its loss callback"
        );
        self.state.handle_user_event(
            event_loop,
            MasonryUserEvent::GpuDeviceLost(old),
            &mut self.driver,
        );
        assert!(
            self.state.surfaces.is_empty(),
            "all surfaces on the destroyed device must be detached"
        );
        assert!(
            self.state.render_cx.devices.is_empty(),
            "lost device must leave the live registry"
        );
        assert_eq!(
            self.state.gpu_device_id(first),
            None,
            "detached surface cannot report a live device"
        );
        assert_eq!(
            self.state.present_mode(first),
            None,
            "detached surface cannot report a configured mode"
        );
        assert_eq!(
            self.driver.lost_devices,
            [old],
            "driver must receive exactly one loss notification for the old device"
        );
        assert!(
            !self.state.add_wgpu_features(wgpu::Features::empty()),
            "device configuration must stay frozen after initial allocation"
        );

        self.exercise_replacement_device(event_loop, first, second, old, &device);
    }

    fn exercise_replacement_device(
        &mut self,
        event_loop: &ActiveEventLoop,
        first: WindowId,
        second: WindowId,
        old: GpuDeviceId,
        device: &wgpu::Device,
    ) {
        let new = self.prepare_surface(first);
        assert_ne!(new, old, "replacement device must have a fresh identity");
        assert_eq!(
            self.prepare_surface(second),
            new,
            "both compatible surfaces must reuse the replacement device"
        );
        assert_eq!(
            self.state.gpu_device_id(first),
            Some(new),
            "recreated surface must expose replacement identity"
        );
        assert_eq!(
            self.driver.devices.len(),
            2,
            "replacement device must be announced exactly once"
        );
        assert_ne!(
            &self.driver.devices[1], device,
            "replacement must use a distinct WGPU device"
        );
        // Late duplicate loss notification cannot discard replacement surfaces.
        self.state.handle_user_event(
            event_loop,
            MasonryUserEvent::GpuDeviceLost(old),
            &mut self.driver,
        );
        assert_eq!(
            self.state.surfaces.len(),
            2,
            "stale loss notifications cannot detach replacement surfaces"
        );
        assert_eq!(
            self.driver.lost_devices,
            [old],
            "driver must receive exactly one loss notification for the old device"
        );
        assert_eq!(
            self.state.render_cx.devices.len(),
            1,
            "only the replacement device may remain registered"
        );
        assert_eq!(
            self.prepare_surface(first),
            new,
            "stale loss notification must not force another replacement"
        );
        assert_eq!(
            self.driver.devices.len(),
            2,
            "replacement device must be announced exactly once"
        );
    }

    fn exercise(&mut self, event_loop: &ActiveEventLoop) {
        // A task started while constructing the initial window can finish
        // before the first native resume.
        self.send_action(event_loop, self.initial, 1);
        assert!(
            self.driver.actions.is_empty(),
            "actions before initial resume must remain queued"
        );
        self.resume(event_loop);
        assert_eq!(
            self.driver.starts, 1,
            "startup must run only once across resumes"
        );
        assert_eq!(
            self.driver.created,
            [self.initial],
            "initial window must be initialized exactly once"
        );
        assert_eq!(
            self.driver.actions,
            [(self.initial, 1)],
            "only the initial action may be delivered before the next resume"
        );

        self.resume(event_loop);
        assert_eq!(
            self.driver.starts, 1,
            "startup must run only once across resumes"
        );

        let mut properties = DefaultProperties::new();
        let _ = properties.insert::<Label, _>(ContentColor::new(Color::WHITE));
        let window = hidden_window().with_default_properties(Arc::new(properties));
        let live = window.id;
        self.state.create_window(event_loop, window).unwrap();
        self.state.handle_signals(event_loop, &mut self.driver);
        assert_eq!(
            self.driver.created,
            [self.initial, live],
            "new live window must be initialized without restarting app"
        );
        let window = self.state.window_mut(live).unwrap();
        assert_eq!(
            window
                .render_root()
                .get_layer_root(0)
                .unwrap()
                .get_prop::<ContentColor>()
                .color,
            Color::WHITE,
            "new window must receive its per-window properties"
        );
        window.set_default_properties(None);
        assert_eq!(
            window
                .render_root()
                .get_layer_root(0)
                .unwrap()
                .get_prop::<ContentColor>()
                .color,
            Color::BLACK,
            "removing an override must restore application defaults"
        );
        window.set_base_color(Color::WHITE);
        assert_eq!(
            window.base_color(),
            Color::WHITE,
            "runtime base-color update must take effect"
        );

        self.exercise_suspension(event_loop, live);
    }

    fn exercise_suspension(&mut self, event_loop: &ActiveEventLoop, live: WindowId) {
        self.state.handle_suspended(event_loop);
        let pending = hidden_window();
        let pending_id = pending.id;
        self.state.create_window(event_loop, pending).unwrap();
        let cancelled = hidden_window();
        let cancelled_id = cancelled.id;
        self.state.create_window(event_loop, cancelled).unwrap();
        self.send_action(event_loop, live, 2);
        self.send_action(event_loop, pending_id, 3);
        self.send_action(event_loop, cancelled_id, 4);
        assert!(
            self.state.close_window(cancelled_id),
            "queued window must be cancellable before creation"
        );
        assert_eq!(
            self.driver.actions,
            [(self.initial, 1)],
            "only the initial action may be delivered before the next resume"
        );
        self.resume(event_loop);

        assert_eq!(
            self.driver.starts, 1,
            "startup must run only once across resumes"
        );
        assert_eq!(
            self.driver.created,
            [self.initial, live, pending_id],
            "only surviving queued window may be initialized on resume"
        );
        assert_eq!(
            self.driver.actions,
            [(self.initial, 1), (live, 2), (pending_id, 3)],
            "resume must deliver surviving queued actions in order"
        );
        assert!(
            self.state.suspended_actions.is_empty(),
            "resume must drain the suspended action queue"
        );

        self.exercise_gpu_recovery(event_loop, self.initial, live);

        self.send_action(event_loop, pending_id, 99);
        assert!(
            self.state.exit,
            "async exit action must update application state"
        );
        assert!(
            event_loop.exiting(),
            "async exit must stop the native event loop without another window event"
        );
    }
}

impl ApplicationHandler<MasonryUserEvent> for Lifecycle {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.ran {
            self.ran = true;
            self.exercise(event_loop);
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: HandleId, _: WinitWindowEvent) {}
}

#[test]
#[ignore = "Requires an X11 display; CI runs explicitly with xvfb-run."]
fn native_window_lifecycle_and_gpu_recovery() {
    let event_loop = EventLoop::with_user_event()
        .with_x11()
        .with_any_thread(true)
        .build()
        .unwrap();
    let initial = hidden_window();
    let initial_id = initial.id;
    let mut properties = DefaultProperties::new();
    let _ = properties.insert::<Label, _>(ContentColor::new(Color::BLACK));
    let mut lifecycle = Lifecycle {
        state: MasonryState::new(
            event_loop.create_proxy(),
            event_loop.owned_display_handle(),
            vec![initial],
            properties,
            masonry_imaging::texture_render::Backend::Vello,
        )
        .unwrap(),
        driver: Driver::default(),
        initial: initial_id,
        ran: false,
    };
    event_loop.run_app(&mut lifecycle).unwrap();
    assert!(lifecycle.ran);
}
