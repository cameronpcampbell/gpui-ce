use crate::{
    AnyElement, App, Bounds, ContentMask, Element, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window,
};

/// Builds a `Deferred` element, which delays the layout and paint of its child.
pub fn deferred(child: impl IntoElement) -> Deferred {
    Deferred {
        child: Some(child.into_any_element()),
        priority: DeferredPriority::from(0),
        content_mask: None,
    }
}

/// An element which delays the painting of its child until after all of
/// its ancestors, while keeping its layout as part of the current element tree.
///
/// Per [`Window::prepaint_deferred_draws`], deferred elements causing additional deferred elements
/// should be constrained to limited circumstances and will stop processing after some depth
/// (otherwise the renderer would be subject to an infinite loop when processing deferred draws).
#[derive(gpui_macros::Reflect)]
pub struct Deferred {
    child: Option<AnyElement>,
    priority: DeferredPriority,
    content_mask: Option<ContentMask<Pixels>>,
}

impl Element for Deferred {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<crate::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let layout_id = self.child.as_mut().unwrap().request_layout(window, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = self.child.take().unwrap();
        let element_offset = window.element_offset();
        let priority = self.priority.evaluate(cx);
        window.defer_draw(child, element_offset, priority, self.content_mask);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

impl IntoElement for Deferred {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Deferred {
    /// Sets a priority for the element. A higher priority conceptually means painting the element
    /// on top of deferred draws with a lower priority (i.e. closer to the viewer).
    pub fn priority(mut self, priority: impl Into<DeferredPriority>) -> Self {
        self.priority = priority.into();
        self
    }

    /// Configures the priority of the deferred element to be 1 greater than the previous
    /// deferred element (when this element is caused by another deferred element, even if indirectly).
    /// Defaults to 1 when this is a deferred element caused by non-deferred elements.
    pub fn priority_auto(mut self) -> Self {
        self.priority = DeferredPriority::Auto;
        self
    }

    /// When a content mask is provided, the deferred element will be clipped to that region during
    /// both prepaint and paint.
    pub fn content_mask(mut self, mask: ContentMask<Pixels>) -> Self {
        self.content_mask = Some(mask);
        self
    }
}

/// Describes how the priority for a [`Deferred`] element is calculated.
pub enum DeferredPriority {
    /// The priority is calculated based on whether the deferred element is a by-product of another deferred element.
    /// If this is the first deferred element in a rendering stack, priority is 1.
    /// Otherwise the priority is the previous priority + 1 (so this element renders in front of the previous in the stack).
    Auto,
    /// Takes an explicit priority value for the deferred element.
    Value(usize),
}
impl From<usize> for DeferredPriority {
    fn from(value: usize) -> Self {
        Self::Value(value)
    }
}
impl DeferredPriority {
    /// Calculates the new priority based on the current value in the [`DeferredPriorityStackCache`].
    /// Should only be called during prepaint of a [`Deferred`] element.
    fn evaluate(&self, cx: &App) -> usize {
        match self {
            DeferredPriority::Value(value) => *value,
            DeferredPriority::Auto => match DeferredPriorityStackCache::current_depth(cx) {
                None => 1, // NOTE: functionality change. If using auto, we start at a priority of 1 instead of 0.
                Some(prev_deferred_priority) => prev_deferred_priority.saturating_add(1),
            },
        }
    }
}

/// Internal global to track the depth of deferred renders during prepaint.
#[derive(Default)]
pub(crate) struct DeferredPriorityStackCache(Vec<usize>);
impl crate::Global for DeferredPriorityStackCache {}
impl DeferredPriorityStackCache {
    pub(crate) fn push(priority: usize, cx: &mut App) {
        cx.default_global::<Self>().0.push(priority);
    }

    pub(crate) fn pop(cx: &mut App) {
        cx.default_global::<Self>().0.pop();
    }

    fn current_depth(cx: &App) -> Option<usize> {
        let cache = cx.try_global::<Self>()?;
        cache.0.last().copied()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Context, Entity, Select, StyleRefinement, TestAppContext, Window, anchored, deferred, div,
        point, prelude::*, px, size,
    };
    use std::{
        cell::{Cell, RefCell},
        rc::{Rc, Weak},
    };

    /// A stand-in for a dock panel hosting a popover (deferred draw) whose
    /// content opens another popover (a deferred draw created while
    /// prepainting the first one's content).
    struct PanelView {
        captures: Rc<RefCell<Vec<Weak<Cell<usize>>>>>,
    }

    impl Render for PanelView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let capture = Rc::new(Cell::new(0));
            self.captures.borrow_mut().push(Rc::downgrade(&capture));

            div().key_context("Panel").size_full().child(
                deferred(
                    anchored().position(point(px(10.), px(10.))).child(
                        div()
                            .key_context("Popover")
                            .w(px(200.))
                            .h(px(200.))
                            .child(
                                deferred(
                                    anchored().position(point(px(30.), px(30.))).child(
                                        div()
                                            .key_context("NestedMenu")
                                            .debug_selector(|| "NESTED_MENU".into())
                                            .size(px(10.))
                                            .class("menu"),
                                    ),
                                )
                                .priority(2),
                            )
                            .select(
                                Select::descendants().class("menu").reflects(crate::Styled),
                                move |element| {
                                    capture.set(capture.get() + 1);

                                    element.size(px(50.))
                                },
                            ),
                    ),
                )
                .priority(1),
            )
        }
    }

    struct RootView {
        panel: Entity<PanelView>,
    }

    impl Render for RootView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().key_context("Root").size_full().child(
                self.panel
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
        }
    }

    /// Nested deferred ranges must index the retained draw vector so cached replay
    /// reuses the correct dispatch nodes. Replay must also release old selector captures.
    #[gpui::test]
    fn test_nested_deferred_draws_with_reused_views(cx: &mut TestAppContext) {
        let captures = Rc::new(RefCell::new(Vec::new()));
        let captures_for_panel = captures.clone();
        let window = cx.open_window(size(px(800.), px(600.)), |_, cx| {
            let panel = cx.new(|_| PanelView {
                captures: captures_for_panel,
            });

            RootView { panel }
        });
        cx.run_until_parked();

        let menu_bounds = window
            .update(cx, |_, window, _| {
                window
                    .rendered_frame
                    .debug_bounds
                    .get("NESTED_MENU")
                    .copied()
            })
            .unwrap()
            .expect("NESTED_MENU debug bounds not found");
        assert_eq!(menu_bounds.size, size(px(50.), px(50.)));
        assert_eq!(captures.borrow().len(), 1);
        assert_eq!(captures.borrow()[0].upgrade().unwrap().get(), 1);

        // Re-render the root while the panel and both deferred records stay cached.
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.run_until_parked();
        assert_eq!(captures.borrow().len(), 1);
        assert!(captures.borrow()[0].upgrade().is_none());

        // Reuse the subtree a second time, exercising ranges that were
        // themselves recorded during a reused frame.
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.run_until_parked();
        assert_eq!(captures.borrow().len(), 1);

        // Re-render the panel itself again to prove the popovers still draw.
        window
            .update(cx, |root, _, cx| {
                root.panel.update(cx, |_, cx| cx.notify());
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(captures.borrow().len(), 2);
        assert_eq!(captures.borrow()[1].upgrade().unwrap().get(), 1);

        window
            .update(cx, |_, window, _| {
                assert_eq!(window.rendered_frame.deferred_draws.len(), 2);
                assert!(
                    window
                        .rendered_frame
                        .debug_bounds
                        .contains_key("NESTED_MENU")
                );
            })
            .unwrap();
    }
}
