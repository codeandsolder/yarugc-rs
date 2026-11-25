// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

// Bitmap and vector images share the same leaf-widget semantics.
// Keep the trait implementations synchronized without introducing dynamic dispatch.
macro_rules! image_widget_methods {
    () => {
        fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

        fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
            if ObjectFit::matches(property_type) {
                ctx.request_layout();
            }
        }

        fn measure(
            &mut self,
            ctx: &mut MeasureCtx<'_>,
            props: &PropertiesRef<'_>,
            axis: Axis,
            len_req: LenReq,
            cross_length: Option<Length>,
        ) -> Length {
            let cache = ctx.property_cache();
            let object_fit = props.get::<ObjectFit>(cache);
            let preferred_size = self.preferred_size();

            object_fit.measure(axis, len_req, cross_length, preferred_size)
        }

        fn accessibility_role(&self) -> Role {
            Role::Image
        }

        fn accessibility(
            &mut self,
            _ctx: &mut AccessCtx<'_>,
            _props: &PropertiesRef<'_>,
            node: &mut Node,
        ) {
            if let Some(alt_text) = &self.alt_text {
                node.set_description(&**alt_text);
            }
            if self.decorative {
                node.set_hidden();
            }
        }

        fn children_ids(&self) -> ChildrenIds {
            ChildrenIds::new()
        }
    };
}

pub(super) use image_widget_methods;
